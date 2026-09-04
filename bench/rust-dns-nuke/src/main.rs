//! dnsnuke - a UDP DNS stress/flood tool. Pure std, no dependencies.
//!
//! Spawns N worker threads, each with its own UDP socket, each keeping up to
//! `inflight` A-queries outstanding at once. Reports throughput, latency
//! percentiles, timeouts and rcodes. Exits non-zero when the target "fails"
//! under load (any timeout/SERVFAIL, or p99 latency over --fail-p99-ms).
//!
//! usage:
//!   dnsnuke <host> <port> [--duration S] [--workers N] [--inflight K]
//!           [--timeout-ms M] [--random-sub] [--fail-p99-ms M]

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

// ---- config ---------------------------------------------------------------

struct Config {
    host: String,
    port: u16,
    duration: Duration,
    workers: usize,
    inflight: usize,
    timeout: Duration,
    random_sub: bool,
    fail_p99_ms: f64,
}

fn parse_args() -> Config {
    let mut a = std::env::args().skip(1);
    let host = a.next().unwrap_or_else(|| die("missing host"));
    let port: u16 = a
        .next()
        .unwrap_or_else(|| die("missing port"))
        .parse()
        .unwrap_or_else(|_| die("bad port"));
    let mut c = Config {
        host,
        port,
        duration: Duration::from_secs(15),
        workers: thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        inflight: 256,
        timeout: Duration::from_millis(2000),
        random_sub: false,
        fail_p99_ms: 1000.0,
    };
    while let Some(flag) = a.next() {
        let mut val = || a.next().unwrap_or_else(|| die("flag needs a value"));
        match flag.as_str() {
            "--duration" => c.duration = Duration::from_secs_f64(val().parse().unwrap()),
            "--workers" => c.workers = val().parse().unwrap(),
            "--inflight" => c.inflight = val().parse().unwrap(),
            "--timeout-ms" => c.timeout = Duration::from_millis(val().parse().unwrap()),
            "--random-sub" => c.random_sub = true,
            "--fail-p99-ms" => c.fail_p99_ms = val().parse().unwrap(),
            other => die(&format!("unknown flag {other}")),
        }
    }
    c
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    eprintln!("usage: dnsnuke <host> <port> [--duration S] [--workers N] [--inflight K] [--timeout-ms M] [--random-sub] [--fail-p99-ms M]");
    std::process::exit(2);
}

// ---- tiny fast PRNG (xorshift64) ------------------------------------------

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

const DOMAINS: &[&str] = &[
    "google.com", "youtube.com", "facebook.com", "wikipedia.org", "amazon.com",
    "cloudflare.com", "github.com", "microsoft.com", "apple.com", "netflix.com",
    "baidu.com", "taobao.com", "qq.com", "bing.com", "reddit.com",
    "twitter.com", "instagram.com", "linkedin.com", "openwrt.org", "kernel.org",
];

// ---- DNS wire (query build + rcode read) ----------------------------------

fn build_query(buf: &mut Vec<u8>, id: u16, name: &str) {
    buf.clear();
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&[0x01, 0x00]); // RD
    buf.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // QD=1
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);
    buf.extend_from_slice(&[0, 1, 0, 1]); // A, IN
}

#[inline]
fn rcode(resp: &[u8]) -> i32 {
    if resp.len() < 4 {
        -1
    } else {
        (resp[3] & 0x0F) as i32
    }
}

// ---- shared counters ------------------------------------------------------

#[derive(Default)]
struct Counters {
    sent: AtomicU64,
    ok: AtomicU64,
    servfail: AtomicU64,
    other_rcode: AtomicU64,
    timeout: AtomicU64,
    send_err: AtomicU64,
}

