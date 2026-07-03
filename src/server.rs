//! Listener + request pipeline:
//! hosts/block/redirect -> cache (fresh/stale) -> group failover -> cache fill.

use crate::cache::{self, CacheEntry, DnsCache, Freshness};
use crate::config::Config;
use crate::dns;
use crate::group::Group;
use crate::router::{Decision, Router};
use crate::stats::Stats;
use crate::qlog::QueryLog;
use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;

pub struct Ctx {
    pub cfg: Config,
    pub cache: Option<Arc<DnsCache>>,
    pub router: Router,
    pub groups: Vec<Arc<Group>>,
    pub stats: Arc<Stats>,
    pub refresh_tx: mpsc::Sender<RefreshJob>,
    pub qlog: Arc<QueryLog>,
}

pub struct RefreshJob {
    key: cache::CacheKey,
    qname: String,
    qtype: u16,
    qclass: u16,
}

impl Ctx {
    pub fn group(&self, name: &str) -> &Arc<Group> {
        self.groups
            .iter()
            .find(|g| g.name == name)
            .unwrap_or(&self.groups[0])
    }

    fn group_for(&self, qname: &str, qtype: u16) -> &Arc<Group> {
        match self.router.decide(qname, qtype) {
            Decision::Forward(name) => self.group(name),
            _ => self.group("main"),
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Clamp TTLs, extract cache metadata and store the response.
fn cache_store(ctx: &Ctx, key: cache::CacheKey, resp: &mut Vec<u8>, question_end: usize) {
    let Some(cache) = &ctx.cache else { return };
    dns::clamp_ttls(resp, question_end, ctx.cfg.cache.min_ttl, ctx.cfg.cache.max_ttl);
    let Some(info) = dns::cache_info(resp, question_end, ctx.cfg.cache.negative_ttl) else {
        return;
    };
    if info.ttl == 0 {
        return;
    }
    let stale_ttl = if ctx.cfg.cache.serve_stale || ctx.cfg.cache.stale_ttl > 0 {
        ctx.cfg.cache.stale_ttl
    } else {
        0
    };
    cache.insert(
        key,
        CacheEntry {
            data: resp.clone(),
            ttl_offsets: info.ttl_offsets.into_boxed_slice(),
            question_len: (question_end - dns::HEADER_LEN) as u16,
            stored_at: Instant::now(),
            ttl: info.ttl,
            stale_ttl,
            hits: AtomicU32::new(0),
            refreshing: AtomicBool::new(false),
            stored_unix: unix_now(),
        },
    );
}

/// Forward to the right group and fill the cache. Returns the response with
/// the *client's* original ID restored, plus (group name, winning upstream).
async fn resolve_upstream(
    ctx: &Arc<Ctx>,
    query: &[u8],
    meta: &dns::QueryMeta,
    key: &cache::CacheKey,
) -> Result<(Vec<u8>, String, String)> {
    let group = ctx.group_for(&meta.qname, meta.qtype);
    let (mut resp, winner) = group.resolve(query, &ctx.stats).await?;
    // sanity: response must echo our question (anti-spoofing / bug guard)
    match dns::parse_query(&resp) {
        Some(rmeta) if rmeta.qname == meta.qname && rmeta.qtype == meta.qtype => {}
        _ => anyhow::bail!("upstream response question mismatch"),
    }
    cache_store(ctx, key.clone(), &mut resp, meta.question_end);
    dns::set_id(&mut resp, meta.id);
    Ok((resp, group.name.clone(), winner))
}

/// Trigger a background refresh unless one is already running for the entry.
fn maybe_refresh(ctx: &Arc<Ctx>, entry: &CacheEntry, meta: &dns::QueryMeta, key: &cache::CacheKey) {
    if entry
        .refreshing
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        let _ = ctx.refresh_tx.try_send(RefreshJob {
            key: key.clone(),
            qname: meta.qname.clone(),
            qtype: meta.qtype,
            qclass: meta.qclass,
        });
    }
}

/// The full pipeline. Returns None for packets that must be dropped.
pub async fn handle_query(
    ctx: &Arc<Ctx>,
    query: &[u8],
    via_tcp: bool,
    client: IpAddr,
) -> Option<Vec<u8>> {
    if query.len() < dns::HEADER_LEN || dns::is_response(query) {
        return None;
    }
    let start = Instant::now();
    let stats = &ctx.stats;
    stats.total.fetch_add(1, Ordering::Relaxed);
    stats.rate.incr();
    if via_tcp {
        stats.tcp.fetch_add(1, Ordering::Relaxed);
    } else {
        stats.udp.fetch_add(1, Ordering::Relaxed);
    }

    let Some(meta) = dns::parse_query(query) else {
        let mut r = query[..dns::HEADER_LEN].to_vec();
        r[2] = 0x80 | (r[2] & 0x79);
        r[3] = 0x80 | dns::RCODE_FORMERR;
        return Some(r);
    };

    let qlog = |route: &str, upstream: &str| {
        ctx.qlog
            .record(client, &meta.qname, meta.qtype, route, upstream, start.elapsed());
    };

    // routing decisions that answer locally
    match ctx.router.decide(&meta.qname, meta.qtype) {
        Decision::Hosts(ips) => {
            stats.hosts_served.fetch_add(1, Ordering::Relaxed);
            let r = dns::build_ip_reply(query, &meta, ips, ctx.router.hosts_ttl);
            qlog("hosts", "");
            return Some(r);
        }
        Decision::Block => {
            stats.blocked.fetch_add(1, Ordering::Relaxed);
            qlog("blocked", "");
            return Some(dns::build_reply(query, meta.question_end, dns::RCODE_NXDOMAIN));
        }
        Decision::Redirect(target) => {
            stats.redirected.fetch_add(1, Ordering::Relaxed);
            let target = target.to_string();
            let r = resolve_redirect(ctx, query, &meta, &target).await;
            qlog("redirect", &target);
            return Some(r);
        }
        Decision::Forward(_) => {}
    }

    let key = cache::make_key(&meta.qname, meta.qtype, meta.qclass);

    // cache lookup
    if let Some(cache) = &ctx.cache {
        let now = Instant::now();
        if let Some((entry, freshness)) = cache.get(&key, now) {
            match freshness {
                Freshness::Fresh { remaining } => {
                    stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                    if ctx.cfg.cache.prefetch
                        && remaining <= ctx.cfg.cache.prefetch_margin
                        && entry.hits.load(Ordering::Relaxed) >= ctx.cfg.cache.prefetch_min_hits
                    {
                        stats.prefetches.fetch_add(1, Ordering::Relaxed);
                        maybe_refresh(ctx, &entry, &meta, &key);
                    }
                    qlog("cache", "");
                    return Some(finish(&entry.make_response(query, &meta, now), query, &meta, via_tcp));
                }
                Freshness::Stale if ctx.cfg.cache.serve_stale => {
                    stats.stale_served.fetch_add(1, Ordering::Relaxed);
                    maybe_refresh(ctx, &entry, &meta, &key);
                    qlog("stale", "");
                    return Some(finish(&entry.make_response(query, &meta, now), query, &meta, via_tcp));
                }
                Freshness::Stale => {
                    // serve-stale disabled: try upstream, fall back to stale on failure
                    stats.cache_misses.fetch_add(1, Ordering::Relaxed);
                    return match resolve_upstream(ctx, query, &meta, &key).await {
                        Ok((resp, group, winner)) => {
                            qlog(&group, &winner);
                            Some(finish(&resp, query, &meta, via_tcp))
                        }
                        Err(_) => {
                            stats.stale_served.fetch_add(1, Ordering::Relaxed);
                            qlog("stale", "");
                            Some(finish(&entry.make_response(query, &meta, now), query, &meta, via_tcp))
                        }
                    };
                }
            }
        }
        stats.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    // miss -> upstream with failover
    match resolve_upstream(ctx, query, &meta, &key).await {
        Ok((resp, group, winner)) => {
            qlog(&group, &winner);
            Some(finish(&resp, query, &meta, via_tcp))
        }
        Err(e) => {
            log::debug!("resolve {} failed: {}", meta.qname, e);
            ctx.stats.upstream_errors.fetch_add(1, Ordering::Relaxed);
            ctx.stats.servfail.fetch_add(1, Ordering::Relaxed);
            qlog("servfail", "");
            Some(dns::build_reply(query, meta.question_end, dns::RCODE_SERVFAIL))
        }
    }
}

/// Resolve a redirected name and answer the client with its addresses.
async fn resolve_redirect(
    ctx: &Arc<Ctx>,
    query: &[u8],
    meta: &dns::QueryMeta,
    target: &str,
) -> Vec<u8> {
    if meta.qtype != dns::TYPE_A && meta.qtype != dns::TYPE_AAAA {
        return dns::build_reply(query, meta.question_end, dns::RCODE_NOERROR);
    }
    let Some(tq) = dns::build_query(target, meta.qtype, meta.qclass) else {
        return dns::build_reply(query, meta.question_end, dns::RCODE_SERVFAIL);
    };
    let Some(tmeta) = dns::parse_query(&tq) else {
        return dns::build_reply(query, meta.question_end, dns::RCODE_SERVFAIL);
    };
    let tkey = cache::make_key(&tmeta.qname, tmeta.qtype, tmeta.qclass);

    // reuse cached target if possible
    let cached = ctx.cache.as_ref().and_then(|c| {
        c.get(&tkey, Instant::now())
            .filter(|(_, f)| matches!(f, Freshness::Fresh { .. }))
            .map(|(e, _)| e.data.clone())
    });
    let tresp = match cached {
        Some(r) => r,
        None => match resolve_upstream(ctx, &tq, &tmeta, &tkey).await {
            Ok((r, _, _)) => r,
            Err(_) => return dns::build_reply(query, meta.question_end, dns::RCODE_SERVFAIL),
        },
    };
    let ips = dns::extract_ips(&tresp, tmeta.question_end);
    let ttl = dns::min_answer_ttl(&tresp, tmeta.question_end, ctx.router.hosts_ttl);
    dns::build_ip_reply(query, meta, &ips, ttl)
}

/// UDP size guard: replace oversized UDP answers with a TC probe.
fn finish(resp: &[u8], query: &[u8], meta: &dns::QueryMeta, via_tcp: bool) -> Vec<u8> {
    if !via_tcp && resp.len() > meta.udp_size as usize {
        return dns::build_truncated(query, meta.question_end);
    }
    resp.to_vec()
}

// ------------------------------------------------------------- listeners

fn make_udp_socket(addr: SocketAddr, reuse_port: bool) -> Result<std::net::UdpSocket> {
    let domain = if addr.is_ipv4() {
        socket2::Domain::IPV4
    } else {
        socket2::Domain::IPV6
    };
    let sock = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if reuse_port {
        sock.set_reuse_port(true)?;
    }
    let _ = reuse_port;
    sock.set_recv_buffer_size(1 << 20).ok();
    sock.set_send_buffer_size(1 << 20).ok();
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    Ok(sock.into())
}

pub async fn run_udp(ctx: Arc<Ctx>, addr: SocketAddr) -> Result<()> {
    let n = match ctx.cfg.server.udp_sockets {
        0 => {
            if cfg!(target_os = "linux") {
                std::thread::available_parallelism().map(|p| p.get()).unwrap_or(2).min(4)
            } else {
                1
            }
        }
        n => n.min(16),
    };
    for i in 0..n {
        let sock = make_udp_socket(addr, n > 1)
            .with_context(|| format!("bind udp {}", addr))?;
        let sock = Arc::new(UdpSocket::from_std(sock)?);
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                match sock.recv_from(&mut buf).await {
                    Ok((len, peer)) => {
                        let query = buf[..len].to_vec();
                        let ctx = ctx.clone();
                        let sock = sock.clone();
                        tokio::spawn(async move {
                            if let Some(resp) = handle_query(&ctx, &query, false, peer.ip()).await {
                                let _ = sock.send_to(&resp, peer).await;
                            }
                        });
                    }
                    Err(e) => {
                        log::debug!("udp recv error: {}", e);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        });
        if i == 0 {
            log::info!("UDP listening on {} ({} sockets)", addr, n);
        }
    }
    Ok(())
}

pub async fn run_tcp(ctx: Arc<Ctx>, addr: SocketAddr) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind tcp {}", addr))?;
    log::info!("TCP listening on {}", addr);
    let idle = Duration::from_secs(ctx.cfg.server.tcp_idle_timeout.max(5));
    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            };
            let ctx = ctx.clone();
            tokio::spawn(async move {
                stream.set_nodelay(true).ok();
                let (mut rd, wr) = stream.into_split();
                let wr = Arc::new(tokio::sync::Mutex::new(wr));
                loop {
                    let mut lenbuf = [0u8; 2];
                    match tokio::time::timeout(idle, rd.read_exact(&mut lenbuf)).await {
                        Ok(Ok(_)) => {}
                        _ => break,
                    }
                    let len = u16::from_be_bytes(lenbuf) as usize;
                    if len < dns::HEADER_LEN {
                        break;
                    }
                    let mut qbuf = vec![0u8; len];
                    if rd.read_exact(&mut qbuf).await.is_err() {
                        break;
                    }
                    // pipelined: answer out of order as results arrive
                    let ctx = ctx.clone();
                    let wr = wr.clone();
                    tokio::spawn(async move {
                        if let Some(resp) = handle_query(&ctx, &qbuf, true, peer.ip()).await {
                            let mut frame = Vec::with_capacity(resp.len() + 2);
                            frame.extend_from_slice(&(resp.len() as u16).to_be_bytes());
                            frame.extend_from_slice(&resp);
                            let mut w = wr.lock().await;
                            let _ = w.write_all(&frame).await;
                        }
                    });
                }
            });
        }
    });
    Ok(())
}

