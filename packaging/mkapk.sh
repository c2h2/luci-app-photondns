#!/usr/bin/env bash
# Build an OpenWrt .apk (APKv3, apk-tools 3.x) from a staged package tree.
#
# usage: mkapk.sh <staging_dir> <name> <version> <arch> <description> <out.apk> \
#                 [depends] [postinst_script] [--conf path ...]
#
# Requires the `apk` binary (apk-tools >= 3.0), which is what OpenWrt 25.12+
# uses (24.10 and earlier still use opkg/.ipk). Unsigned packages are
# produced; OpenWrt installs them when the repo / --allow-untrusted is set,
# which the install-test step passes explicitly.
#
# This script is Linux-only (there is no apk-tools for macOS); it is invoked
# from the CI Linux runner, not from local macOS testing.
set -euo pipefail

STAGE="$1"; NAME="$2"; VER="$3"; ARCH="$4"; DESC="$5"; OUT="$6"
DEPENDS="${7:-}"; POSTINST="${8:-}"

if ! command -v apk >/dev/null 2>&1; then
	echo "apk-tools not found; cannot build .apk" >&2
	exit 3
fi

args=(mkpkg
	--info "name:$NAME"
	--info "version:$VER"
	--info "arch:$ARCH"
	--info "description:$DESC"
	--info "license:GPL-3.0-only"
	--info "url:https://github.com/c2h2/luci-app-photondns"
)

# dependencies (space-separated -> repeated --info depends:)
if [ -n "$DEPENDS" ]; then
	for d in $DEPENDS; do
		args+=(--info "depends:$d")
	done
fi

# post-install trigger script
if [ -n "$POSTINST" ] && [ -f "$POSTINST" ]; then
	args+=(--script "post-install:$POSTINST")
fi

# the staged tree becomes the package files
args+=(--files "$STAGE")

apk "${args[@]}" --output "$OUT"
echo "built $OUT"
