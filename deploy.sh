#!/bin/sh
# Deploy photondns + LuCI app to an OpenWrt device over SSH.
# usage: ./deploy.sh [--reset] [root@192.168.1.1] [target-triple] [luci-flavor]
#   --reset  full-wipe photondns state on the device BEFORE deploying: UCI config,
#            all rule files, downloaded china/ad lists, cache dump and log. The
#            packaged defaults are then re-seeded, giving a fresh-install state.
#   target-triple defaults to aarch64-unknown-linux-musl (photonicat2);
#     use x86_64-unknown-linux-musl for x86/64 devices (e.g. the 192.168.1.4 gateway).
#   luci-flavor: auto (default) | modern | compat | none
#     modern = JS client + ucode rpcd app (luci-app-photondns)
#     compat = legacy Lua/CBI app for old LuCI without the JS client (luci-app-photondns-compat)
#     auto   = probe the device and pick modern if /www/luci-static/resources/rpc.js exists
set -e

RESET=0
if [ "$1" = "--reset" ]; then
	RESET=1
	shift
fi

HOST="${1:-root@192.168.1.1}"
TARGET="${2:-aarch64-unknown-linux-musl}"
FLAVOR="${3:-auto}"
SSH="ssh -i $HOME/.ssh/id_rsa"
SCP="scp -i $HOME/.ssh/id_rsa -O"
DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="$DIR/target/$TARGET/release/photondns"
BENCH="$DIR/target/$TARGET/release/photonbench"
RBENCH="$DIR/target/$TARGET/release/photonrbench"
APP="$DIR/openwrt/luci-app-photondns"
CAPP="$DIR/openwrt/luci-app-photondns-compat"

[ -f "$BIN" ] || { echo "binary missing - run: cargo zigbuild --release --target $TARGET"; exit 1; }
echo "==> deploying $TARGET build to $HOST"

# ---- decide LuCI flavor ----
if [ "$FLAVOR" = "auto" ]; then
	if $SSH "$HOST" "[ -f /www/luci-static/resources/rpc.js ]"; then
		FLAVOR="modern"
	else
		FLAVOR="compat"
	fi
	echo "==> auto-detected LuCI flavor: $FLAVOR"
fi

# ---- common: binary, init, shell scripts, config/rule seed ----
echo "==> copying core files to $HOST"
$SSH "$HOST" "mkdir -p /usr/share/rpcd/acl.d /etc/photondns; /etc/init.d/photondns stop 2>/dev/null; true"

# ---- optional: full-wipe existing state so defaults get re-seeded ----
if [ "$RESET" = "1" ]; then
	echo "==> --reset: wiping all photondns settings/state on $HOST"
	$SSH "$HOST" sh <<'EOF'
# service is already stopped above; drop cron + fw4 hijack chain it installed
[ -x /etc/init.d/photondns ] && /etc/init.d/photondns disable 2>/dev/null
[ -x /sbin/fw4 ] && nft delete table inet photondns 2>/dev/null
rm -f /etc/crontabs/root.photondns 2>/dev/null
sed -i '/photondns/d' /etc/crontabs/root 2>/dev/null
# uci config + generated runtime toml + log
rm -f /etc/config/photondns /var/etc/photondns.toml /var/log/photondns.log
# rule files, downloaded lists, cache dump, locks
rm -f /etc/photondns/hosts.txt /etc/photondns/block.txt \
	/etc/photondns/local_domains.txt /etc/photondns/redirect.txt \
	/etc/photondns/china_list.txt /etc/photondns/ad_list.txt \
	/etc/photondns/cache.dump /etc/photondns/redirect.lock
true
EOF
fi
$SCP "$BIN" "$HOST:/usr/bin/photondns"
[ -f "$BENCH" ] && $SCP "$BENCH" "$HOST:/usr/bin/photonbench"
[ -f "$RBENCH" ] && $SCP "$RBENCH" "$HOST:/usr/bin/photonrbench"
$SCP "$APP/root/etc/init.d/photondns" "$HOST:/etc/init.d/photondns"
$SCP "$APP/root/usr/bin/photondns-chinalist" "$HOST:/usr/bin/photondns-chinalist"
$SCP "$APP/root/usr/bin/photondns-adlist" "$HOST:/usr/bin/photondns-adlist"
$SCP "$APP/root/etc/uci-defaults/40_luci-photondns" "$HOST:/tmp/40_luci-photondns"

