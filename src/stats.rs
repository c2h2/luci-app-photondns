use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Lock-free sliding 60s rate window.
pub struct RateWindow {
    slots: [AtomicU64; 60],
    stamps: [AtomicU64; 60],
}

impl RateWindow {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
            stamps: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    pub fn incr(&self) {
        let now = unix_now();
        let i = (now % 60) as usize;
        if self.stamps[i].swap(now, Ordering::Relaxed) != now {
            self.slots[i].store(0, Ordering::Relaxed);
        }
        self.slots[i].fetch_add(1, Ordering::Relaxed);
    }

    /// events in the last full 60 seconds
    pub fn last_minute(&self) -> u64 {
        let now = unix_now();
        let mut sum = 0;
        for i in 0..60 {
            let stamp = self.stamps[i].load(Ordering::Relaxed);
            if now.saturating_sub(stamp) < 60 {
                sum += self.slots[i].load(Ordering::Relaxed);
            }
        }
        sum
    }
}

pub struct Stats {
    pub started_unix: u64,
    pub total: AtomicU64,
    pub udp: AtomicU64,
    pub tcp: AtomicU64,
    pub doh: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub stale_served: AtomicU64,
    pub prefetches: AtomicU64,
    pub blocked: AtomicU64,
    pub hosts_served: AtomicU64,
    pub redirected: AtomicU64,
    pub upstream_errors: AtomicU64,
    pub servfail: AtomicU64,
    pub hedged: AtomicU64,
    pub rate: RateWindow,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            started_unix: unix_now(),
            total: AtomicU64::new(0),
            udp: AtomicU64::new(0),
            tcp: AtomicU64::new(0),
            doh: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            stale_served: AtomicU64::new(0),
            prefetches: AtomicU64::new(0),
            blocked: AtomicU64::new(0),
            hosts_served: AtomicU64::new(0),
            redirected: AtomicU64::new(0),
            upstream_errors: AtomicU64::new(0),
            servfail: AtomicU64::new(0),
            hedged: AtomicU64::new(0),
            rate: RateWindow::new(),
        }
    }

    pub fn uptime(&self) -> u64 {
        unix_now().saturating_sub(self.started_unix)
    }
}
