//! Upstream groups and query strategies.
//!
//! All strategies share one hedged-execution engine: an ordered candidate
//! list plus a hedge interval. If the current attempt hasn't answered within
//! the hedge delay (adaptive: ~2x the best upstream's EWMA latency), the next
//! candidate is fired *in parallel* and the first good answer wins. This is
//! what makes failover effectively free: a dead upstream costs one hedge
//! delay (tens of ms), not a timeout.

use crate::config::{FailoverCfg, GroupCfg};
use crate::stats::Stats;
use crate::upstream::Upstream;
use anyhow::{anyhow, bail, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strategy {
    Race,
    Fastest,
    Parallel,
    Sequential,
    Random,
}

impl Strategy {
    fn parse(s: &str) -> Strategy {
        match s {
            "fastest" => Strategy::Fastest,
            "parallel" => Strategy::Parallel,
            "sequential" => Strategy::Sequential,
            "random" => Strategy::Random,
            _ => Strategy::Race,
        }
    }
}

pub struct Group {
    pub name: String,
    pub strategy: Strategy,
    pub upstreams: Vec<Arc<Upstream>>,
    pub backups: Vec<Arc<Upstream>>,
    pub hedge_delay: Duration,
    pub timeout: Duration,
    pub fail_threshold: u32,
    pub recover_threshold: u32,
    pub cooldown: Duration,
    /// queries routed to this group (visibility for split-DNS setups)
    pub queries: std::sync::atomic::AtomicU64,
}

impl Group {
    pub fn from_config(g: &GroupCfg, fo: &FailoverCfg) -> Result<Arc<Group>> {
        let bootstrap: SocketAddr = if g.bootstrap.contains(':') {
            g.bootstrap.parse()
        } else {
            format!("{}:53", g.bootstrap).parse()
        }
        .map_err(|_| anyhow!("group {}: bad bootstrap '{}'", g.name, g.bootstrap))?;
        let mk = |addrs: &[String]| -> Result<Vec<Arc<Upstream>>> {
            addrs
                .iter()
                .map(|a| Upstream::new(a, g.insecure_skip_verify, g.idle_timeout, bootstrap))
                .collect()
        };
        Ok(Arc::new(Group {
            name: g.name.clone(),
            strategy: Strategy::parse(&g.strategy),
            upstreams: mk(&g.upstreams)?,
            backups: mk(&g.backups)?,
            hedge_delay: Duration::from_millis(g.hedge_delay_ms.max(1)),
            timeout: Duration::from_millis(g.timeout_ms.max(100)),
            fail_threshold: fo.fail_threshold.max(1),
            recover_threshold: fo.recover_threshold.max(1),
            cooldown: Duration::from_secs(fo.cooldown.max(1)),
            queries: std::sync::atomic::AtomicU64::new(0),
        }))
    }

    /// Candidate order for this query. Backups ride along at the end of the
    /// list (except for `parallel`), so even a cold-start query with dead
    /// primaries fails over within one hedge interval instead of SERVFAILing.
    /// When all primaries are down, backups take over fully.
    fn candidates(&self) -> Vec<Arc<Upstream>> {
        let mut avail: Vec<_> = self
            .upstreams
            .iter()
            .filter(|u| u.state.is_available())
            .cloned()
            .collect();
        if !avail.is_empty() {
            self.order(&mut avail);
            if self.strategy != Strategy::Parallel {
                let mut backs: Vec<_> = self
                    .backups
                    .iter()
                    .filter(|u| u.state.is_available())
                    .cloned()
                    .collect();
                backs.sort_by_key(|u| u.state.ewma_us_or_default());
                avail.extend(backs);
            }
            return avail;
        }
        let mut backup_avail: Vec<_> = self
            .backups
            .iter()
            .filter(|u| u.state.is_available())
            .cloned()
            .collect();
        if !backup_avail.is_empty() {
            log::debug!("group {}: all primaries down, using backups", self.name);
            self.order(&mut backup_avail);
            return backup_avail;
        }
        // everything is down: try the whole set anyway rather than fail
        self.upstreams
            .iter()
            .chain(self.backups.iter())
            .cloned()
            .collect()
    }

    /// Order upstreams for attempt sequence according to the strategy.
    fn order(&self, v: &mut Vec<Arc<Upstream>>) {
        match self.strategy {
            Strategy::Race | Strategy::Fastest | Strategy::Parallel => {
                v.sort_by_key(|u| u.state.ewma_us_or_default());
            }
            Strategy::Random => fastrand::shuffle(v),
            Strategy::Sequential => {} // keep configured order
        }
    }

    /// Hedge delay for this query: adaptive for race/random, immediate for
    /// parallel, a full attempt timeout for sequential/fastest.
    fn hedge_for(&self, best_ewma_us: u64) -> Duration {
        match self.strategy {
            Strategy::Parallel => Duration::ZERO,
            Strategy::Sequential | Strategy::Fastest => self.timeout,
            Strategy::Race | Strategy::Random => {
                let adaptive = Duration::from_micros((best_ewma_us * 2).max(20_000));
                adaptive.min(self.hedge_delay)
            }
        }
    }

    /// Resolve and return (response, winning upstream address).
    pub async fn resolve(&self, query: &[u8], stats: &Stats) -> Result<(Vec<u8>, String)> {
        self.queries.fetch_add(1, Ordering::Relaxed);
        let cands = self.candidates();
        if cands.is_empty() {
            bail!("group {}: no upstreams", self.name);
        }
        let max_attempts = cands.len().min(3);
        let hedge = self.hedge_for(cands[0].state.ewma_us_or_default());
        let overall_deadline = Instant::now() + self.timeout * 2;

        let mut inflight = FuturesUnordered::new();
        let mut next = 0usize;
        let spawn_next = |inflight: &mut FuturesUnordered<_>, next: &mut usize| {
            if *next < max_attempts {
                let u = cands[*next].clone();
                let q = query.to_vec();
                let per_attempt = (Instant::now() + self.timeout).min(overall_deadline);
                let (ft, rt, cd) = (self.fail_threshold, self.recover_threshold, self.cooldown);
                inflight.push(async move {
                    let addr = u.addr_str.clone();
                    u.query_measured(&q, per_attempt, ft, rt, cd)
                        .await
                        .map(|resp| (resp, addr))
                });
                *next += 1;
                true
            } else {
                false
            }
        };
        spawn_next(&mut inflight, &mut next);

        let mut last_err: Option<anyhow::Error> = None;
        loop {
            tokio::select! {
                biased;
                res = inflight.next() => {
                    match res {
                        Some(Ok(win)) => return Ok(win),
                        Some(Err(e)) => {
                            last_err = Some(e);
                            // a failed attempt immediately triggers the next hedge
                            if !spawn_next(&mut inflight, &mut next) && inflight.is_empty() {
                                break;
                            }
                        }
                        None => break, // all attempts done
                    }
                }
                _ = tokio::time::sleep(hedge), if next < max_attempts => {
                    if spawn_next(&mut inflight, &mut next) {
                        stats.hedged.fetch_add(1, Ordering::Relaxed);
                    }
                }
                _ = tokio::time::sleep_until(overall_deadline.into()) => {
                    bail!("group {}: query deadline exceeded", self.name);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("group {}: all upstreams failed", self.name)))
    }

    /// All upstreams incl. backups (for probing/stats).
    pub fn all_upstreams(&self) -> impl Iterator<Item = &Arc<Upstream>> {
        self.upstreams.iter().chain(self.backups.iter())
    }
}

/// Active health prober: keeps EWMA fresh, detects dead upstreams even when
/// idle, and closes/reopens circuit breakers. Also keeps TCP/TLS connections
/// warm so real queries don't pay handshake latency.
pub fn spawn_prober(groups: Vec<Arc<Group>>, fo: &FailoverCfg) {
    let interval = Duration::from_secs(fo.health_check_interval.max(2));
    let domain = fo.health_check_domain.clone();
    let (ft, rt) = (fo.fail_threshold.max(1), fo.recover_threshold.max(1));
    let cooldown = Duration::from_secs(fo.cooldown.max(1));
    tokio::spawn(async move {
        // spread first probes out a little
        tokio::time::sleep(Duration::from_millis(500)).await;
        loop {
            for g in &groups {
                // give probes the same budget as real queries; a hard 1.5s cap
                // flaps high-latency (international) upstreams on brief spikes
                let timeout = g.timeout;
                for u in g.all_upstreams() {
                    let u = u.clone();
                    let q = match crate::dns::build_query(&domain, crate::dns::TYPE_A, 1) {
                        Some(q) => q,
                        None => continue,
                    };
                    tokio::spawn(async move {
                        let deadline = Instant::now() + timeout;
                        let _ = u.query_measured(&q, deadline, ft, rt, cooldown).await;
                    });
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
}
