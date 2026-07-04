#!/usr/bin/env bash
# Assemble photondns OpenWrt packages (.ipk) from a pre-built binary.
#
# usage: build-packages.sh <rust-target-triple> <path-to-photondns-binary> <outdir>
#
# Produces, in <outdir>:
#   photondns_<ver>_<openwrt-arch>.ipk           (the daemon, arch-specific)
#   luci-app-photondns_<ver>_all.ipk             (modern JS/ucode LuCI app)
#   luci-app-photondns-compat_<ver>_all.ipk      (legacy Lua/CBI LuCI app)
#
# The two LuCI packages are architecture-independent (Architecture: all) so
# they are only emitted once; re-running for another arch just rebuilds the
# identical files.
set -euo pipefail

TRIPLE="$1"; BIN="$2"; OUT="$3"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -1)"
RELEASE=1
MAINTAINER="c2h2"

mkdir -p "$OUT"

# rust triple -> OpenWrt package Architecture string
case "$TRIPLE" in
	aarch64-unknown-linux-musl)        ARCH=aarch64_generic ;;
	x86_64-unknown-linux-musl)         ARCH=x86_64 ;;
	armv7-unknown-linux-musleabihf)    ARCH=arm_cortex-a7_neon-vfpv4 ;;
	arm-unknown-linux-musleabi)        ARCH=arm_arm926ej-s ;;
	mips-unknown-linux-musl)           ARCH=mips_24kc ;;
	mipsel-unknown-linux-musl)         ARCH=mipsel_24kc ;;
	riscv64gc-unknown-linux-musl)      ARCH=riscv64_riscv64 ;;
	*) echo "unknown target triple: $TRIPLE" >&2; exit 1 ;;
esac

echo "==> photondns $VERSION  triple=$TRIPLE  openwrt-arch=$ARCH"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------- daemon pkg
stage="$WORK/photondns"
mkdir -p "$stage/usr/bin"
install -m 0755 "$BIN" "$stage/usr/bin/photondns"

ctrl="$WORK/photondns.control"
cat > "$ctrl" <<EOF
Package: photondns
Version: $VERSION-$RELEASE
Architecture: $ARCH
Maintainer: $MAINTAINER
Section: net
Priority: optional
License: GPL-3.0-only
Description: High-performance DNS forwarder written in Rust.
 Sharded LRU cache, serve-stale, prefetch, cache persistence,
 UDP/TCP/DoT/DoH upstreams and hedged-racing failover with
 per-upstream health tracking.
EOF

bash "$HERE/mkipk.sh" "$stage" "$ctrl" \
	"$OUT/photondns_${VERSION}-${RELEASE}_${ARCH}.ipk"

if [ "${EMIT_APK:-0}" = "1" ]; then
	bash "$HERE/mkapk.sh" "$stage" photondns "$VERSION-r$RELEASE" "$ARCH" \
		"High-performance DNS forwarder written in Rust" \
		"$OUT/photondns_${VERSION}-${RELEASE}_${ARCH}.apk"
fi

# ------------------------------------------------------ LuCI apps (arch: all)
# Only build these once (on the first arch) to avoid redundant identical files.
if [ "${SKIP_LUCI:-0}" != "1" ]; then

	build_luci() {
		# build_luci <pkgname> <srcroot> <depends> <description>
		local pkg="$1" srcroot="$2" deps="$3" desc="$4"
		local s="$WORK/$pkg"
		mkdir -p "$s"
		# copy the package's on-device tree
		if [ -d "$srcroot/root" ]; then cp -R "$srcroot/root/." "$s/"; fi
		if [ -d "$srcroot/htdocs" ]; then
			mkdir -p "$s/www/luci-static"
			cp -R "$srcroot/htdocs/luci-static/." "$s/www/luci-static/"
		fi
		# compile zh_Hans po -> lmo if present
		local po="$srcroot/po/zh_Hans/photondns.po"
		if [ -f "$po" ] && command -v python3 >/dev/null 2>&1; then
			mkdir -p "$s/usr/lib/lua/luci/i18n"
			python3 "$REPO/tools/po2lmo.py" "$po" \
				"$s/usr/lib/lua/luci/i18n/photondns.zh-cn.lmo" 2>/dev/null \
				|| rmdir "$s/usr/lib/lua/luci/i18n" 2>/dev/null || true
		fi
		# ensure shipped scripts are executable
		find "$s" -path '*/etc/init.d/*' -o -path '*/usr/bin/*' -o -path '*/etc/uci-defaults/*' 2>/dev/null \
			| while read -r f; do [ -f "$f" ] && chmod 0755 "$f"; done

		local c="$WORK/$pkg.control"
		cat > "$c" <<EOF
Package: $pkg
Version: $VERSION-$RELEASE
Architecture: all
Maintainer: $MAINTAINER
Section: luci
Priority: optional
License: GPL-3.0-only
Depends: $deps
Description: $desc
EOF
		# preserve user config + rule files across upgrades
		local conf="$WORK/$pkg.conffiles"
		{
			echo "/etc/config/photondns"
		} > "$conf"

		# postinst: run this package's uci-defaults + clear luci cache,
		# exactly as opkg's default_postinst would.
		local post="$WORK/$pkg.postinst"
		{
			echo '#!/bin/sh'
			echo '[ -n "${IPKG_INSTROOT}" ] && exit 0'
			# apply each uci-defaults file this package ships, then remove it
			if [ -d "$s/etc/uci-defaults" ]; then
				for d in "$s"/etc/uci-defaults/*; do
					[ -f "$d" ] || continue
					local base; base="$(basename "$d")"
					echo "[ -f /etc/uci-defaults/$base ] && ( . /etc/uci-defaults/$base ) && rm -f /etc/uci-defaults/$base"
				done
			fi
			echo 'rm -f /tmp/luci-indexcache* /tmp/luci-modulecache/* 2>/dev/null'
			echo '/etc/init.d/rpcd reload 2>/dev/null || true'
			echo 'exit 0'
		} > "$post"

		bash "$HERE/mkipk.sh" "$s" "$c" \
			"$OUT/${pkg}_${VERSION}-${RELEASE}_all.ipk" \
			"$conf" "$post"

		if [ "${EMIT_APK:-0}" = "1" ]; then
			# apk depends: space-separated, no commas
			local apkdeps; apkdeps="$(echo "$deps" | tr ',' ' ')"
			bash "$HERE/mkapk.sh" "$s" "$pkg" "$VERSION-r$RELEASE" all \
				"$desc" "$OUT/${pkg}_${VERSION}-${RELEASE}_all.apk" \
				"$apkdeps" "$post"
		fi
	}

	build_luci luci-app-photondns \
		"$REPO/openwrt/luci-app-photondns" \
		"photondns, curl, rpcd, rpcd-mod-ucode" \
		"LuCI web UI for photondns (JS client + ucode backend)."

	build_luci luci-app-photondns-compat \
		"$REPO/openwrt/luci-app-photondns-compat" \
		"photondns, curl, luci-compat" \
		"LuCI web UI for photondns (legacy Lua/CBI, for old firmware)."
fi

echo "==> done. artifacts in $OUT:"
ls -la "$OUT"