# ---- flavor-specific LuCI frontend ----
if [ "$FLAVOR" = "modern" ]; then
	echo "==> installing modern (JS + ucode) LuCI app"
	$SSH "$HOST" "mkdir -p /usr/share/rpcd/ucode /usr/share/luci/menu.d /usr/share/ucitrack /www/luci-static/resources/view/photondns"
	$SCP "$APP/root/usr/share/rpcd/ucode/luci.photondns" "$HOST:/usr/share/rpcd/ucode/luci.photondns"
	$SCP "$APP/root/usr/share/rpcd/acl.d/luci-app-photondns.json" "$HOST:/usr/share/rpcd/acl.d/"
	$SCP "$APP/root/usr/share/luci/menu.d/luci-app-photondns.json" "$HOST:/usr/share/luci/menu.d/"
	$SCP "$APP/root/usr/share/ucitrack/luci-app-photondns.json" "$HOST:/usr/share/ucitrack/"
	$SCP "$APP"/htdocs/luci-static/resources/view/photondns/*.js "$HOST:/www/luci-static/resources/view/photondns/"
	I18N_PO="$APP/po/zh_Hans/photondns.po"
elif [ "$FLAVOR" = "compat" ]; then
	echo "==> installing compat (legacy Lua/CBI) LuCI app"
	# remove any stale modern-app files so LuCI doesn't error on the dead JS menu
	$SSH "$HOST" "rm -f /usr/share/luci/menu.d/luci-app-photondns.json /usr/share/rpcd/ucode/luci.photondns /usr/share/rpcd/acl.d/luci-app-photondns.json; rm -rf /www/luci-static/resources/view/photondns"
	$SSH "$HOST" "mkdir -p /usr/lib/lua/luci/controller /usr/lib/lua/luci/model/cbi/photondns /usr/lib/lua/luci/view/photondns"
	$SCP "$CAPP/root/usr/lib/lua/luci/controller/photondns.lua" "$HOST:/usr/lib/lua/luci/controller/photondns.lua"
	$SCP "$CAPP"/root/usr/lib/lua/luci/model/cbi/photondns/*.lua "$HOST:/usr/lib/lua/luci/model/cbi/photondns/"
	$SCP "$CAPP"/root/usr/lib/lua/luci/view/photondns/*.htm "$HOST:/usr/lib/lua/luci/view/photondns/"
	$SCP "$CAPP/root/usr/share/rpcd/acl.d/luci-app-photondns-compat.json" "$HOST:/usr/share/rpcd/acl.d/"
	I18N_PO="$CAPP/po/zh_Hans/photondns.po"
	[ -f "$I18N_PO" ] || I18N_PO="$APP/po/zh_Hans/photondns.po"
else
	echo "==> flavor=none: service only, no LuCI frontend"
	I18N_PO=""
fi

# ---- i18n: compile po -> lmo and install (zh-cn) ----
if [ -n "$I18N_PO" ] && command -v python3 >/dev/null && [ -f "$I18N_PO" ]; then
	python3 "$DIR/tools/po2lmo.py" "$I18N_PO" /tmp/photondns.zh-cn.lmo
	$SSH "$HOST" "mkdir -p /usr/lib/lua/luci/i18n"
	$SCP /tmp/photondns.zh-cn.lmo "$HOST:/usr/lib/lua/luci/i18n/photondns.zh-cn.lmo"
	rm -f /tmp/photondns.zh-cn.lmo
fi

echo "==> installing"
$SSH "$HOST" sh <<'EOF'
set -e
chmod +x /usr/bin/photondns /usr/bin/photondns-chinalist /usr/bin/photondns-adlist /etc/init.d/photondns
[ -f /usr/share/rpcd/ucode/luci.photondns ] && chmod +x /usr/share/rpcd/ucode/luci.photondns
[ -f /usr/bin/photonbench ] && chmod +x /usr/bin/photonbench
[ -f /usr/bin/photonrbench ] && chmod +x /usr/bin/photonrbench
# seed uci config only if absent (preserve user settings on redeploy)
if [ ! -f /etc/config/photondns ]; then
	touch /etc/config/photondns
fi
sh /tmp/40_luci-photondns && rm -f /tmp/40_luci-photondns
EOF

if $SSH "$HOST" "[ ! -s /etc/config/photondns ]"; then
	$SCP "$APP/root/etc/config/photondns" "$HOST:/etc/config/photondns"
fi

$SSH "$HOST" "/etc/init.d/rpcd restart; /etc/init.d/uhttpd restart 2>/dev/null; rm -f /tmp/luci-indexcache* /tmp/luci-modulecache/* 2>/dev/null; /etc/init.d/photondns enable; /etc/init.d/photondns start"
echo "==> done ($FLAVOR). service started (if enabled in uci: photondns.main.enabled=1)"
