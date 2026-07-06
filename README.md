# photondns

**English** | [简体中文](README.zh-CN.md)

High-performance DNS forwarder written in **Rust**, built for OpenWrt routers
and fast anywhere Linux runs. Ships with a full LuCI web UI
(`luci-app-photondns`).

**2.3 MB static binary · ~5 MB RSS · zero re-encode forwarding · hedged failover**

## Why it's fast

- **Zero re-encode path** — queries are forwarded as raw wire bytes (only the
  2-byte ID is patched); cached answers are byte-copied with in-place ID/TTL
  patching. No DNS message re-serialization anywhere.
- **High concurrency by design** — tokio multi-threaded runtime, sharded LRU
  cache (16 shards, `parking_lot`), lock-free per-upstream health/latency
  stats, multi-socket UDP listeners (`SO_REUSEPORT`) on Linux.
- **Multiplexed upstreams** — RFC 7766 pipelining over persistent TCP/DoT
  connections, HTTP keep-alive pool for DoH, shared UDP sockets with ID
  demultiplexing.
- **Hedged failover** — upstreams ranked by health + EWMA latency; slow
  answers are raced against the next-best upstream, circuit breaker + active
  prober take dead upstreams out of rotation. No client-visible SERVFAIL,
  even with every primary upstream dead.

## Benchmarks — can it do 100k qps? 1M?

Measured with the bundled `photonrbench` (random domains, cold pass = cache
misses through the full upstream path, warm pass = cache hits):

| platform | scenario | throughput | latency |
|---|---|---|---|
| Apple M3 Pro (12C), loopback | single UDP socket, 200k queries | **~105,000 qps** | p50 0.6 ms, p99 1.0 ms |
| Apple M3 Pro (12C), loopback | 8 sockets × 8 parallel clients | **~276,000 qps** aggregate | p50 1.5 ms |
| Ariaboard photonicat2 (4×A55 router), on-device | cached lookups | **~90,000 qps** | avg 0.35 ms |

So: **100k+ qps — yes**, on a single UDP socket, even router-class ARM
hardware gets close. **~276k qps** measured on a laptop *with the load
generator competing for the same 12 cores* — the server alone goes higher.
**1M qps is extrapolation, not yet measured**: throughput scales per-socket
(`SO_REUSEPORT` fan-out is linear until CPU saturation), so a many-core
server with dedicated load generators is projected to reach it. If you
measure it, open an issue with numbers.

Reproduce:

```sh
cargo build --release
./target/release/photondns -c config.toml            # listen 127.0.0.1:15533
./target/release/photonrbench 127.0.0.1:15533 200000 64
# photonrbench [server:port] [count] [concurrency]
# env: SUFFIX=<real-domain>  WARM=0  SEED=<n>
```

The cold pass exercises the real forwarding/failover path — point the config
at a local stub upstream unless you want to send N random domains to a
public resolver.

## Recent changes (July 2026)

- **DoH server** — new `[server] doh_listen` serves RFC 8484
  (GET `?dns=` / POST `application/dns-message`) alongside UDP/TCP.
  Two hosting modes: plain HTTP behind a TLS reverse proxy
  (Caddy: `reverse_proxy /dns-query 127.0.0.1:8054`) or native HTTPS with
  `doh_cert`/`doh_key` PEM files. UDP and TCP listeners are now individually
  toggleable (`server.udp` / `server.tcp`), all exposed in LuCI too.
- **Retry round** — when an entire first hedged round fails fast (resets,
  REFUSED), one retry runs over the full upstream set *including backups*
  within the same overall deadline. Previously `parallel` groups could
  SERVFAIL without ever trying their backups.
- **Serve-stale reliability** — fixed two leaks that could leave a cache
  entry permanently flagged as "refresh in progress" (it would then serve
  ever-older stale data until full expiry); a refresh that returns an
  uncacheable answer (e.g. TTL 0) now evicts the stale entry instead.
- **Failover tuning** — the health prober now uses the group's query timeout
  (was a hardcoded 1.5 s), so high-latency international upstreams no longer
  flap DOWN on brief spikes; default query timeout raised 2000 → 5000 ms.
- **Defaults** — ad blocking is now off by default in the packaged config.
- **Versioning** — builds are stamped `0.x.z-rN` (N = git commit count),
  reported by `-V`, `/version` and `/stats`.
- **`/resolve` API + test page** — dig-like JSON diagnostics through the real
  pipeline: route taken, winning upstream, rcode, answers, latency.
- **Standalone binaries in releases** — CI now also attaches each daemon
  build as a plain static binary (`photondns-<ver>-<arch>-linux-musl`,
  aarch64/x86_64/armv7/riscv64) — no OpenWrt needed, runs on any Linux
  distro; pairs with a plain systemd unit or `run_standalone.sh`.
