//! Per-upstream health: EWMA latency, consecutive-failure circuit breaker
//! with half-open recovery.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub struct UpstreamState {
    healthy: AtomicBool,
    consec_fail: AtomicU32,
    consec_ok: AtomicU32,
    /// exponentially weighted moving average RTT in µs (0 = no data yet)
    ewma_us: AtomicU64,
    pub ok: AtomicU64,
    pub fail: AtomicU64,
    pub last_rtt_us: AtomicU64,
    /// unix ms until which a down upstream stays excluded (then half-open)
    cooldown_until_ms: AtomicU64,
    pub down_events: AtomicU64,
}

impl UpstreamState {
    pub fn new() -> Self {
        Self {
            healthy: AtomicBool::new(true),
            consec_fail: AtomicU32::new(0),
            consec_ok: AtomicU32::new(0),
            ewma_us: AtomicU64::new(0),
            ok: AtomicU64::new(0),
            fail: AtomicU64::new(0),
            last_rtt_us: AtomicU64::new(0),
            cooldown_until_ms: AtomicU64::new(0),
            down_events: AtomicU64::new(0),
        }
    }

    pub fn record_success(&self, rtt: Duration, recover_threshold: u32, name: &str) {
        let us = rtt.as_micros() as u64;
        self.last_rtt_us.store(us, Ordering::Relaxed);
        self.ok.fetch_add(1, Ordering::Relaxed);
        self.consec_fail.store(0, Ordering::Relaxed);
        let old = self.ewma_us.load(Ordering::Relaxed);
        let new = if old == 0 { us } else { (old * 7 + us) / 8 };
        self.ewma_us.store(new, Ordering::Relaxed);
        if !self.healthy.load(Ordering::Relaxed) {
            let ok = self.consec_ok.fetch_add(1, Ordering::Relaxed) + 1;
            if ok >= recover_threshold {
                self.healthy.store(true, Ordering::Relaxed);
                self.consec_ok.store(0, Ordering::Relaxed);
                log::warn!("upstream {} recovered, back in rotation", name);
            }
        }
    }

    pub fn record_failure(&self, fail_threshold: u32, cooldown: Duration, name: &str) {
        self.fail.fetch_add(1, Ordering::Relaxed);
        self.consec_ok.store(0, Ordering::Relaxed);
        let fails = self.consec_fail.fetch_add(1, Ordering::Relaxed) + 1;
        let was_healthy = self.healthy.load(Ordering::Relaxed);
        if was_healthy && fails >= fail_threshold {
            self.healthy.store(false, Ordering::Relaxed);
            self.down_events.fetch_add(1, Ordering::Relaxed);
            self.cooldown_until_ms
                .store(unix_ms() + cooldown.as_millis() as u64, Ordering::Relaxed);
            log::warn!(
                "upstream {} marked DOWN after {} consecutive failures",
                name,
                fails
            );
        } else if !was_healthy {
            // failed during half-open trial: restart cooldown
            self.cooldown_until_ms
                .store(unix_ms() + cooldown.as_millis() as u64, Ordering::Relaxed);
        }
    }

    /// Available = healthy, or down but past cooldown (half-open trial).
    pub fn is_available(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
            || unix_ms() >= self.cooldown_until_ms.load(Ordering::Relaxed)
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// EWMA in µs; unknown upstreams get a neutral 50ms so they are tried early.
    pub fn ewma_us_or_default(&self) -> u64 {
        match self.ewma_us.load(Ordering::Relaxed) {
            0 => 50_000,
            v => v,
        }
    }

    pub fn ewma_ms(&self) -> f64 {
        self.ewma_us.load(Ordering::Relaxed) as f64 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_and_recovers() {
        let s = UpstreamState::new();
        assert!(s.is_healthy());
        for _ in 0..3 {
            s.record_failure(3, Duration::from_millis(50), "t");
        }
        assert!(!s.is_healthy());
        assert!(!s.is_available());
        std::thread::sleep(Duration::from_millis(60));
        assert!(s.is_available()); // half-open
        s.record_success(Duration::from_millis(10), 2, "t");
        assert!(!s.is_healthy()); // needs 2 consecutive
        s.record_success(Duration::from_millis(10), 2, "t");
        assert!(s.is_healthy());
    }

    #[test]
    fn ewma_moves() {
        let s = UpstreamState::new();
        s.record_success(Duration::from_millis(100), 2, "t");
        let e1 = s.ewma_us_or_default();
        assert_eq!(e1, 100_000);
        s.record_success(Duration::from_millis(20), 2, "t");
        assert!(s.ewma_us_or_default() < e1);
    }
}
