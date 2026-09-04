# rust-dns-nuke

A UDP DNS stress/flood tool for load-testing a photondns instance (or any UDP
DNS server) directly. Pure Rust `std`, no dependencies, so it builds on an old
toolchain without fetching crates.

It spawns N worker threads, each with its own UDP socket, each keeping up to
`--inflight` A-queries outstanding at once. It reports throughput, latency
percentiles, timeouts and rcodes, and exits non-zero when the target "fails"
under load (any timeout or SERVFAIL, or p99 latency above `--fail-p99-ms`).

## Build

```sh
cargo build --release
```

Binary at `target/release/dnsnuke`.

## Usage

```sh
dnsnuke <host> <port> [flags]
```

| flag | default | meaning |
|------|---------|---------|
| `--duration S`     | 15   | run length in seconds |
| `--workers N`      | ncpu | worker threads |
| `--inflight K`     | 256  | queries kept outstanding per worker |
| `--timeout-ms M`   | 2000 | per-query timeout |
| `--random-sub`     | off  | prepend a random label so every query misses cache and hits upstream |
| `--fail-p99-ms M`  | 1000 | p99 budget for the PASS/FAIL verdict |

Total in-flight cap is `workers * inflight`.

## Examples

Cached flat-out throughput (all names cached, tests the local hot path):

```sh
dnsnuke 172.16.10.4 15533 --duration 15 --workers 12 --inflight 512
```

Cache-busting (every query traverses an upstream — this is the run that
exercises upstream failover and the connection-recovery path):

```sh
dnsnuke 172.16.10.4 15533 --duration 20 --workers 8 --inflight 128 \
        --random-sub --timeout-ms 3000 --fail-p99-ms 3000
```

## Reading the verdict

`FAIL` here means "the target dropped queries or was slow under this load", not
"the server is broken". Under a flat-out flood some loss and elevated p99 are
expected once you pass saturation. What matters for photondns is that after the
run the daemon is still up, still resolving, and no upstream is stuck DOWN:
check `http://<host>:8053/stats` and the `marked DOWN` / `recovered` lines in
the log. During a cache-busting flood upstreams should flip DOWN and recover
within seconds, never wedge.