- **New tooling** — `run_standalone.sh` (build & run locally with a generated
  config, no OpenWrt needed), `tools/tricky-tests.sh` (26 edge-case checks
  against a live server: 0x20 case echo, TC→TCP fallback, negative cache,
  special-TLD/PTR blocking, bursts...), `tools/compare-dns.py` (resolve N
  random Tranco domains via independent DoH references and via your server,
  store both result sets, grade mismatches with PTR-based CDN detection).

## Features

- UDP + TCP + **DoH server** (RFC 8484) listeners, each toggleable; DoH runs
  plain-HTTP behind a reverse proxy (Caddy/nginx) or native TLS with a PEM
  cert. Upstreams: `udp://`, `tcp://`, `tls://` (DoT), `https://` (DoH) with
  bootstrap resolution
- Cache: configurable size, TTL clamping, **serve-stale**, **prefetch**,
  persistence across restarts
- Failover strategies: `race` (default), `fastest`, `parallel`,
  `sequential`, `random`; adaptive hedge delay, circuit breaker, health prober
- Rules: hosts, block (NXDOMAIN), redirect, local-domain routing
- **China / non-China split DNS** — one click downloads the
  dnsmasq-china-list (~110k domains); mainland domains resolve via your local
  group, everything else via the main group
- **Ad blocking** with auto-updated lists (anti-AD, AdRules, hosts format)
- **Live query log** in LuCI (client, domain, route, upstream, latency)
- Special-TLD (`.local`/`.lan`) and private-PTR protection, optional
  HTTPS/SVCB type-65 rejection
- HTTP JSON API: `/stats`, `/flush`, `/log`, `/health`, `/version`,
  `/resolve?name=…&type=…` (dig-like diagnostics with route + upstream)
- Bilingual LuCI app (English / 简体中文): live dashboard, settings, rule
  editor, log viewer; dnsmasq takeover and firewall DNS hijack options

## Quick start

Standalone (one command — builds if needed, generates a demo config with DoT
upstreams and a plain-HTTP DoH listener on `127.0.0.1:8054`):

```sh
./run_standalone.sh                          # Ctrl-C to stop
dig @127.0.0.1 -p 15533 example.com
```

Or manually:

```sh
cargo build --release
./target/release/photondns -c config.toml    # -t to validate config
```

```toml
[server]
listen = ["0.0.0.0:15533"]
udp = true
tcp = true
doh_listen = "127.0.0.1:8054"   # "" = off; add doh_cert/doh_key for native TLS

[cache]
size = 8192
serve_stale = true

[[group]]
name = "main"
strategy = "race"
upstreams = ["udp://223.5.5.5", "udp://119.29.29.29"]
backups = ["tls://8.8.8.8"]
```

To publish the DoH endpoint for browsers, either front it with Caddy:

```
dns.example.com {
    reverse_proxy /dns-query 127.0.0.1:8054
}
```

or serve TLS natively: `doh_cert = "/path/fullchain.pem"`,
`doh_key = "/path/key.pem"`, then
`curl --doh-url https://dns.example.com/dns-query https://example.org`.

OpenWrt — prebuilt packages (recommended). Each
[release](https://github.com/c2h2/luci-app-photondns/releases) ships
`.ipk` (opkg, ≤ 23.05) and `.apk` (apk, ≥ 24.10) for aarch64, x86_64, armv7,
and riscv64, plus the two arch-independent LuCI apps:

```sh
# opkg (OpenWrt ≤ 23.05) — pick your arch
opkg install photondns_*_aarch64_generic.ipk luci-app-photondns_*_all.ipk
# apk (OpenWrt ≥ 24.10)
apk add --allow-untrusted photondns_*_aarch64_generic.apk luci-app-photondns_*_all.apk
uci set photondns.main.enabled=1; uci commit photondns; /etc/init.d/photondns restart
```

OpenWrt — build + deploy from source over SSH:

```sh
cargo zigbuild --release --target aarch64-unknown-linux-musl
./deploy.sh root@192.168.1.1
ssh root@192.168.1.1 'uci set photondns.main.enabled=1; uci commit photondns; /etc/init.d/photondns restart'
dig @192.168.1.1 -p 15533 example.com
```

Then open LuCI → Services → photondns. Enable *DNS Forward* to make it the
system resolver (dnsmasq settings are backed up and restored).

## Repository layout

```
src/                       Rust sources (server, cache, upstreams, failover, router, API)
src/bin/photonbench.rs     single-domain UDP load generator
src/bin/photonrbench.rs    randomized parallel benchmark
openwrt/photondns/         OpenWrt package Makefile (SDK build)
openwrt/luci-app-photondns/         LuCI app (JS + ucode rpcd, UCI schema, procd init)
openwrt/luci-app-photondns-compat/  legacy Lua/CBI LuCI app for old firmware
deploy.sh                  direct-to-device deployment over SSH
run_standalone.sh          build & run locally with a generated config
tools/tricky-tests.sh      edge-case battery against a live server
tools/compare-dns.py       cross-check answers vs independent DoH references
```

## License

GPL-3.0-only.