/// Background refresh worker (prefetch + serve-stale updates).
pub fn spawn_refresher(ctx: Arc<Ctx>, mut rx: mpsc::Receiver<RefreshJob>) {
    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                let Some(q) = dns::build_query(&job.qname, job.qtype, job.qclass) else {
                    return;
                };
                let group = ctx.group_for(&job.qname, job.qtype);
                match group.resolve(&q, &ctx.stats).await {
                    Ok((mut resp, _winner)) => {
                        // find question_end of the *response*
                        if let Some(rmeta) = dns::parse_query(&resp) {
                            if rmeta.qname == job.qname {
                                cache_store(&ctx, job.key.clone(), &mut resp, rmeta.question_end);
                                log::debug!("refreshed {}", job.qname);
                                return;
                            }
                        }
                        clear_refreshing(&ctx, &job.key);
                    }
                    Err(e) => {
                        log::debug!("refresh {} failed: {}", job.qname, e);
                        clear_refreshing(&ctx, &job.key);
                    }
                }
            });
        }
    });
}

fn clear_refreshing(ctx: &Arc<Ctx>, key: &cache::CacheKey) {
    if let Some(cache) = &ctx.cache {
        if let Some((entry, _)) = cache.get(key, Instant::now()) {
            entry.refreshing.store(false, Ordering::Release);
        }
    }
}
