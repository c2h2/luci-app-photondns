# photondns optimization + re-benchmark vs mosdns

Goal: make photondns faster and re-compare against mosdns. Work spanned two
hosts — a macOS arm64 desktop (the original bench box) and a 24-core Ubuntu
24.04 x86_64 Linux VM (`192.168.0.62`, the deployment-representative target).
Network is removed throughout (both resolvers forward to an instant loopback
stub), so numbers reflect resolver cost, not WAN.

## TL;DR

- **photondns is ~4–5× faster than mosdns on the Linux forwarding path** (and up
  to ~9× when mosdns hits its ~1 s forward-stall spikes). This is the headline
  re-bench result.
- **Per-query CPU was cut ~2×** on the router's hottest work (FxHash vs SipHash),
  plus fewer allocations and no double response-copy, and an optional mimalloc
  allocator. These land on the weak-CPU OpenWrt target; on fast desktop/Linux
  they're invisible because that path isn't CPU-bound.
- **recvmmsg/sendmmsg batching was implemented, verified correct, and measured:
  ~1.0× (no throughput gain) on Linux.** recvmmsg does batch (~6 datagrams/call),
  but client-facing syscalls are only ~30% of the total and Linux syscalls are
  cheap — the unbatched upstream forward dominates. Kept as an opt-in
  (`PHOTONDNS_UDP_BATCH=1`), default off.

## Why the original "2× on the macOS bench" wasn't reachable

On the macOS loopback forward bench, photondns uses only ~3 of 10 cores flat-out
— it's **syscall/scheduling-bound, not CPU-bound**. The bare stub does 125k QPS
with 2 syscalls/query; a forwarder does ~4 syscalls/query → a ~60–70k floor that
photondns already sat at (66–72k). No pure-code change moves that number there.
The bottleneck is the platform, not the code.

## Changes made (portable, all targets)

| change | file | effect |
|---|---|---|
| FxHash instead of SipHash (router domain-sets + cache shard/LRU) | `router.rs`, `cache.rs` | hottest per-query op **2.1× / 1.7×** cheaper (below) |
| `finish()` takes `Vec` by value | `server.rs` | one fewer full-packet copy on every answer |
| allocation-free EDNS scan in `parse_query` | `dns.rs` | drops a per-query throwaway `Vec` |
| optional mimalloc (`--features fastalloc`) | `Cargo.toml`, `main.rs` | large win on musl/OpenWrt; opt-in so default cross-build stays pure-Rust |
| opt-in recvmmsg/sendmmsg batching (Linux) | `server.rs` | see measurement below; default off |

### CPU microbenchmark (proves the FxHash win — `src/bin/hotbench.rs`)

Real china list (111,712 entries), real query names:

| scenario | SipHash | FxHash | speedup |
|---|--:|--:|--:|
| foreign_miss (full label walk) | 45.0 ns/query | 21.4 ns/query | **2.10×** |
| china_miss (early-exit match) | 35.7 ns/query | 20.8 ns/query | **1.72×** |

## Linux re-benchmark vs mosdns

Host: 24-core Ubuntu 24.04 QEMU VM. mosdns v5.3.4-0-gb732318 (same build as the
macOS bench). Hardened loopback stub. Flat-out, cold cache, median QPS.

**Generator ceiling (dnsperf → stub direct): ~560,000 QPS** — well above every
resolver number below, so these are resolver-bound, not generator-bound.

Representative same-session run (`REP=5 SECS=6`):

| scenario | photondns | mosdns | photondns / mosdns |
|---|--:|--:|--:|
| foreign_miss | 328k–357k | 74k | **4.5–4.8×** |
| china_miss | 300k–326k | 77k | **3.9–4.2×** |

mosdns is also far less stable: a second run measured it at 38k on foreign_miss
(its ~1 s forward-stall behavior), pushing the ratio to ~9×. photondns stayed
300k+ across every run.

### recvmmsg/sendmmsg batching — measured, no gain

Sequential-phase runs *appeared* to show 1.1–1.7×, but that was load drift
between phases on a shared VM. A **CPU-pinned, interleaved A/B** (batch and
per-packet alternating each rep, so both see the same machine load) settled it:

```
foreign_miss  median  per-packet=211,705  batched=215,478  →  1.02×
```

`strace -c` (batched, 3 s) shows why: recvmmsg pulls ~6 datagrams/call (batching
works), but the syscall mix is dominated by the **unbatched upstream forward**:

| syscall | share | note |
|---|--:|---|
| recvfrom + sendto (upstream → stub) | ~70% | one pair per query, not batched |
| recvmmsg + sendmmsg (client side) | ~30% | batched, but the minority |

Batching only the client socket can't move a total dominated by the upstream
path, and Linux syscalls are cheap regardless. To actually cut forwarding
syscalls you'd batch the upstream demux too — a much larger change against the
async oneshot-demux transport, and still low-value since photondns already
outruns mosdns 4–9× and a home router never approaches these rates.

## Recommendation

- Ship the CPU wins (FxHash, copy/alloc reductions) — pure upside, especially on
  OpenWrt. Consider enabling `fastalloc` where a C cross-toolchain is available.
- Leave recvmmsg batching opt-in/off: correct but no measured benefit, and it
  carries unsafe FFI. Revisit only if profiling a real overloaded deployment.

## Reproduce (Linux box)

```
bench/linbench.sh    # 3-way: per-packet vs batched vs mosdns, flat-out
bench/abbatch.sh     # interleaved + CPU-pinned batch-vs-nobatch A/B (SCEN=foreign_miss)
bench/ceil.sh        # dnsperf -> stub ceiling
bench/syscount.sh    # strace -c syscall breakdown (needs ptrace_scope=0)
target/release/hotbench bench/lists/china_list.txt bench/queries/foreign_miss_2m.txt   # FxHash vs SipHash
```
