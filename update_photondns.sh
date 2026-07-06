#!/bin/sh
# update_photondns.sh - download photondns from GitHub, build, deploy.
# For plain Linux servers (systemd), no OpenWrt. Copy to /root and run as
# root; run it again any time to update to latest GitHub main. Idempotent.
#
#   - clones/updates https://github.com/c2h2/luci-app-photondns (branch main)
#   - cargo release build (runs as $BUILD_USER, needs rustup toolchain there)
#   - installs /usr/local/bin/photondns
#   - writes default /etc/photondns/config.toml (only if missing)
#   - installs + enables systemd service (autostart on boot)
#   - frees port 53 from the systemd-resolved stub listener (once)
#
# env overrides:  REPO  BRANCH  BUILD_USER  SRC
set -eu

REPO="${REPO:-https://github.com/c2h2/luci-app-photondns.git}"
BRANCH="${BRANCH:-main}"
SRC="${SRC:-/opt/photondns/src}"
BUILD_USER="${BUILD_USER:-c2h2}"
BIN="/usr/local/bin/photondns"
CONF="/etc/photondns/config.toml"
UNIT="/etc/systemd/system/photondns.service"

[ "$(id -u)" = 0 ] || { echo "error: run as root" >&2; exit 1; }

echo "==> fetching $REPO ($BRANCH)"
install -d -o "$BUILD_USER" -g "$BUILD_USER" "$(dirname "$SRC")"
if [ ! -d "$SRC/.git" ]; then
	sudo -u "$BUILD_USER" -H git clone --branch "$BRANCH" "$REPO" "$SRC"
else
	sudo -u "$BUILD_USER" -H git -C "$SRC" fetch origin "$BRANCH"
	sudo -u "$BUILD_USER" -H git -C "$SRC" reset --hard "origin/$BRANCH"
fi
echo "==> HEAD: $(git -C "$SRC" log -1 --oneline)"

echo "==> building (cargo release, this can take a few minutes)"
sudo -u "$BUILD_USER" -H sh -c "cd '$SRC' && PATH=\"\$HOME/.cargo/bin:\$PATH\" cargo build --release --bin photondns"

echo "==> installing $BIN"
install -m 0755 "$SRC/target/release/photondns" "$BIN.new"
mv -f "$BIN.new" "$BIN"

echo "==> service user + dirs"
id photondns >/dev/null 2>&1 || \
	useradd --system --home /var/lib/photondns --shell /usr/sbin/nologin photondns
install -d -o photondns -g photondns /var/lib/photondns
install -d /etc/photondns

if [ ! -f "$CONF" ]; then
	echo "==> writing default $CONF"
	cat > "$CONF" <<'EOF'
# photondns - public resolver: udp/tcp :53 + DoH.
# DoH is plain http on 127.0.0.1:8054 - terminate TLS in front of it, e.g.
# Caddy:  reverse_proxy /dns-query 127.0.0.1:8054

[server]
listen = ["0.0.0.0:53"]
udp = true
tcp = true
doh_listen = "127.0.0.1:8054"

[cache]
dump_file = "/var/lib/photondns/cache.dump"

[api]
listen = "127.0.0.1:8053"

[log]
level = "info"

[[group]]
name = "main"
upstreams = ["tls://1.1.1.1", "tls://8.8.8.8", "tls://9.9.9.9"]
EOF
fi

# systemd-resolved's stub only binds 127.0.0.53/.54 but keep it off so
# nothing fights over port 53; host DNS goes straight to real upstreams.
if systemctl is-active --quiet systemd-resolved && \
   [ ! -f /etc/systemd/resolved.conf.d/photondns.conf ]; then
	echo "==> disabling systemd-resolved stub listener"
	mkdir -p /etc/systemd/resolved.conf.d
	printf '[Resolve]\nDNSStubListener=no\n' > /etc/systemd/resolved.conf.d/photondns.conf
	ln -sf /run/systemd/resolve/resolv.conf /etc/resolv.conf
	systemctl restart systemd-resolved
fi

echo "==> installing systemd service (autostart)"
cat > "$UNIT" <<'EOF'
[Unit]
Description=photondns DNS forwarder
Documentation=https://github.com/c2h2/luci-app-photondns
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/photondns -c /etc/photondns/config.toml
User=photondns
Group=photondns
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=true
Restart=always
RestartSec=2
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable photondns >/dev/null 2>&1 || true

echo "==> validating config"
"$BIN" -t -c "$CONF"

echo "==> restarting photondns"
systemctl restart photondns
sleep 1
systemctl is-active photondns

echo "==> smoke test @127.0.0.1"
dig +time=3 +tries=1 @127.0.0.1 github.com +short | head -3
dig +time=3 +tries=1 +tcp @127.0.0.1 cloudflare.com +short | head -3
echo "==> photondns update done: $(git -C "$SRC" log -1 --format=%h) -> $BIN"
