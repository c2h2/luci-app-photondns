//! cachebench — CPU-isolated microbenchmark of the cache hot path.
//!
//! The loopback forwarding benchmark on a fast desktop is syscall/scheduling
//! bound, so it cannot show per-query CPU/allocation improvements (see the note
//! in hotbench.rs). This measures the cache HIT path in isolation — the common
//! case on a busy resolver — across many threads, so allocator contention shows.
//!
//! It reproduces exactly what server.rs does per cache hit:
//!   1. make_key(qname, qtype, qclass)           -> one String alloc + format
//!   2. cache.get(&key)                           -> shard lock + Arc clone
//!   3. entry.make_response(query, meta, ...)     -> full response Vec clone
//!
//! Build with the same allocator the daemon ships with to compare:
//!   cargo run --release --bin cachebench
//!   cargo run --release --features fastalloc --bin cachebench
//!
//! usage: cachebench [threads] [iters_per_thread]

#[cfg(feature = "fastalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Instant;

use lru::LruCache;
use parking_lot::Mutex;
use rustc_hash::{FxBuildHasher, FxHasher};

const SHARDS: usize = 16;

// ---- a faithful copy of the cache key + shard + response-materialize path ---

fn make_key(qname: &str, qtype: u16, qclass: u16) -> String {
    format!("{}\u{0}{}\u{0}{}", qname, qtype, qclass)
}

struct Entry {
    data: Vec<u8>,
    ttl_offsets: Box<[u16]>,
    question_len: u16,
}

impl Entry {
    // mirrors cache.rs CacheEntry::make_response: clone bytes, set id, copy the
    // client question back, age TTLs.
    fn make_response(&self, query: &[u8], id: u16, qend: usize) -> Vec<u8> {
        let mut out = self.data.clone();
        out[0..2].copy_from_slice(&id.to_be_bytes());
        let qlen = qend - 12;
        if qlen == self.question_len as usize && out.len() >= qend {
            out[12..qend].copy_from_slice(&query[12..qend]);
        }
        for &off in self.ttl_offsets.iter() {
            let off = off as usize;
            if off + 4 <= out.len() {
                let ttl = u32::from_be_bytes([out[off], out[off + 1], out[off + 2], out[off + 3]]);
                let new = ttl.saturating_sub(5).max(1);
                out[off..off + 4].copy_from_slice(&new.to_be_bytes());
            }
        }
        out
    }
}

struct Cache {
    shards: Vec<Mutex<LruCache<String, Arc<Entry>, FxBuildHasher>>>,
}

impl Cache {
    fn new(cap: usize) -> Self {
        let per = (cap / SHARDS).max(1);
        Cache {
            shards: (0..SHARDS)
                .map(|_| {
                    Mutex::new(LruCache::with_hasher(
                        NonZeroUsize::new(per).unwrap(),
                        FxBuildHasher,
                    ))
                })
                .collect(),
        }
    }
    fn shard(&self, key: &str) -> &Mutex<LruCache<String, Arc<Entry>, FxBuildHasher>> {
        let mut h = FxHasher::default();
        key.hash(&mut h);
        &self.shards[(h.finish() as usize) % SHARDS]
    }
    fn insert(&self, key: String, e: Entry) {
        self.shard(&key).lock().put(key, Arc::new(e));
    }
    fn get(&self, key: &str) -> Option<Arc<Entry>> {
        self.shard(key).lock().get(key).cloned()
    }
}

// build a realistic ~100-byte A response with 2 answers for `name`
fn sample_entry(name: &str) -> (String, Entry, Vec<u8>, usize) {
    let mut q = vec![0u8; 12];
    q[5] = 1; // QDCOUNT
    for label in name.split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&1u16.to_be_bytes()); // A
    q.extend_from_slice(&1u16.to_be_bytes()); // IN
    let qend = q.len();
    let mut r = q.clone();
    r[2] = 0x81;
    r[3] = 0x80;
    r[7] = 2; // ANCOUNT=2
    let mut offs = Vec::new();
    for (ttl, ip) in [(300u32, [1u8, 2, 3, 4]), (60u32, [5, 6, 7, 8])] {
        r.extend_from_slice(&[0xC0, 0x0C]);
        r.extend_from_slice(&1u16.to_be_bytes());
        r.extend_from_slice(&1u16.to_be_bytes());
        offs.push(r.len() as u16);
        r.extend_from_slice(&ttl.to_be_bytes());
        r.extend_from_slice(&4u16.to_be_bytes());
        r.extend_from_slice(&ip);
    }
    let key = make_key(name, 1, 1);
    let entry = Entry {
        data: r,
        ttl_offsets: offs.into_boxed_slice(),
        question_len: (qend - 12) as u16,
    };
    (key, entry, q, qend)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let threads: usize = a.get(1).map(|s| s.parse().unwrap()).unwrap_or(8);
    let iters: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(2_000_000);

    let alloc = if cfg!(feature = "fastalloc") {
        "mimalloc"
    } else {
        "system"
    };

    // populate the cache with a working set of names
    const N: usize = 2000;
    let cache = Arc::new(Cache::new(65536));
    let mut queries: Vec<(String, Vec<u8>, usize)> = Vec::with_capacity(N);
    for i in 0..N {
        let name = format!("host{:04}.example{}.com", i, i % 37);
        let (key, entry, q, qend) = sample_entry(&name);
        cache.insert(key, entry);
        queries.push((name, q, qend));
    }
    let queries = Arc::new(queries);

    let barrier = Arc::new(Barrier::new(threads + 1));
    let total_bytes = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();
    for t in 0..threads {
        let cache = cache.clone();
        let queries = queries.clone();
        let barrier = barrier.clone();
        let total_bytes = total_bytes.clone();
        handles.push(std::thread::spawn(move || {
            let mut rng = 0x9E3779B97F4A7C15u64 ^ (t as u64 + 1);
            let mut local: u64 = 0;
            barrier.wait();
            for _ in 0..iters {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                let idx = (rng as usize) % queries.len();
                let (name, q, qend) = &queries[idx];
                // exactly the server.rs cache-hit path:
                let key = make_key(name, 1, 1);
                if let Some(entry) = cache.get(&key) {
                    let resp = entry.make_response(q, (rng as u16) ^ 0xBEEF, *qend);
                    local += resp.len() as u64;
                }
            }
            total_bytes.fetch_add(local, Ordering::Relaxed);
        }));
    }
    barrier.wait();
    let start = Instant::now();
    for h in handles {
        h.join().unwrap();
    }
    let el = start.elapsed();

    let total_ops = (threads * iters) as f64;
    let per_ns = el.as_secs_f64() / total_ops * 1e9;
    let qps = total_ops / el.as_secs_f64();
    println!(
        "cachebench [{alloc:>7}]  threads={threads} iters/thread={iters}\n  \
         {per_ns:>7.1} ns/hit   {qps:>12.0} hits/s   ({:.1}s, sink={})",
        el.as_secs_f64(),
        total_bytes.load(Ordering::Relaxed)
    );
}
