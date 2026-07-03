#!/bin/sh
# Deploy photondns + luci-app-photondns to an OpenWrt device over SSH.
# usage: ./deploy.sh [root@192.168.1.1]
set -e

HOST="${1:-root@192.168.1.1}"
SSH="ssh -i $HOME/.ssh/id_rsa"
SCP="scp -i $HOME/.ssh/id_rsa -O"
DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="$DIR/target/aarch64-unknown-linux-musl/release/photondns"
BENCH="$DIR/target/aarch64-unknown-linux-musl/release/photonbench"
APP="$DIR/openwrt/luci-app-photondns"

[ -f "$BIN" ] || { echo "binary missing - run: cargo zigbuild --release --target aarch64-unknown-linux-musl"; exit 1; }

echo "==> copying files to $HOST"
$SSH "$HOST" "mkdir -p /usr/share/rpcd/ucode /usr/share/rpcd/acl.d /usr/share/luci/menu.d /usr/share/ucitrack /www/luci-static/resources/view/photondns /etc/photondns; /etc/init.d/photondns stop 2>/dev/null; true"
$SCP "$BIN" "$HOST:/usr/bin/photondns"
[ -f "$BENCH" ] && $SCP "$BENCH" "$HOST:/usr/bin/photonbench"
$SCP "$APP/root/etc/init.d/photondns" "$HOST:/etc/init.d/photondns"
$SCP "$APP/root/usr/share/rpcd/ucode/luci.photondns" "$HOST:/usr/share/rpcd/ucode/luci.photondns"
$SCP "$APP/root/usr/share/rpcd/acl.d/luci-app-photondns.json" "$HOST:/usr/share/rpcd/acl.d/"
$SCP "$APP/root/usr/share/luci/menu.d/luci-app-photondns.json" "$HOST:/usr/share/luci/menu.d/"
$SCP "$APP/root/usr/share/ucitrack/luci-app-photondns.json" "$HOST:/usr/share/ucitrack/"
$SCP "$APP"/htdocs/luci-static/resources/view/photondns/*.js "$HOST:/www/luci-static/resources/view/photondns/"
$SCP "$APP/root/etc/uci-defaults/40_luci-photondns" "$HOST:/tmp/40_luci-photondns"

echo "==> installing"
$SSH "$HOST" sh <<'EOF'
set -e
chmod +x /usr/bin/photondns /etc/init.d/photondns /usr/share/rpcd/ucode/luci.photondns
[ -f /usr/bin/photonbench ] && chmod +x /usr/bin/photonbench
# seed uci config only if absent (preserve user settings on redeploy)
if [ ! -f /etc/config/photondns ]; then
	touch /etc/config/photondns
	NEW=1
fi
sh /tmp/40_luci-photondns && rm -f /tmp/40_luci-photondns
EOF

if $SSH "$HOST" "[ ! -s /etc/config/photondns ]"; then
	$SCP "$APP/root/etc/config/photondns" "$HOST:/etc/config/photondns"
fi

$SSH "$HOST" "/etc/init.d/rpcd restart; /etc/init.d/uhttpd restart 2>/dev/null; rm -f /tmp/luci-indexcache*; /etc/init.d/photondns enable; /etc/init.d/photondns start"
echo "==> done. service started (if enabled in uci: photondns.main.enabled=1)"
