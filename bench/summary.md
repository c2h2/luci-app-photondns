# photondns vs mosdns — local DNS benchmark (network delay removed)

**Host:** Apple Mac17,3, 10 cores (4P+6E), macOS 26.4.1 · **Resolvers:** photondns (Rust, release) vs mosdns v5.3.4 · **Generator:** dnsperf · **Date:** 2026-07-08

## Method

Both resolvers forward every miss to an **instant loopback stub** (`udp://127.0.0.1:5300`) that answers in microseconds, so upstream/WAN RTT ≈ 0 and each query's time is the resolver's *own* processing (route match + cache + response build), not the network. The comparison is equalized: identical lists, plain LRU cache (65536), 8 worker threads each (mosdns `GOMAXPROCS=8`; photondns auto-caps `min(cores,8)=8`), same query files, same dnsperf flags. mosdns is configured **match-first** (ad-match + china-match run *before* the cache via `goto`) to mirror photondns running `router.decide()` before the cache on every query, including hits. Only one resolver is under load at a time; figures are medians of repeated runs.

Two upstream stubs were used. The **plain** stub is a single UDP socket with default buffers; under flat-out loopback load a single socket can itself queue/drop, which throttles the resolver behind it. The **hardened** stub (`stubdns_hard.go`: one `SO_REUSEPORT` socket per core, 8 MB `SO_RCVBUF`/`SO_SNDBUF`) is drop-proof and is the **fair, definitive upstream** — the forwarding-path numbers below come from it.

### Data provenance

| Input | File | Size |
|---|---|--:|
| Ad/tracker blocklist (→ NXDOMAIN) | `lists/ad_list.txt` | 105,793 domains |
| China routing list | `lists/china_list.txt` | 111,712 domains |
| Cache-hit queries | `queries/hit_10k.txt` | 10,000 |
| Ad-block queries | `queries/ad_block_50k.txt` | 50,000 |
| Foreign miss (forward) | `queries/foreign_miss_2m.txt` | 2,000,000 |
| China miss (forward) | `queries/china_miss_2m.txt` | 2,000,000 |
| Mixed steady-state | `queries/mixed.txt` | 100,000 |

## Headline

- **Cache-hit and ad-block are a tie** — flat-out they sit at the dnsperf→stub ceiling (~125k QPS), so throughput cannot differentiate them, and clean per-query latency is within noise.
- **The forwarding (miss) paths are the real differentiator.** photondns delivers modestly higher throughput (**1.06–1.13×**) and, more importantly, **dramatically tighter tail latency**: photondns worst-case ≈ 8–17 ms with **zero** queries over 500 ms, while mosdns spikes to **~1–2 s** on hundreds of queries.
- Those ~1 s mosdns spikes are **mosdns-side, not an artifact of our stub** — they persist unchanged against the drop-proof hardened stub (see root-cause section).

## 1 · Generator-bound scenarios (cache-hit, ad-block) — effectively a tie

Flat-out these are limited by the dnsperf+loopback ceiling (~125,756 QPS median), not by the resolver, so equal numbers here mean "both faster than the harness can measure," not "identically fast."

| scenario | pd QPS | md QPS | pd fixed-load avg | md fixed-load avg | note |
|---|--:|--:|--:|--:|---|
| cache-hit | 107,826 | 96,392 | 0.115 ms | 0.129 ms | both near ceiling → tie |
| ad-block NXDOMAIN | 72,932 | 73,755 | 0.147 ms | 0.129 ms | tie (md marginally faster on clean latency) |

## 2 · Forwarding paths (foreign_miss, china_miss) — resolver-bound, definitive (hardened stub)

**Flat-out throughput** (median of 6 cold runs):

| scenario | pd QPS | md QPS | pd/md | pd max | md max |
|---|--:|--:|--:|--:|--:|
| foreign miss | 71,292 | 63,251 | **1.13×** | 8.7 ms | 2000.2 ms |
| china miss | 49,538 | 46,870 | **1.06×** | 8.0 ms | 2001.8 ms |

**Clean per-query latency at 15,000 qps** (sub-saturation, so this is per-query cost, not queueing):

| scenario | pd avg | pd sustains | md avg | md sustains | pd max | md max |
|---|--:|--:|--:|--:|--:|--:|
| foreign miss | ~0.17 ms | 15,000 | ~0.6–1.35 ms | ~13.9k | 7–9 ms | 1001–1005 ms |
| china miss | ~0.21 ms | 15,000 | ~0.7–0.97 ms | ~13.8k | 7–14 ms | 1002–2002 ms |

photondns holds the full 15k offered load; mosdns cannot sustain even 15k on the miss path (~13.8–14.3k actual) **and** still spikes to ~1 s — at sub-saturation load.

**Tail distribution** (flat-out `-v` per-query capture, hundreds of thousands of samples):

