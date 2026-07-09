//! In-memory query log: fixed-size ring buffer, newest-first snapshots
//! for the LuCI "Query Log" page.

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct QueryLogEntry {
    pub ts: u64,
    pub client: String,
    pub qname: String,
    pub qtype: u16,
    /// how the client query arrived: "udp" | "tcp" | "doh"
    pub proto: &'static str,
    /// cache | stale | hosts | blocked | redirect | servfail | <group name>
    pub route: String,
    /// winning upstream for forwarded queries, "" otherwise
    pub upstream: String,
    pub rtt_us: u32,
}

pub struct QueryLog {
    buf: Mutex<VecDeque<QueryLogEntry>>,
    cap: usize,
}

impl QueryLog {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Mutex::new(VecDeque::with_capacity(cap.min(65536))),
            cap,
        }
    }

    pub fn enabled(&self) -> bool {
        self.cap > 0
    }

    pub fn push(&self, entry: QueryLogEntry) {
        if self.cap == 0 {
            return;
        }
        let mut buf = self.buf.lock();
        if buf.len() == self.cap {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        client: std::net::IpAddr,
        qname: &str,
        qtype: u16,
        proto: &'static str,
        route: &str,
        upstream: &str,
        rtt: std::time::Duration,
    ) {
        if self.cap == 0 {
            return;
        }
        self.push(QueryLogEntry {
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            client: client.to_string(),
            qname: qname.to_string(),
            qtype,
            proto,
            route: route.to_string(),
            upstream: upstream.to_string(),
            rtt_us: rtt.as_micros().min(u32::MAX as u128) as u32,
        });
    }

    /// Up to `n` newest entries, newest first.
    pub fn snapshot(&self, n: usize) -> Vec<serde_json::Value> {
        let buf = self.buf.lock();
        buf.iter()
            .rev()
            .take(n)
            .map(|e| {
                serde_json::json!({
                    "ts": e.ts,
                    "client": e.client,
                    "qname": e.qname,
                    "qtype": e.qtype,
                    "proto": e.proto,
                    "route": e.route,
                    "upstream": e.upstream,
                    "rtt_ms": (e.rtt_us as f64 / 100.0).round() / 10.0,
                })
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.buf.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_semantics() {
        let log = QueryLog::new(3);
        for i in 0..5 {
            log.push(QueryLogEntry {
                ts: i,
                client: "10.0.0.1".into(),
                qname: format!("q{}.example.com", i),
                qtype: 1,
                proto: "udp",
                route: "cache".into(),
                upstream: String::new(),
                rtt_us: 100,
            });
        }
        assert_eq!(log.len(), 3);
        let snap = log.snapshot(10);
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0]["qname"], "q4.example.com"); // newest first
        assert_eq!(snap[2]["qname"], "q2.example.com");
    }

    #[test]
    fn disabled_when_zero() {
        let log = QueryLog::new(0);
        assert!(!log.enabled());
        log.record(
            "10.0.0.1".parse().unwrap(),
            "x.com",
            1,
            "udp",
            "cache",
            "",
            std::time::Duration::from_millis(1),
        );
        assert_eq!(log.len(), 0);
    }
}
