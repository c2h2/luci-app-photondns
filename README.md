# photondns

**English** | [简体中文](README.zh-CN.md)

High-performance DNS forwarder for OpenWrt, written in Rust. A from-scratch
reimagining of [mosdns](https://github.com/sbwml/luci-app-mosdns) focused on
raw speed and **failover that never makes a client wait**, with a full LuCI
web UI (`luci-app-photondns`).

Single static binary, **2.3 MB**, **~5 MB RSS** at runtime.

## Measured performance (Ariaboard photonicat2, rockchip aarch64)

| metric | result |
|---|---|
| cached lookups, on-device | **~90,000 qps**, avg 0.35 ms |
| cache hit latency | 0 ms (dig), TTLs age correctly |
| cold-start query with *all* primary upstreams dead | answered in **285 ms** via hedged backup |
| steady-state with dead primaries (breaker open) | **12 ms** |
| memory | 5.2 MB RSS |

## Why it's fast

- **Zero re-encode forwarding**: queries are forwarded as raw wire bytes (only
  the 2-byte ID is patched); cached responses are byte-copied with in-place
  ID/TTL/question patching. No DNS message re-serialization anywhere.
- **Sharded LRU cache** (16 shards, `parking_lot` mutexes) with configurable
  size; single-digit-µs lookups under concurrency.
- **Multiplexed upstream connections**: TCP and DoT use RFC 7766 query
  pipelining over persistent connections; DoH uses an HTTP/1.1 keep-alive
  pool; UDP uses shared sockets with ID demultiplexing.
- Multi-socket UDP listeners (`SO_REUSEPORT`) on Linux.

## Failover ("blazing fast" by design)

Every query runs through a *hedged execution engine*:

1. Upstreams are ranked by health + EWMA latency (per-upstream, lock-free).
2. The best upstream is asked first. If no answer arrives within the **adaptive
   hedge delay** (~2× the best upstream's EWMA, capped by `hedge_delay_ms`),
   the next-best upstream is raced *in parallel*; first good answer wins.
3. Any hard failure triggers the next candidate immediately.
4. **Backup upstreams ride along** at the end of the candidate list, so even a
   cold-start query with every primary dead resolves in one hedge interval —
   no SERVFAIL, no timeout.
5. A **circuit breaker** (N consecutive failures → down, cooldown → half-open
   → M successes → restored) takes dead upstreams out of rotation; an **active
   health prober** keeps latency stats fresh, detects dead upstreams while
   idle, and keeps TLS connections warm.
6. If an upstream's UDP answer is truncated, it is retried over TCP
   automatically (method fallback).
7. If *everything* fails, expired cache entries are served as a last resort.

Strategies: `race` (default), `fastest`, `parallel`, `sequential`, `random`.

## Feature parity with luci-app-mosdns, and then some

- UDP + TCP listeners, configurable listen address/port
- Upstreams: `udp://`, `tcp://`, `tls://` (DoT), `https://` (DoH), with
  bootstrap resolution of DoT/DoH hostnames and `insecure_skip_verify`
- **Configurable cache size**, min/max TTL clamping, negative-answer TTL
- **Serve-stale** (lazy cache) + **prefetch** of popular entries before expiry
- **Cache persistence** across restarts (periodic + on shutdown)
- Rule files: hosts, block list (NXDOMAIN), redirect, local-domain routing to
  a separate "local" upstream group
- **China / non-China split DNS**: one click downloads the
  dnsmasq-china-list (~110k domains, CN-friendly mirrors); mainland domains
  resolve via your Local-domain DNS group, everything else via the primary
  group — per-group query counters in `/stats` show the split live
- Reject HTTPS/SVCB type-65 queries (optional)
- Built-in protection: `.local`/`.lan`/RFC-6761 special TLDs and private PTR
  zones are answered NXDOMAIN locally instead of leaking upstream
- **Ad blocking**: auto-downloaded lists (anti-AD, Cats-Team AdRules, hosts
  files...) answered NXDOMAIN, with a LuCI update page and status
- **Live query log** in LuCI: last N queries (default 5000, in-memory) with
  client, domain, route taken (cache/stale/hosts/blocked/local/main),
  winning upstream and latency, filterable and auto-refreshing
- **Scheduled auto-update** (cron) for the China and ad lists
- dnsmasq takeover (`redirect`) and firewall DNS hijack (`dns_hijack`) options
- HTTP JSON API: `/stats`, `/flush`, `/log`, `/health`, `/version` (127.0.0.1)
- LuCI app: live status dashboard (upstream health, EWMA latency, hedges,
  cache hit rate), full settings editor, rule file editor, log viewer —
  bilingual (English / 简体中文)

## Repository layout

```
src/                    Rust sources (server, cache, upstreams, failover, router, API)
src/bin/photonbench.rs    single-domain UDP DNS load generator
src/bin/photonrbench.rs   randomized parallel benchmark (1000 fresh domains/run)
openwrt/photondns/        OpenWrt package Makefile (SDK build)
openwrt/luci-app-photondns/  LuCI app: views, rpcd ucode backend, ACL, menu,
                        UCI schema + procd init that generates the TOML config,
                        po translations (zh_Hans)
tools/po2lmo.py         po -> lmo compiler for direct deployments
test_dns.sh             verbose dig/nslookup test with timing + route info
deploy.sh               direct-to-device deployment over SSH
```

## Benchmarking

`photonrbench` generates a fresh set of random domains each run (so the cold
pass is all cache-misses, exercising the real upstream/failover path), fires
them through a parallel worker pool, then re-queries the same set warm to
measure raw cache-serving speed. Reports throughput and p50/p90/p99 latency.

```sh
photonrbench [server:port] [count] [concurrency]   # defaults 127.0.0.1:15533 1000 50
#   env: SUFFIX=<domain>  (append a real suffix)   WARM=0  (skip warm phase)
#        SEED=<n>         (reproducible domain set for A/B comparisons)
```

On the photonicat2 (loopback, no network overhead) a warm run does
**~55,000 qps at p50 0.5 ms / p99 3 ms**; the cold pass is WAN-bound by the
upstream RTT, which is the point — it measures the forwarder, not a loop.

## Build

```sh
cargo test                                              # unit tests
cargo build --release                                   # host build
cargo zigbuild --release --target aarch64-unknown-linux-musl   # OpenWrt aarch64
```

## Deploy to a device

```sh
./deploy.sh root@192.168.1.1
ssh root@192.168.1.1 'uci set photondns.main.enabled=1; uci commit photondns; /etc/init.d/photondns restart'
dig @192.168.1.1 -p 15533 example.com
```

Then open LuCI → Services → photondns. To make it the system resolver, enable
*DNS Forward* (and optionally *DNS Redirect*) in Basic Settings — this
reconfigures dnsmasq to forward to photondns (original settings are backed up
and restored when disabled).

## Configuration

`/etc/config/photondns` (UCI) is the source of truth; the init script generates
`/var/etc/photondns.toml`. The daemon can also be run standalone with a
hand-written TOML file (`photondns -c config.toml`, `-t` to validate):

```toml
[server]
listen = ["0.0.0.0:15533"]

[cache]
size = 8192          # entries (the headline knob)
serve_stale = true
prefetch = true
dump_file = "/etc/photondns/cache.dump"

[failover]
health_check_interval = 10
fail_threshold = 3
cooldown = 15

[[group]]
name = "main"
strategy = "race"
upstreams = ["udp://223.5.5.5", "udp://119.29.29.29"]
backups = ["tls://8.8.8.8"]
hedge_delay_ms = 250
timeout_ms = 2000
```

## License

GPL-3.0-only.
