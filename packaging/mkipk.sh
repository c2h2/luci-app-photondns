#!/usr/bin/env bash
# Build an OpenWrt .ipk from a staged package tree.
#
# usage: mkipk.sh <staging_dir> <control_file> <out.ipk> [conffiles] [postinst] [prerm]
#
#   staging_dir  directory whose contents become the package data (the future /)
#   control_file path to a ready control file (contains Package/Version/Arch/...)
#   out.ipk      output path
#   conffiles    optional file listing config paths to preserve on upgrade
#   postinst     optional postinstall script
#   prerm        optional preremove script
#
# An .ipk is an `ar` archive of: debian-binary, control.tar.gz, data.tar.gz.
# We build it reproducibly (sorted, fixed mtime, root:root owner) so identical
# inputs yield identical bytes.
set -euo pipefail

STAGE="$1"; CONTROL="$2"; OUT="$3"
CONFFILES="${4:-}"; POSTINST="${5:-}"; PRERM="${6:-}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

MTIME="${SOURCE_DATE_EPOCH:-0}"
# BusyBox/GNU tar differ; detect GNU-style flags for reproducibility.
if tar --version 2>/dev/null | grep -qi 'gnu'; then
	TAR_REPRO=(--sort=name --owner=0 --group=0 --numeric-owner --mtime="@$MTIME")
else
	# bsdtar (macOS): no --sort; emulate with find|sort, fixed owner/mtime below.
	TAR_REPRO=(--uid 0 --gid 0)
fi

pack_tar() {
	# pack_tar <srcdir> <out.tar.gz>
	local src="$1" out="$2"
	if tar --version 2>/dev/null | grep -qi 'gnu'; then
		( cd "$src" && find . | LC_ALL=C sort > "$WORK/list" )
		tar "${TAR_REPRO[@]}" --no-recursion -C "$src" -czf "$out" -T "$WORK/list"
	else
		# bsdtar (macOS): --no-recursion + a fully sorted file list avoids the
		# default behaviour of re-expanding each listed directory.
		( cd "$src" && find . | LC_ALL=C sort > "$WORK/list" )
		COPYFILE_DISABLE=1 tar "${TAR_REPRO[@]}" --no-recursion -C "$src" -czf "$out" -T "$WORK/list"
	fi
}

# ---- data.tar.gz : the actual files that land on the device ----
pack_tar "$STAGE" "$WORK/data.tar.gz"

# ---- control.tar.gz : metadata + maintainer scripts ----
CTRL="$WORK/control"
mkdir -p "$CTRL"
cp "$CONTROL" "$CTRL/control"
# ensure control ends with a newline (opkg is picky)
[ -z "$(tail -c1 "$CTRL/control")" ] || printf '\n' >> "$CTRL/control"
[ -n "$CONFFILES" ] && [ -f "$CONFFILES" ] && cp "$CONFFILES" "$CTRL/conffiles"
if [ -n "$POSTINST" ] && [ -f "$POSTINST" ]; then cp "$POSTINST" "$CTRL/postinst"; chmod 755 "$CTRL/postinst"; fi
if [ -n "$PRERM" ] && [ -f "$PRERM" ]; then cp "$PRERM" "$CTRL/prerm"; chmod 755 "$CTRL/prerm"; fi
pack_tar "$CTRL" "$WORK/control.tar.gz"

# ---- debian-binary ----
printf '2.0\n' > "$WORK/debian-binary"

# ---- assemble the ar archive (order matters for opkg) ----
rm -f "$OUT"
( cd "$WORK" && ar rc "$OUT.tmp" debian-binary control.tar.gz data.tar.gz )
mv "$WORK/$(basename "$OUT").tmp" "$OUT" 2>/dev/null || mv "$OUT.tmp" "$OUT"

echo "built $OUT"
