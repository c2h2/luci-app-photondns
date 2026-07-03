//! photonrbench - randomized parallel DNS benchmark for photondns.
//!
//! Generates N random domains each run (so every cold pass is all-misses),
//! fires them through a parallel worker pool, and reports throughput plus
//! full latency percentiles. Runs two phases:
//!   cold  - random domains, first sight => cache misses => upstream path
//!   warm  - the SAME domains re-queried => cache hits => raw serving speed
//!
//! usage: photonrbench [server:port] [count] [concurrency]
//!   defaults: 127.0.0.1:15533  1000  50
//! env:
//!   SUFFIX=example.com   append a real registrable suffix so misses still
//!                        resolve upstream (default: random <label>.com)
//!   WARM=0               skip the warm (cache-hit) phase
//!   SEED=<n>             deterministic domain set (default: time-based)

use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Small xorshift RNG so we don't pull in a crate for a bench binary.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn label(&mut self, len: usize) -> String {
        const CS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut s = String::with_capacity(len);
        // first char must be a letter
        s.push((b'a' + (self.next() % 26) as u8) as char);
        for _ in 1..len {
            s.push(CS[(self.next() % CS.len() as u64) as usize] as char);
        }
        s
    }
}

fn gen_domains(n: usize, seed: u64, suffix: &Option<String>) -> Vec<String> {
    let mut rng = Rng(seed | 1);
    (0..n)
        .map(|_| {
            let len = 8 + (rng.next() % 9) as usize; // 8..16 chars
            let host = rng.label(len);
            match suffix {
                Some(s) => format!("{}.{}", host, s),
                None => format!("{}.com", host),
            }
        })
        .collect()
}

fn build_query(name: &str, id: u16, qtype: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len() + 20);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.split('.').filter(|l| !l.is_empty()) {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out
}

struct Phase {
    ok: AtomicU64,
    err: AtomicU64,
    servfail: AtomicU64,
    lat_us: Mutex<Vec<u32>>,
}

impl Phase {
    fn new(cap: usize) -> Self {
        Phase {
            ok: AtomicU64::new(0),
            err: AtomicU64::new(0),
            servfail: AtomicU64::new(0),
            lat_us: Mutex::new(Vec::with_capacity(cap)),
        }
    }
}

