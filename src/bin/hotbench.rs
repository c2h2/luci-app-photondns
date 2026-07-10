//! hotbench — CPU-isolated microbenchmark of the query hot path.
//!
//! The loopback forwarding benchmark on a fast desktop is syscall/scheduling
//! bound (photondns uses only ~3 of 10 cores flat-out), so it cannot show
//! per-query CPU improvements. On the real target (an OpenWrt router: weak CPU,
//! slow musl malloc) the query path IS cpu-bound, and that is what this measures
//! in isolation: domain-set matching (the router's hottest work) with the old
//! SipHash vs the new FxHash, over the REAL china list and REAL query names.
//!
//! usage: hotbench <china_list.txt> <query_names.txt> [iters]

use std::collections::HashSet;
use std::hash::BuildHasher;
use std::time::Instant;

/// A domain set generic over the hasher, matching src/router.rs DomainSet logic.
struct DomainSet<S> {
    full: HashSet<String, S>,
    suffix: HashSet<String, S>,
}

impl<S: BuildHasher + Default> DomainSet<S> {
    fn load(path: &str) -> Self {
        let mut full: HashSet<String, S> = HashSet::default();
        let mut suffix: HashSet<String, S> = HashSet::default();
        let text = std::fs::read_to_string(path).expect("read list");
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(d) = line.strip_prefix("full:") {
                full.insert(norm(d));
            } else if let Some(d) = line.strip_prefix("domain:") {
                suffix.insert(norm(d));
            } else {
                suffix.insert(norm(line));
            }
        }
        Self { full, suffix }
    }
    // identical to router.rs DomainSet::matches
    #[inline]
    fn matches(&self, name: &str) -> bool {
        if self.full.contains(name) || self.suffix.contains(name) {
            return true;
        }
        let mut rest = name;
        while let Some(i) = rest.find('.') {
            rest = &rest[i + 1..];
            if self.suffix.contains(rest) {
                return true;
            }
        }
        false
    }
}

fn norm(d: &str) -> String {
    d.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn run<S: BuildHasher + Default>(label: &str, list: &str, names: &[String], iters: usize) -> f64 {
    let set = DomainSet::<S>::load(list);
    // warm
    let mut hits = 0u64;
    for n in names {
        if set.matches(n) {
            hits += 1;
        }
    }
    let t = Instant::now();
    let mut sink = 0u64;
    for _ in 0..iters {
        for n in names {
            if set.matches(n) {
                sink += 1;
            }
        }
    }
    let el = t.elapsed();
    let total = (iters * names.len()) as f64;
    let per = el.as_secs_f64() / total * 1e9;
    let qps = total / el.as_secs_f64();
    println!(
        "  {:10}  {:>7.1} ns/query   {:>10.0} match/s/core   ({} entries, {}/{} names matched, sink={})",
        label, per, qps, set.full.len() + set.suffix.len(), hits, names.len(), sink
    );
    per
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: hotbench <china_list.txt> <query_names.txt> [iters]");
        std::process::exit(2);
    }
    let list = &a[1];
    let qfile = &a[2];
    let iters: usize = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(40);

    // query files are dnsperf format: "<name> <TYPE>" per line
    let text = std::fs::read_to_string(qfile).expect("read queries");
    let names: Vec<String> = text
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(|n| n.trim_end_matches('.').to_ascii_lowercase())
        .take(20000)
        .collect();
    println!(
        "hotbench: {} names x {} iters vs {}",
        names.len(),
        iters,
        list
    );

    println!("router domain-set matching (the hottest per-query CPU work):");
    let sip = run::<std::collections::hash_map::RandomState>("SipHash", list, &names, iters);
    let fx = run::<rustc_hash::FxBuildHasher>("FxHash", list, &names, iters);
    println!(
        "  => FxHash speedup: {:.2}x  ({:.0}% less CPU per match)",
        sip / fx,
        (1.0 - fx / sip) * 100.0
    );
}