// worker returns its sorted latency samples (µs) so main can merge percentiles
fn worker(cfg: Arc<Config>, target: SocketAddr, ctr: Arc<Counters>, seed: u64) -> Vec<u64> {
    let sock = match UdpSocket::bind(if target.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }) {
        Ok(s) => s,
        Err(_) => {
            ctr.send_err.fetch_add(1, Ordering::Relaxed);
            return Vec::new();
        }
    };
    // short read timeout so we can interleave sends and reaping
    sock.set_read_timeout(Some(Duration::from_millis(2))).ok();
    let _ = sock.connect(target);

    let mut rng = Rng::new(seed);
    let mut pending: HashMap<u16, Instant> = HashMap::with_capacity(cfg.inflight * 2);
    let mut lat: Vec<u64> = Vec::new();
    let mut qbuf: Vec<u8> = Vec::with_capacity(64);
    let mut rbuf = [0u8; 1500];
    let mut next_id: u16 = (seed & 0xFFFF) as u16;
    let mut namebuf = String::with_capacity(32);

    let end = Instant::now() + cfg.duration;
    let mut last_reap = Instant::now();

    while Instant::now() < end {
        // top up the in-flight window
        while pending.len() < cfg.inflight {
            let id = next_id;
            next_id = next_id.wrapping_add(1);
            if pending.contains_key(&id) {
                break; // window wrapped onto a live id; reap first
            }
            let base = DOMAINS[rng.below(DOMAINS.len())];
            let name: &str = if cfg.random_sub {
                namebuf.clear();
                let r = rng.next_u64();
                // 8 lowercase letters + "." + base -> cache-busting
                for i in 0..8 {
                    let c = b'a' + ((r >> (i * 5)) % 26) as u8;
                    namebuf.push(c as char);
                }
                namebuf.push('.');
                namebuf.push_str(base);
                &namebuf
            } else {
                base
            };
            build_query(&mut qbuf, id, name);
            match sock.send(&qbuf) {
                Ok(_) => {
                    pending.insert(id, Instant::now());
                    ctr.sent.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    ctr.send_err.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
        }

        // drain whatever answers are ready
        loop {
            match sock.recv(&mut rbuf) {
                Ok(n) if n >= 4 => {
                    let id = u16::from_be_bytes([rbuf[0], rbuf[1]]);
                    if let Some(t0) = pending.remove(&id) {
                        lat.push(t0.elapsed().as_micros() as u64);
                        match rcode(&rbuf[..n]) {
                            0 => ctr.ok.fetch_add(1, Ordering::Relaxed),
                            2 => ctr.servfail.fetch_add(1, Ordering::Relaxed),
                            _ => ctr.other_rcode.fetch_add(1, Ordering::Relaxed),
                        };
                    }
                }
                Ok(_) => {}
                Err(_) => break, // WouldBlock/timeout: nothing more to read now
            }
        }

        // reap timed-out queries roughly every 20ms
        if last_reap.elapsed() >= Duration::from_millis(20) {
            let now = Instant::now();
            let to = cfg.timeout;
            let before = pending.len();
            pending.retain(|_, &mut t| now.duration_since(t) < to);
            let reaped = (before - pending.len()) as u64;
            if reaped > 0 {
                ctr.timeout.fetch_add(reaped, Ordering::Relaxed);
            }
            last_reap = now;
        }
    }

    // drain window for late answers
    let drain_until = Instant::now() + cfg.timeout;
    while !pending.is_empty() && Instant::now() < drain_until {
        match sock.recv(&mut rbuf) {
            Ok(n) if n >= 4 => {
                let id = u16::from_be_bytes([rbuf[0], rbuf[1]]);
                if let Some(t0) = pending.remove(&id) {
                    lat.push(t0.elapsed().as_micros() as u64);
                    match rcode(&rbuf[..n]) {
                        0 => ctr.ok.fetch_add(1, Ordering::Relaxed),
                        2 => ctr.servfail.fetch_add(1, Ordering::Relaxed),
                        _ => ctr.other_rcode.fetch_add(1, Ordering::Relaxed),
                    };
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    ctr.timeout.fetch_add(pending.len() as u64, Ordering::Relaxed);

    lat.sort_unstable();
    lat
}

fn pct(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p * sorted.len() as f64) as usize).min(sorted.len() - 1);
    sorted[idx] as f64 / 1000.0
}

fn main() {
    let cfg = Arc::new(parse_args());
    let target: SocketAddr = (cfg.host.as_str(), cfg.port)
        .to_socket_addrs()
        .unwrap_or_else(|e| die(&format!("resolve target: {e}")))
        .next()
        .unwrap_or_else(|| die("no address for target"));

    println!("== dnsnuke -> {target}");
    println!(
        "   workers={} inflight/worker={} duration={:?} timeout={:?} random_sub={}",
        cfg.workers, cfg.inflight, cfg.duration, cfg.timeout, cfg.random_sub
    );
    println!("   total in-flight cap = {}", cfg.workers * cfg.inflight);

    let ctr = Arc::new(Counters::default());
    let t0 = Instant::now();
    let mut handles = Vec::new();
    for w in 0..cfg.workers {
        let cfg = cfg.clone();
        let ctr = ctr.clone();
        let seed = 0x9E3779B97F4A7C15u64
            .wrapping_mul(w as u64 + 1)
            ^ (t0.elapsed().as_nanos() as u64);
        handles.push(thread::spawn(move || worker(cfg, target, ctr, seed)));
    }
    let mut all_lat: Vec<u64> = Vec::new();
    for h in handles {
        if let Ok(mut l) = h.join() {
            all_lat.append(&mut l);
        }
    }
    let elapsed = t0.elapsed().as_secs_f64();
    all_lat.sort_unstable();

    let sent = ctr.sent.load(Ordering::Relaxed);
    let ok = ctr.ok.load(Ordering::Relaxed);
    let servfail = ctr.servfail.load(Ordering::Relaxed);
    let other = ctr.other_rcode.load(Ordering::Relaxed);
    let timeout = ctr.timeout.load(Ordering::Relaxed);
    let send_err = ctr.send_err.load(Ordering::Relaxed);
    let answered = ok + servfail + other;
    let loss = if sent > 0 { timeout as f64 / sent as f64 * 100.0 } else { 0.0 };

    let mean_ms = if !all_lat.is_empty() {
        all_lat.iter().sum::<u64>() as f64 / all_lat.len() as f64 / 1000.0
    } else {
        0.0
    };

    println!("\n== results ({elapsed:.1}s)");
    println!("   sent          {sent:>10}  ({:.0} q/s)", sent as f64 / elapsed);
    println!("   answered      {answered:>10}  ({:.0} q/s)", answered as f64 / elapsed);
    println!("     noerror     {ok:>10}");
    println!("     servfail    {servfail:>10}");
    println!("     other rcode {other:>10}");
    println!("   timeouts      {timeout:>10}  ({loss:.2}% loss)");
    println!("   send errors   {send_err:>10}");
    if !all_lat.is_empty() {
        println!(
            "   latency ms    min={:.1} p50={:.1} p90={:.1} p99={:.1} max={:.1} mean={:.1}",
            all_lat[0] as f64 / 1000.0,
            pct(&all_lat, 0.50),
            pct(&all_lat, 0.90),
            pct(&all_lat, 0.99),
            *all_lat.last().unwrap() as f64 / 1000.0,
            mean_ms
        );
    }

    let p99 = pct(&all_lat, 0.99);
    let mut reasons: Vec<String> = Vec::new();
    if timeout > 0 {
        reasons.push(format!("{timeout} timeouts ({loss:.2}% loss)"));
    }
    if servfail > 0 {
        reasons.push(format!("{servfail} SERVFAIL"));
    }
    if !all_lat.is_empty() && p99 > cfg.fail_p99_ms {
        reasons.push(format!("p99 {p99:.0}ms > {:.0}ms", cfg.fail_p99_ms));
    }
    if reasons.is_empty() {
        println!("\n== verdict: PASS (no loss, no servfail, p99 within budget)");
        std::process::exit(0);
    } else {
        println!("\n== verdict: FAIL - {}", reasons.join("; "));
        std::process::exit(1);
    }
}
