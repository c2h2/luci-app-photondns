//! Tiny UDP DNS load generator for benchmarking photondns.
//! usage: photonbench <server:port> <qname> [concurrency] [total]

use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn build_query(name: &str, id: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.split('.').filter(|l| !l.is_empty()) {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: photonbench <server:port> <qname> [concurrency=32] [total=10000]");
        std::process::exit(2);
    }
    let server = args[1].clone();
    let qname = args[2].clone();
    let concurrency: usize = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(32);
    let total: u64 = args.get(4).map(|s| s.parse().unwrap()).unwrap_or(10_000);

    let sent = Arc::new(AtomicU64::new(0));
    let ok = Arc::new(AtomicU64::new(0));
    let errs = Arc::new(AtomicU64::new(0));
    let lat_us_sum = Arc::new(AtomicU64::new(0));
    let lat_us_max = Arc::new(AtomicU64::new(0));

    let start = Instant::now();
    let mut handles = Vec::new();
    for t in 0..concurrency {
        let server = server.clone();
        let qname = qname.clone();
        let sent = sent.clone();
        let ok = ok.clone();
        let errs = errs.clone();
        let lat_sum = lat_us_sum.clone();
        let lat_max = lat_us_max.clone();
        handles.push(std::thread::spawn(move || {
            let sock = UdpSocket::bind("0.0.0.0:0").unwrap();
            sock.connect(&server).unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut buf = [0u8; 4096];
            let mut id = t as u16;
            loop {
                let n = sent.fetch_add(1, Ordering::Relaxed);
                if n >= total {
                    break;
                }
                id = id.wrapping_add(1);
                let q = build_query(&qname, id);
                let t0 = Instant::now();
                if sock.send(&q).is_err() {
                    errs.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                match sock.recv(&mut buf) {
                    Ok(n) if n >= 12 && buf[0..2] == id.to_be_bytes() => {
                        let us = t0.elapsed().as_micros() as u64;
                        lat_sum.fetch_add(us, Ordering::Relaxed);
                        lat_max.fetch_max(us, Ordering::Relaxed);
                        ok.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {
                        errs.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let elapsed = start.elapsed();
    let okn = ok.load(Ordering::Relaxed);
    let errn = errs.load(Ordering::Relaxed);
    println!(
        "sent {} ok {} err {} in {:.2}s -> {:.0} qps, avg {:.2} ms, max {:.2} ms",
        total.min(sent.load(Ordering::Relaxed)),
        okn,
        errn,
        elapsed.as_secs_f64(),
        okn as f64 / elapsed.as_secs_f64(),
        if okn > 0 {
            lat_us_sum.load(Ordering::Relaxed) as f64 / okn as f64 / 1000.0
        } else {
            0.0
        },
        lat_us_max.load(Ordering::Relaxed) as f64 / 1000.0
    );
}
