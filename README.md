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

## Features

- UDP + TCP listeners; upstreams: `udp://`, `tcp://`, `tls://` (DoT),
  `https://` (DoH) with bootstrap resolution
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
- HTTP JSON API: `/stats`, `/flush`, `/log`, `/health`, `/version`
- Bilingual LuCI app (English / 简体中文): live dashboard, settings, rule
  editor, log viewer; dnsmasq takeover and firewall DNS hijack options

## Quick start

Standalone:

```sh
cargo build --release
./target/release/photondns -c config.toml    # -t to validate config
```

```toml
[server]
listen = ["0.0.0.0:15533"]

[cache]
size = 8192
serve_stale = true

[[group]]
name = "main"
strategy = "race"
upstreams = ["udp://223.5.5.5", "udp://119.29.29.29"]
backups = ["tls://8.8.8.8"]
```

OpenWrt — prebuilt packages (recommended). Each
[release](https://github.com/c2h2/luci-app-photondns/releases) ships
`.ipk` (opkg, ≤ 23.05) and `.apk` (apk, ≥ 24.10) for aarch64, x86_64, armv7,
riscv64 (and best-effort mips/mipsel), plus the two arch-independent LuCI apps:

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
```

## License

GPL-3.0-only.