| scenario | resolver | p50 | p90 | p99 | p999 | max | # >500 ms |
|---|---|--:|--:|--:|--:|--:|--:|
| foreign miss | photondns | 1.43 | 1.61 | 1.93 | 6.0 | **17.3** | **0** |
| foreign miss | mosdns | 0.68 | 1.45 | 1.85 | 10.2 | **1006.1** | **290** |
| china miss | photondns | 1.80 | 2.25 | 2.43 | 2.54 | **9.4** | **0** |
| china miss | mosdns | 2.10 | 2.72 | 3.05 | 4.68 | **1016.2** | **25** |

(All latencies in ms.) Note photondns's p50 is *higher* than mosdns's on foreign_miss at flat-out — because photondns is completing more QPS (396k vs 364k samples), so its per-query queue is deeper at saturation. photondns's advantage is in the **worst-case tail** and in **clean sub-saturation latency**, not the median at saturation.

## 3 · Root cause of the ~1 s mosdns spikes

The near-integer-second maxima (~1000/2000 ms) look like a **1 s forward retransmit** after a lost loopback UDP datagram, not Go GC (GC pauses are variable 10–300 ms, never exactly 1000 ms). The decisive test: run the same foreign_miss flat-out against the **plain** stub vs the **drop-proof hardened** stub. If the spikes are our stub dropping packets, they vanish on the hardened stub; if they are mosdns-side, they persist.

| resolver | stub | QPS | max latency | # >50 ms | # >500 ms |
|---|---|--:|--:|--:|--:|
| photondns | plain | 79,523 | 8.9 ms | 0 | 0 |
| photondns | hardened | 75,733 | 8.1 ms | 0 | 0 |
| mosdns | plain | 39,723 | 1003.2 ms | 463 | 463 |
| mosdns | hardened | 50,446 | 1004.5 ms | 444 | 444 |

**Conclusion:** the spikes are **mosdns-side.** A drop-proof upstream with 8 MB buffers and a socket per core barely moves mosdns's count of >500 ms queries (463 → 444) and does not touch the ~1 s ceiling — the retransmit/stall lives in mosdns's UDP forward path, not in our stub. It is therefore **fair to attribute to mosdns as shipped.** (Corroborated: mosdns hits ~1–2 s max on every definitive flat-out run and every clean 15k sub-saturation run; photondns never exceeds ~17 ms anywhere.)

Interesting side effect visible above: the hardened stub *raises mosdns's throughput* (foreign_miss 39.7k → 50.4k; and the full definitive run put it at 63k vs 30k on the old plain-stub run). The single-socket plain stub was throttling mosdns's throughput — which is exactly why the honest throughput comparison uses the hardened stub (§2), not the plain one.

## Honest caveats

- **Throughput lead is modest, and the earlier plain-stub figures overstated it.** An initial plain-single-socket-stub run showed 1.40× (foreign) / 1.15× (china); that gap was partly the stub throttling mosdns. On the fair drop-proof stub the real lead is **1.06–1.13×**. The tail-latency finding, by contrast, is unchanged by the fairer stub.
- **photondns does not win the median at saturation** on foreign_miss (see §2 tail note). Its wins are worst-case tail and clean per-query latency.
- **Single host, loopback, network removed by construction.** This measures resolver processing cost, not real-WAN behavior; absolute QPS is host- and generator-specific and not portable.
- **Core oversubscription:** 8 resolver threads + dnsperf (20 conn / 4 threads) + stub share 10 cores. Mitigated by measuring one resolver at a time and reporting medians; not eliminated.
- The dnsperf `-v` `*.tail.sorted` **max** field for the *mixed* plain-stub run is a parsing artifact (shows ~2.5e10 ms); ignore it — use the `fixed_load` max (pd 4.7 ms, md 1001 ms) for that scenario.

## Reproduce

```
cd bench
./run.sh              # full matrix on the plain stub (throughput + fixed-load + tail)
./stub_experiment.sh  # plain vs hardened stub — proves the 1 s spikes are mosdns-side
./miss_final.sh       # DEFINITIVE forwarding flat-out + tail, on the hardened stub  (§2, §3)
./miss_latency.sh     # clean sub-saturation (15k qps) forwarding latency            (§2)
```

Raw artifacts: `results/max_throughput.csv`, `results/fixed_load.csv`, `results/miss_final.csv`, `results/miss_latency_15000.csv`, `results/stubexp_*.raw`, `results/mf_*.vsorted`, `results/*.r*.txt`.

## Status of the power-loss-interrupted run

**Data collection completed before the power loss.** All outputs are present and internally consistent: `miss_final.csv` (24/24 rows), the four `stubexp_*.raw` files, every `mf_*.vsorted` tail capture, and `miss_latency_15000.csv` (16/16 rows). What the crash destroyed was the **analysis printed to the terminal** and the **write-up** — both recomputed and captured in this document. The `summary.md` you may have seen previously (throughput only, plain stub) predated this forwarding-path investigation and has been superseded here.

**Optional remaining step:** `verify_workflow.js` (a 4-lens adversarial fairness/validity audit) was authored but never run.
