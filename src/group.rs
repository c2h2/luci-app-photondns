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
use crate::upstream::{RcodeError, Upstream};
use anyhow::{anyhow, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A failed hedged round. `answered` = at least one upstream returned a DNS
/// response (SERVFAIL/REFUSED) rather than timing out / being unreachable.
struct RoundFailed {
    err: anyhow::Error,
    answered: bool,
}

/// One upstream's answer in a `Group::compare` run.
pub struct CompareResult {
    pub upstream: String,
    pub backup: bool,
    /// got a DNS response (any rcode) before the deadline
    pub ok: bool,
    pub ms: f64,
    pub rcode: u8,
    pub answers: Vec<std::net::IpAddr>,
    pub error: String,
    /// the failover engine's smoothed latency for this upstream
    pub ewma_ms: f64,
    pub healthy: bool,
}

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
        let timeout = Duration::from_millis(g.timeout_ms.max(100));
        if g.so_mark != 0 {
            if cfg!(any(target_os = "linux", target_os = "android")) {
                log::info!("group {}: upstream sockets use fwmark {:#x}", g.name, g.so_mark);
            } else {
                log::warn!("group {}: so_mark is Linux-only, ignored", g.name);
            }
        }
        let mk = |addrs: &[String]| -> Result<Vec<Arc<Upstream>>> {
            addrs
                .iter()
                .map(|a| {
                    Upstream::new(
                        a,
                        g.insecure_skip_verify,
                        g.idle_timeout,
                        timeout,
                        bootstrap,
                        g.so_mark,
                    )
                })
                .collect()
        };
        Ok(Arc::new(Group {
            name: g.name.clone(),
            strategy: Strategy::parse(&g.strategy),
            upstreams: mk(&g.upstreams)?,
            backups: mk(&g.backups)?,
            hedge_delay: Duration::from_millis(g.hedge_delay_ms.max(1)),
            timeout,
            fail_threshold: fo.fail_threshold.max(1),
            recover_threshold: fo.recover_threshold.max(1),
            cooldown: Duration::from_secs(fo.cooldown.max(1)),
            queries: std::sync::atomic::AtomicU64::new(0),
        }))
    }

    /// Candidate order for this query: available primaries only. Backups
    /// (often a resolver in another "region", e.g. a China DNS behind an
    /// overseas group) never race the primaries; they are used only when
    /// every primary is down or a whole round of primaries timed out without
    /// any DNS answer (see `resolve`), so a name is not leaked to them just
    /// because a primary was slow or answered SERVFAIL.
    fn candidates(&self) -> Vec<Arc<Upstream>> {
        let mut avail: Vec<_> = self
            .upstreams
            .iter()
            .filter(|u| u.state.is_available())
            .cloned()
            .collect();
        if !avail.is_empty() {
            self.order(&mut avail);
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
    fn order(&self, v: &mut [Arc<Upstream>]) {
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
    /// If the whole first round fails fast (broken pipes, REFUSED, ...) one
    /// retry round runs within the same overall deadline. The retry adds the
    /// backups only when no primary produced any DNS answer (they all timed
    /// out or were unreachable); if a primary answered SERVFAIL/REFUSED the
    /// name itself is broken and asking a backup would just leak it.
    pub async fn resolve(&self, query: &[u8], stats: &Stats) -> Result<(Vec<u8>, String)> {
        self.queries.fetch_add(1, Ordering::Relaxed);
        let overall_deadline = Instant::now() + self.timeout * 2;
        match self
            .run_attempts(query, self.candidates(), overall_deadline, stats)
            .await
        {
            Ok(win) => Ok(win),
            Err(f) if Instant::now() < overall_deadline => {
                let pool: Vec<_> = if f.answered {
                    self.upstreams.to_vec()
                } else {
                    self.all_upstreams().cloned().collect()
                };
                let mut all: Vec<_> = pool
                    .iter()
                    .filter(|u| u.state.is_available())
                    .cloned()
                    .collect();
                if all.is_empty() {
                    all = pool;
                }
                all.sort_by_key(|u| u.state.ewma_us_or_default());
                log::debug!(
                    "group {}: first round failed ({}), retrying{}",
                    self.name,
                    f.err,
                    if f.answered { " primaries only" } else { " incl. backups" }
                );
                self.run_attempts(query, all, overall_deadline, stats)
                    .await
                    .map_err(|f| f.err)
            }
            Err(f) => Err(f.err),
        }
    }

    /// One hedged round over `cands`.
    async fn run_attempts(
        &self,
        query: &[u8],
        cands: Vec<Arc<Upstream>>,
        overall_deadline: Instant,
        stats: &Stats,
    ) -> std::result::Result<(Vec<u8>, String), RoundFailed> {
        let fail = |err, answered| RoundFailed { err, answered };
        if cands.is_empty() {
            return Err(fail(anyhow!("group {}: no upstreams", self.name), false));
        }
        let max_attempts = cands.len().min(3);
        let hedge = self.hedge_for(cands[0].state.ewma_us_or_default());

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
                    u.query_measured(&q, per_attempt, ft, rt, cd, false)
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
        // did any upstream answer at all (SERVFAIL/REFUSED)?
        let mut answered = false;
        loop {
            tokio::select! {
                biased;
                res = inflight.next() => {
                    match res {
                        Some(Ok(win)) => return Ok(win),
                        Some(Err(e)) => {
                            answered |= e.is::<RcodeError>();
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
                    return Err(fail(anyhow!("group {}: query deadline exceeded", self.name), answered));
                }
            }
        }
        Err(fail(
            last_err.unwrap_or_else(|| anyhow!("group {}: all upstreams failed", self.name)),
            answered,
        ))
    }

    /// Diagnostics: send `query` to every upstream (backups included) at
    /// once and report each one's own answer and latency. Bypasses health
    /// accounting so a manual comparison never trips the circuit breaker.
    pub async fn compare(&self, query: &[u8], question_end: usize) -> Vec<CompareResult> {
        let deadline = Instant::now() + self.timeout.min(Duration::from_secs(4));
        let n_primary = self.upstreams.len();
        let probes = self.all_upstreams().enumerate().map(|(i, u)| {
            let u = u.clone();
            async move {
                let start = Instant::now();
                let res = u.query(query, deadline).await;
                let ms = (start.elapsed().as_micros() as f64 / 10.0).round() / 100.0;
                let mut r = CompareResult {
                    upstream: u.addr_str.clone(),
                    backup: i >= n_primary,
                    ok: false,
                    ms,
                    rcode: 0,
                    answers: Vec::new(),
                    error: String::new(),
                    ewma_ms: u.state.ewma_ms(),
                    healthy: u.state.is_healthy(),
                };
                match res {
                    Ok(resp) => {
                        r.ok = true;
                        r.rcode = crate::dns::rcode(&resp);
                        r.answers = crate::dns::extract_ips(&resp, question_end);
                    }
                    Err(e) => r.error = e.to_string(),
                }
                r
            }
        });
        let mut out = futures::future::join_all(probes).await;
        out.sort_by(|a, b| b.ok.cmp(&a.ok).then(a.ms.total_cmp(&b.ms)));
        out
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
                        let _ = u.query_measured(&q, deadline, ft, rt, cooldown, true).await;
                    });
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns;
    use std::sync::atomic::AtomicUsize;
    use tokio::net::UdpSocket;

    #[derive(Clone, Copy)]
    enum Peer {
        /// answer NOERROR with 1.2.3.4
        Ok,
        /// answer SERVFAIL for names containing "broken", else NOERROR
        ServfailBroken,
        /// never answer
        Silent,
    }

    /// Fake UDP resolver; returns its address and a query counter.
    async fn peer(kind: Peer) -> (String, Arc<AtomicUsize>) {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else { return };
                h.fetch_add(1, Ordering::SeqCst);
                let q = &buf[..n];
                let Some(meta) = dns::parse_query(q) else { continue };
                let resp = match kind {
                    Peer::Silent => continue,
                    Peer::ServfailBroken if meta.qname.contains("broken") => {
                        dns::build_reply(q, meta.question_end, dns::RCODE_SERVFAIL)
                    }
                    _ => dns::build_ip_reply(q, &meta, &["1.2.3.4".parse().unwrap()], 60),
                };
                let _ = sock.send_to(&resp, from).await;
            }
        });
        (format!("udp://{}", addr), hits)
    }

    fn group(strategy: &str, primaries: Vec<String>, backups: Vec<String>) -> Arc<Group> {
        let cfg = GroupCfg {
            name: "main".into(),
            strategy: strategy.into(),
            upstreams: primaries,
            backups,
            hedge_delay_ms: 20,
            timeout_ms: 300,
            insecure_skip_verify: false,
            bootstrap: "127.0.0.1:53".into(),
            idle_timeout: 30,
            so_mark: 0,
        };
        let fo: FailoverCfg = toml::from_str("").unwrap();
        Group::from_config(&cfg, &fo).unwrap()
    }

    fn q(name: &str) -> Vec<u8> {
        dns::build_query(name, dns::TYPE_A, 1).unwrap()
    }

    #[tokio::test]
    async fn servfail_names_never_reach_backup_nor_mark_primaries_down() {
        for strategy in ["parallel", "race", "sequential"] {
            let (p1, _) = peer(Peer::ServfailBroken).await;
            let (p2, _) = peer(Peer::ServfailBroken).await;
            let (b, backup_hits) = peer(Peer::Ok).await;
            let g = group(strategy, vec![p1, p2], vec![b]);
            let stats = Stats::new();
            for i in 0..6 {
                let r = g.resolve(&q(&format!("x{}.broken.example", i)), &stats).await;
                assert!(r.is_err(), "{}: SERVFAIL everywhere must fail", strategy);
            }
            assert_eq!(backup_hits.load(Ordering::SeqCst), 0, "{}: backup leaked", strategy);
            for u in &g.upstreams {
                assert!(u.state.is_healthy(), "{}: {} wrongly marked down", strategy, u.addr_str);
            }
            // and the group still answers normal names from primaries
            let (_, winner) = g.resolve(&q("www.example.com"), &stats).await.unwrap();
            assert!(g.upstreams.iter().any(|u| u.addr_str == winner), "{}", strategy);
        }
    }

    #[tokio::test]
    async fn healthy_primaries_never_hedge_to_backup() {
        for strategy in ["race", "fastest", "parallel", "sequential", "random"] {
            let (p, _) = peer(Peer::Ok).await;
            let (b, backup_hits) = peer(Peer::Ok).await;
            let g = group(strategy, vec![p], vec![b]);
            let stats = Stats::new();
            for i in 0..5 {
                g.resolve(&q(&format!("n{}.example.com", i)), &stats).await.unwrap();
            }
            assert_eq!(backup_hits.load(Ordering::SeqCst), 0, "{}: backup used", strategy);
        }
    }

    #[tokio::test]
    async fn timed_out_primaries_fall_back_to_backup() {
        let (p, _) = peer(Peer::Silent).await;
        let (b, backup_hits) = peer(Peer::Ok).await;
        let g = group("race", vec![p], vec![b.clone()]);
        let stats = Stats::new();
        let (_, winner) = g.resolve(&q("www.example.com"), &stats).await.unwrap();
        assert_eq!(winner, b, "timeout must fall back to the backup");
        assert!(backup_hits.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn timeout_is_not_counted_while_upstream_answers_others() {
        // an upstream that answers normal names but hangs on one name
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else { return };
                let q = &buf[..n];
                let Some(meta) = dns::parse_query(q) else { continue };
                if meta.qname.contains("lame") {
                    continue;
                }
                let resp = dns::build_ip_reply(q, &meta, &["1.2.3.4".parse().unwrap()], 60);
                let _ = sock.send_to(&resp, from).await;
            }
        });
        let g = group("race", vec![format!("udp://{}", addr)], vec![]);
        let stats = Stats::new();
        for i in 0..4 {
            let lq = q(&format!("x{}.lame.example", i));
            let oq = q(&format!("ok{}.example.com", i));
            let lame = g.resolve(&lq, &stats);
            let ok = async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                g.resolve(&oq, &stats).await
            };
            let (l, o) = tokio::join!(lame, ok);
            assert!(l.is_err());
            assert!(o.is_ok());
        }
        assert!(g.upstreams[0].state.is_healthy(), "lame name must not take it down");
    }
}
