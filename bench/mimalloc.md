# mimalloc (fastalloc) on the cache hot path

The daemon allocates several small buffers per query (cache key string, response
byte copy, per-attempt query copy). Under multi-thread load the stock musl
allocator on OpenWrt serializes those allocations and its latency degrades badly
once threads oversubscribe cores. mimalloc keeps per-thread heaps, so it stays
flat.

`cachebench` (a CPU-isolated copy of the server.rs cache-hit path — make_key +
shard get + make_response) measured on the x86_64 test router (4 cores, musl):

| threads | allocator | ns/hit | hits/s |
|--------:|-----------|-------:|-------:|
| 4  | system   | 138–152 | 6.6–7.2M |
| 4  | mimalloc | 118–135 | 7.4–8.4M |
| 8  | system   | 150–193 | 5.2–6.6M |
| 8  | mimalloc | 117–120 | 8.4–8.5M |

At 4 threads (one per core) mimalloc is ~12–15% faster. At 8 threads
(oversubscribed — the overload case where "stuck" reports come from) the system
allocator collapses to 150–193 ns while mimalloc holds ~118 ns: **35–40%
faster and far steadier**.

The release workflow builds every target with `--features fastalloc`. To
reproduce:

```sh
cargo zigbuild --release --target x86_64-unknown-linux-musl --bin cachebench
cargo zigbuild --release --features fastalloc --target x86_64-unknown-linux-musl --bin cachebench
# copy both to the router, then:
./cachebench-system   8 1000000
./cachebench-mimalloc 8 1000000
```