/// Run one pass over `domains` with `concurrency` worker threads.
/// Returns wall-clock elapsed.
fn run_phase(
    server: &str,
    domains: &[String],
    concurrency: usize,
    timeout: Duration,
    phase: &Arc<Phase>,
) -> Duration {
    let cursor = Arc::new(AtomicU64::new(0));
    let domains = Arc::new(domains.to_vec());
    let start = Instant::now();
    let mut handles = Vec::new();

    for t in 0..concurrency {
        let server = server.to_string();
        let domains = domains.clone();
        let cursor = cursor.clone();
        let phase = phase.clone();
        handles.push(std::thread::spawn(move || {
            let sock = match UdpSocket::bind("0.0.0.0:0") {
                Ok(s) => s,
                Err(_) => return,
            };
            if sock.connect(&server).is_err() {
                return;
            }
            sock.set_read_timeout(Some(timeout)).ok();
            let mut buf = [0u8; 4096];
            let mut id = (t as u16).wrapping_mul(7919).wrapping_add(1);
            let mut local_lat: Vec<u32> = Vec::new();
            loop {
                let i = cursor.fetch_add(1, Ordering::Relaxed) as usize;
                if i >= domains.len() {
                    break;
                }
                id = id.wrapping_add(1);
                let q = build_query(&domains[i], id, 1);
                let t0 = Instant::now();
                if sock.send(&q).is_err() {
                    phase.err.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                // match by ID; ignore stray/duplicate packets briefly
                let mut matched = false;
                for _ in 0..4 {
                    match sock.recv(&mut buf) {
                        Ok(n) if n >= 12 && buf[0..2] == id.to_be_bytes() => {
                            let us = t0.elapsed().as_micros().min(u32::MAX as u128) as u32;
                            local_lat.push(us);
                            let rcode = buf[3] & 0x0f;
                            if rcode == 2 {
                                phase.servfail.fetch_add(1, Ordering::Relaxed);
                            }
                            phase.ok.fetch_add(1, Ordering::Relaxed);
                            matched = true;
                            break;
                        }
                        Ok(_) => continue, // wrong id, keep reading
                        Err(_) => break,   // timeout
                    }
                }
                if !matched {
                    phase.err.fetch_add(1, Ordering::Relaxed);
                }
            }
            if !local_lat.is_empty() {
                phase.lat_us.lock().unwrap().extend_from_slice(&local_lat);
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    start.elapsed()
}

fn pct(sorted: &[u32], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)] as f64 / 1000.0
}

fn report(name: &str, count: usize, elapsed: Duration, phase: &Phase) {
    let ok = phase.ok.load(Ordering::Relaxed);
    let err = phase.err.load(Ordering::Relaxed);
    let sf = phase.servfail.load(Ordering::Relaxed);
    let secs = elapsed.as_secs_f64().max(1e-9);
    let mut lat = phase.lat_us.lock().unwrap().clone();
    lat.sort_unstable();
    let mean = if lat.is_empty() {
        0.0
    } else {
        lat.iter().map(|&x| x as u64).sum::<u64>() as f64 / lat.len() as f64 / 1000.0
    };
    println!(
        "  {:<5} {:>6} queries in {:>6.3}s  {:>8.0} qps   ok {} err {} servfail {}",
        name, count, secs, ok as f64 / secs, ok, err, sf
    );
    println!(
        "        latency ms:  min {:.2}  p50 {:.2}  p90 {:.2}  p99 {:.2}  max {:.2}  mean {:.2}",
        pct(&lat, 0.0),
        pct(&lat, 50.0),
        pct(&lat, 90.0),
        pct(&lat, 99.0),
        pct(&lat, 100.0),
        mean
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("usage: photonrbench [server:port] [count] [concurrency]");
        eprintln!("  defaults: 127.0.0.1:15533  1000  50");
        eprintln!("  env: SUFFIX=<domain>  WARM=0  SEED=<n>");
        std::process::exit(0);
    }
    let server = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:15533".into());
    let count: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);
    let concurrency: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(50);
    let suffix = std::env::var("SUFFIX").ok().filter(|s| !s.is_empty());
    let warm = std::env::var("WARM").map(|v| v != "0").unwrap_or(true);
    let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    });

    let domains = gen_domains(count, seed, &suffix);
    println!(
        "photonrbench -> {}  ({} random domains, concurrency {}, suffix .{}, seed {})",
        server,
        count,
        concurrency,
        suffix.as_deref().unwrap_or("com"),
        seed
    );
    println!("  sample: {}  {}  {}", domains[0], domains[count / 2], domains[count - 1]);

    let cold = Arc::new(Phase::new(count));
    let e1 = run_phase(&server, &domains, concurrency, Duration::from_secs(3), &cold);
    report("cold", count, e1, &cold);

    if warm {
        let warm_p = Arc::new(Phase::new(count));
        let e2 = run_phase(&server, &domains, concurrency, Duration::from_secs(3), &warm_p);
        report("warm", count, e2, &warm_p);

        let c = cold.lat_us.lock().unwrap().len();
        let cold_mean = if c > 0 {
            cold.lat_us.lock().unwrap().iter().map(|&x| x as u64).sum::<u64>() as f64
                / c as f64
                / 1000.0
        } else {
            0.0
        };
        let w = warm_p.lat_us.lock().unwrap();
        let warm_mean = if !w.is_empty() {
            w.iter().map(|&x| x as u64).sum::<u64>() as f64 / w.len() as f64 / 1000.0
        } else {
            0.0
        };
        if warm_mean > 0.0 {
            println!(
                "  speedup: warm cache is {:.1}x faster than cold ({:.2} ms -> {:.2} ms mean)",
                cold_mean / warm_mean,
                cold_mean,
                warm_mean
            );
        }
    }
}
