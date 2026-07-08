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

/// How the client query arrived. Only UDP answers are size-limited (TC).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transport {
    Udp,
    Tcp,
    Doh,
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
/// Returns false when the response is not cacheable (nothing was stored).
fn cache_store(ctx: &Ctx, key: cache::CacheKey, resp: &mut Vec<u8>, question_end: usize) -> bool {
    let Some(cache) = &ctx.cache else { return false };
    dns::clamp_ttls(resp, question_end, ctx.cfg.cache.min_ttl, ctx.cfg.cache.max_ttl);
    let Some(info) = dns::cache_info(resp, question_end, ctx.cfg.cache.negative_ttl) else {
        return false;
    };
    if info.ttl == 0 {
        return false;
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
    true
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

/// For AAAA-mode `block_if_ipv4`: does `qname` have any A (IPv4) record?
/// Checks the cache first (the client's parallel A query is usually already
/// there), else does one upstream A lookup and caches it so that parallel A
/// query is then free. On any lookup error we return `false` (fail open:
/// do not suppress AAAA when we cannot confirm IPv4 exists).
async fn name_has_ipv4(ctx: &Arc<Ctx>, meta: &dns::QueryMeta) -> bool {
    let key = cache::make_key(&meta.qname, dns::TYPE_A, meta.qclass);
    if let Some(cache) = &ctx.cache {
        if let Some((entry, _freshness)) = cache.get(&key, Instant::now()) {
            let qend = dns::HEADER_LEN + entry.question_len as usize;
            return !dns::extract_ips(&entry.data, qend).is_empty();
        }
    }
    let Some(aq) = dns::build_query(&meta.qname, dns::TYPE_A, meta.qclass) else {
        return false;
    };
    let ameta = match dns::parse_query(&aq) {
        Some(m) => m,
        None => return false,
    };
    match resolve_upstream(ctx, &aq, &ameta, &key).await {
        Ok((resp, _, _)) => !dns::extract_ips(&resp, ameta.question_end).is_empty(),
        Err(_) => false,
    }
}

/// Trigger a background refresh unless one is already running for the entry.
fn maybe_refresh(ctx: &Arc<Ctx>, entry: &CacheEntry, meta: &dns::QueryMeta, key: &cache::CacheKey) {
    if entry
        .refreshing
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        let sent = ctx.refresh_tx.try_send(RefreshJob {
            key: key.clone(),
            qname: meta.qname.clone(),
            qtype: meta.qtype,
            qclass: meta.qclass,
        });
        if sent.is_err() {
            // queue full: release the flag or the entry can never refresh again
            entry.refreshing.store(false, Ordering::Release);
        }
    }
}

/// The full pipeline. Returns None for packets that must be dropped.
pub async fn handle_query(
    ctx: &Arc<Ctx>,
    query: &[u8],
    transport: Transport,
    client: IpAddr,
) -> Option<Vec<u8>> {
    if query.len() < dns::HEADER_LEN || dns::is_response(query) {
        return None;
    }
    let start = Instant::now();
    let stats = &ctx.stats;
    stats.total.fetch_add(1, Ordering::Relaxed);
    stats.rate.incr();
    match transport {
        Transport::Udp => stats.udp.fetch_add(1, Ordering::Relaxed),
        Transport::Tcp => stats.tcp.fetch_add(1, Ordering::Relaxed),
        Transport::Doh => stats.doh.fetch_add(1, Ordering::Relaxed),
    };

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

    // IPv6 (AAAA) policy: optionally answer AAAA with an empty NOERROR so
    // clients fall back to IPv4. `block_all` always suppresses; `block_if_ipv4`
    // suppresses only when the name also has an A record (IPv6-only names still
    // resolve). An empty NOERROR (not NXDOMAIN) is the correct "no IPv6 here".
    if meta.qtype == dns::TYPE_AAAA {
        let suppress = match ctx.cfg.routing.aaaa_mode.as_str() {
            "block_all" => true,
            "block_if_ipv4" => name_has_ipv4(ctx, &meta).await,
            _ => false,
        };
        if suppress {
            stats.blocked.fetch_add(1, Ordering::Relaxed);
            qlog("aaaa-blocked", "");
            return Some(dns::build_reply(query, meta.question_end, dns::RCODE_NOERROR));
        }
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
                    return Some(finish(entry.make_response(query, &meta, now, ctx.cfg.cache.stale_client_ttl), query, &meta, transport));
                }
                Freshness::Stale if ctx.cfg.cache.serve_stale => {
                    stats.stale_served.fetch_add(1, Ordering::Relaxed);
                    maybe_refresh(ctx, &entry, &meta, &key);
                    qlog("stale", "");
                    return Some(finish(entry.make_response(query, &meta, now, ctx.cfg.cache.stale_client_ttl), query, &meta, transport));
                }
                Freshness::Stale => {
                    // serve-stale disabled: try upstream, fall back to stale on failure
                    stats.cache_misses.fetch_add(1, Ordering::Relaxed);
                    return match resolve_upstream(ctx, query, &meta, &key).await {
                        Ok((resp, group, winner)) => {
                            qlog(&group, &winner);
                            Some(finish(resp, query, &meta, transport))
                        }
                        Err(_) => {
                            stats.stale_served.fetch_add(1, Ordering::Relaxed);
                            qlog("stale", "");
                            Some(finish(entry.make_response(query, &meta, now, ctx.cfg.cache.stale_client_ttl), query, &meta, transport))
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
            Some(finish(resp, query, &meta, transport))
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

/// Structured result of a diagnostic resolve (the `/resolve` API + test page).
pub struct ResolveResult {
    pub route: String,
    pub upstream: String,
    pub rcode: u8,
    pub ips: Vec<std::net::IpAddr>,
    pub ttl: u32,
    pub elapsed: Duration,
}

/// Resolve a name through the real pipeline (router -> cache -> failover) and
/// return structured diagnostics instead of wire bytes. Used by the HTTP
/// `/resolve` endpoint so a browser can get dig-like output: the answer, which
/// route served it, the winning upstream, and how long it took.
///
/// Unlike `handle_query` this never touches the query log or client-facing
/// stats counters beyond what `resolve_upstream` already does; it is a
/// read/diagnose path, not the hot serving path.
pub async fn resolve_named(ctx: &Arc<Ctx>, name: &str, qtype: u16) -> Result<ResolveResult> {
    let start = Instant::now();
    let qclass = dns::CLASS_IN;
    let query = dns::build_query(name, qtype, qclass).context("invalid query name")?;
    let meta = dns::parse_query(&query).context("could not parse built query")?;

    let mk = |route: &str, upstream: &str, resp: &[u8]| ResolveResult {
        route: route.to_string(),
        upstream: upstream.to_string(),
        rcode: dns::rcode(resp),
        ips: dns::extract_ips(resp, meta.question_end),
        ttl: dns::min_answer_ttl(resp, meta.question_end, 0),
        elapsed: start.elapsed(),
    };

    // local routing decisions
    match ctx.router.decide(&meta.qname, meta.qtype) {
        Decision::Hosts(ips) => {
            let r = dns::build_ip_reply(&query, &meta, ips, ctx.router.hosts_ttl);
            return Ok(mk("hosts", "", &r));
        }
        Decision::Block => {
            let r = dns::build_reply(&query, meta.question_end, dns::RCODE_NXDOMAIN);
            return Ok(mk("blocked", "", &r));
        }
        Decision::Redirect(target) => {
            let target = target.to_string();
            let r = resolve_redirect(ctx, &query, &meta, &target).await;
            return Ok(mk("redirect", &target, &r));
        }
        Decision::Forward(_) => {}
    }

    let key = cache::make_key(&meta.qname, meta.qtype, meta.qclass);

    if let Some(cache) = &ctx.cache {
        let now = Instant::now();
        if let Some((entry, freshness)) = cache.get(&key, now) {
            match freshness {
                Freshness::Fresh { .. } => {
                    let r = entry.make_response(&query, &meta, now, ctx.cfg.cache.stale_client_ttl);
                    return Ok(mk("cache", "", &r));
                }
                Freshness::Stale if ctx.cfg.cache.serve_stale => {
                    let r = entry.make_response(&query, &meta, now, ctx.cfg.cache.stale_client_ttl);
                    return Ok(mk("stale", "", &r));
                }
                Freshness::Stale => {}
            }
        }
    }

    let (resp, group, winner) = resolve_upstream(ctx, &query, &meta, &key).await?;
    Ok(mk(&group, &winner, &resp))
}

/// UDP size guard: replace oversized UDP answers with a TC probe. Takes the
/// response by value and returns it unchanged in the common case, so the hot
/// path never copies the whole packet a second time.
fn finish(resp: Vec<u8>, query: &[u8], meta: &dns::QueryMeta, transport: Transport) -> Vec<u8> {
    if transport == Transport::Udp && resp.len() > meta.udp_size as usize {
        return dns::build_truncated(query, meta.question_end);
    }
    resp
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
        // Linux only: opt-in recvmmsg/sendmmsg batching via PHOTONDNS_UDP_BATCH=1.
        // Measured on x86_64 Linux it gives ~no throughput gain (recvmmsg does
        // batch ~6 datagrams/call, but the client-facing syscalls are only ~30%
        // of the total — the unbatched upstream forward recvfrom/sendto dominate,
        // and Linux syscalls are cheap). Kept behind the flag for kernels/NICs
        // where it may help under overload; DEFAULT is the portable per-packet
        // loop so the hot path carries no unsafe FFI unless asked for.
        #[cfg(target_os = "linux")]
        if std::env::var("PHOTONDNS_UDP_BATCH").as_deref() == Ok("1") {
            udp_batch::spawn(ctx.clone(), sock);
        } else {
            udp_per_packet::spawn(ctx.clone(), Arc::new(UdpSocket::from_std(sock)?));
        }
        #[cfg(not(target_os = "linux"))]
        udp_per_packet::spawn(ctx.clone(), Arc::new(UdpSocket::from_std(sock)?));
        if i == 0 {
            log::info!("UDP listening on {} ({} sockets)", addr, n);
        }
    }
    Ok(())
}

/// Portable one-syscall-per-packet UDP serving (macOS/BSD; also the Linux A/B
/// baseline via PHOTONDNS_UDP_BATCH=0). Reads a datagram, spawns the pipeline,
/// sends the answer back.
mod udp_per_packet {
    use super::*;
    pub fn spawn(ctx: Arc<Ctx>, sock: Arc<UdpSocket>) {
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                match sock.recv_from(&mut buf).await {
                    Ok((len, peer)) => {
                        let query = buf[..len].to_vec();
                        let ctx = ctx.clone();
                        let sock = sock.clone();
                        tokio::spawn(async move {
                            if let Some(resp) =
                                handle_query(&ctx, &query, Transport::Udp, peer.ip()).await
                            {
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
    }
}

/// Linux batched UDP serving. `recvmmsg` pulls up to BATCH queries in one
/// syscall; a dedicated sender task coalesces ready answers into `sendmmsg`
/// batches. Each datagram still flows through `handle_query` unchanged — this
/// only cuts the two syscalls/query a forwarder pays down toward ~2/batch.
#[cfg(target_os = "linux")]
mod udp_batch {
    use super::*;
    use std::os::unix::io::AsRawFd;
    use tokio::io::unix::AsyncFd;

    const BATCH: usize = 32;
    const BUFSZ: usize = 2048;

    pub fn spawn(ctx: Arc<Ctx>, std_sock: std::net::UdpSocket) {
        let afd = match AsyncFd::new(std_sock) {
            Ok(a) => Arc::new(a),
            Err(e) => {
                log::error!("udp AsyncFd: {}", e);
                return;
            }
        };
        // Answers flow to a single sender task per socket, which sendmmsg's them.
        let (tx, rx) = mpsc::unbounded_channel::<(Vec<u8>, SocketAddr)>();
        spawn_sender(afd.clone(), rx);

        tokio::spawn(async move {
            let fd = afd.get_ref().as_raw_fd();
            let mut rb = RecvBatch::new();
            loop {
                let mut guard = match afd.readable().await {
                    Ok(g) => g,
                    Err(e) => {
                        log::error!("udp readable: {}", e);
                        return;
                    }
                };
                match guard.try_io(|_| rb.recv(fd)) {
                    Ok(Ok(count)) => {
                        for i in 0..count {
                            if let Some((query, peer)) = rb.message(i) {
                                let ctx = ctx.clone();
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    if let Some(resp) =
                                        handle_query(&ctx, &query, Transport::Udp, peer.ip()).await
                                    {
                                        let _ = tx.send((resp, peer));
                                    }
                                });
                            }
                        }
                    }
                    Ok(Err(e)) => log::debug!("recvmmsg: {}", e),
                    Err(_would_block) => {} // readiness cleared by try_io
                }
            }
        });
    }

    /// Reusable receive scratch. Holds ONLY `Send` data (byte buffers, address
    /// storage, per-slot lengths) so the receive future stays `Send` for
    /// `tokio::spawn`. The `iovec`/`mmsghdr` arrays that carry raw pointers are
    /// built on the stack inside `recv()` and never live across an `.await`.
    struct RecvBatch {
        bufs: Vec<[u8; BUFSZ]>,
        addrs: Vec<libc::sockaddr_storage>,
        lens: Vec<usize>,
        namelens: Vec<libc::socklen_t>,
    }

    impl RecvBatch {
        fn new() -> Self {
            RecvBatch {
                bufs: vec![[0u8; BUFSZ]; BATCH],
                addrs: vec![unsafe { std::mem::zeroed() }; BATCH],
                lens: vec![0usize; BATCH],
                namelens: vec![0; BATCH],
            }
        }

        /// One `recvmmsg`. Returns the number of datagrams received (EAGAIN
        /// surfaces as Err so the caller clears readiness). All libc structs are
        /// local — nothing pointer-bearing is stored or held across an await.
        fn recv(&mut self, fd: i32) -> std::io::Result<usize> {
            let mut iovecs: [libc::iovec; BATCH] = unsafe { std::mem::zeroed() };
            let mut msgs: [libc::mmsghdr; BATCH] = unsafe { std::mem::zeroed() };
            let bufp = self.bufs.as_mut_ptr();
            let addrp = self.addrs.as_mut_ptr();
            for i in 0..BATCH {
                unsafe {
                    iovecs[i].iov_base = (*bufp.add(i)).as_mut_ptr() as *mut libc::c_void;
                    iovecs[i].iov_len = BUFSZ as _;
                    let h = &mut msgs[i].msg_hdr;
                    h.msg_name = addrp.add(i) as *mut libc::c_void;
                    h.msg_namelen = std::mem::size_of::<libc::sockaddr_storage>() as _;
                    h.msg_iov = &mut iovecs[i] as *mut libc::iovec;
                    h.msg_iovlen = 1 as _;
                }
            }
            let n = unsafe {
                // `as _` on the flags: musl types recvmmsg's flags as c_uint,
                // glibc as c_int, so the constant must adapt to the target.
                libc::recvmmsg(
                    fd,
                    msgs.as_mut_ptr(),
                    BATCH as _,
                    libc::MSG_DONTWAIT as _,
                    std::ptr::null_mut(),
                )
            };
            if n < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let n = n as usize;
            for i in 0..n {
                self.lens[i] = msgs[i].msg_len as usize;
                self.namelens[i] = msgs[i].msg_hdr.msg_namelen;
            }
            Ok(n)
        }

        /// Extract datagram `i`: its bytes (copied, since the handler owns them
        /// across awaits) and the peer address.
        fn message(&self, i: usize) -> Option<(Vec<u8>, SocketAddr)> {
            let len = self.lens[i];
            if len < dns::HEADER_LEN || len > BUFSZ {
                return None;
            }
            let sa = unsafe { socket2::SockAddr::new(self.addrs[i], self.namelens[i]) };
            let peer = sa.as_socket()?;
            Some((self.bufs[i][..len].to_vec(), peer))
        }
    }

    /// Drains ready answers and ships them in `sendmmsg` batches.
    fn spawn_sender(
        afd: Arc<AsyncFd<std::net::UdpSocket>>,
        mut rx: mpsc::UnboundedReceiver<(Vec<u8>, SocketAddr)>,
    ) {
        tokio::spawn(async move {
            let fd = afd.get_ref().as_raw_fd();
            let mut out: Vec<(Vec<u8>, socket2::SockAddr)> = Vec::with_capacity(BATCH);
            while let Some((buf, peer)) = rx.recv().await {
                out.push((buf, peer.into()));
                // opportunistically coalesce whatever else is already queued
                while out.len() < BATCH {
                    match rx.try_recv() {
                        Ok((b, p)) => out.push((b, p.into())),
                        Err(_) => break,
                    }
                }
                send_all(&afd, fd, &out).await;
                out.clear();
            }
        });
    }

    /// sendmmsg the whole slice, awaiting writable readiness and handling
    /// partial sends. The pointer-bearing libc structs are built inside the
    /// synchronous `sendmmsg_once` (never across an await), so this future stays
    /// `Send`. Datagrams dropped on a hard error are just lost (UDP).
    async fn send_all(
        afd: &AsyncFd<std::net::UdpSocket>,
        fd: i32,
        msgs: &[(Vec<u8>, socket2::SockAddr)],
    ) {
        let mut sent = 0usize;
        while sent < msgs.len() {
            let mut guard = match afd.writable().await {
                Ok(g) => g,
                Err(_) => return,
            };
            match guard.try_io(|_| sendmmsg_once(fd, &msgs[sent..])) {
                Ok(Ok(0)) => return,         // no progress; avoid spinning
                Ok(Ok(k)) => sent += k,
                Ok(Err(e)) => {
                    log::debug!("sendmmsg: {}", e);
                    return; // hard error: give up on the rest
                }
                Err(_would_block) => continue, // not writable yet
            }
        }
    }

    /// One `sendmmsg` for up to BATCH datagrams. Fully synchronous: all libc
    /// structs are stack-local so no raw pointer crosses an await point.
    fn sendmmsg_once(fd: i32, msgs: &[(Vec<u8>, socket2::SockAddr)]) -> std::io::Result<usize> {
        let n = msgs.len().min(BATCH);
        let mut iovecs: [libc::iovec; BATCH] = unsafe { std::mem::zeroed() };
        let mut mm: [libc::mmsghdr; BATCH] = unsafe { std::mem::zeroed() };
        for i in 0..n {
            let (b, sa) = &msgs[i];
            iovecs[i].iov_base = b.as_ptr() as *mut libc::c_void;
            iovecs[i].iov_len = b.len() as _;
            let h = &mut mm[i].msg_hdr;
            h.msg_name = sa.as_ptr() as *mut libc::c_void;
            h.msg_namelen = sa.len() as _;
            h.msg_iov = &mut iovecs[i] as *mut libc::iovec;
            h.msg_iovlen = 1 as _;
        }
        let r = unsafe { libc::sendmmsg(fd, mm.as_mut_ptr(), n as _, 0) };
        if r < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(r as usize)
        }
    }
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
                        if let Some(resp) = handle_query(&ctx, &qbuf, Transport::Tcp, peer.ip()).await {
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
                        match dns::parse_query(&resp) {
                            Some(rmeta) if rmeta.qname == job.qname => {
                                if cache_store(&ctx, job.key.clone(), &mut resp, rmeta.question_end) {
                                    log::debug!("refreshed {}", job.qname);
                                } else {
                                    // upstream answered but the response is not
                                    // cacheable (e.g. TTL 0): drop the old entry
                                    // instead of serving ever-older stale data
                                    if let Some(cache) = &ctx.cache {
                                        cache.remove(&job.key);
                                    }
                                }
                            }
                            _ => clear_refreshing(&ctx, &job.key),
                        }
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

/// Resolve one (name, qtype) through the group and store it in cache, exactly
/// as a real query would. Best-effort: errors are logged and ignored.
async fn prewarm_one(ctx: &Arc<Ctx>, name: &str, qtype: u16) {
    let Some(q) = dns::build_query(name, qtype, 1) else {
        return;
    };
    let key = cache::make_key(name, qtype, 1);
    let group = ctx.group_for(name, qtype);
    match group.resolve(&q, &ctx.stats).await {
        Ok((mut resp, _)) => {
            if let Some(rmeta) = dns::parse_query(&resp) {
                if rmeta.qname == name {
                    cache_store(ctx, key, &mut resp, rmeta.question_end);
                }
            }
        }
        Err(e) => log::debug!("prewarm {} failed: {}", name, e),
    }
}

/// Keep a fixed list of domains resolved so a first visit is never a cold
/// miss. Runs once at startup, then every `interval` seconds (if > 0). Domains
/// are read fresh each pass so edits to the file take effect without a restart.
///
/// Parse a prewarm domain list: trim, drop blanks and #comments, lowercase.
pub fn parse_prewarm_list(s: &str) -> Vec<String> {
    s.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_ascii_lowercase())
        .collect()
}

pub fn spawn_prewarmer(ctx: Arc<Ctx>) {
    let path = ctx.cfg.prewarm.domains_file.clone();
    if path.is_empty() {
        return;
    }
    let interval = ctx.cfg.prewarm.interval;
    tokio::spawn(async move {
        // small settle delay so upstreams/prober are up before the first pass
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            let domains: Vec<String> = match std::fs::read_to_string(&path) {
                Ok(s) => parse_prewarm_list(&s),
                Err(e) => {
                    log::warn!("prewarm: cannot read {}: {}", path, e);
                    Vec::new()
                }
            };
            if !domains.is_empty() {
                let n = domains.len();
                // bounded concurrency so a big list does not stampede the tunnel
                let sem = Arc::new(tokio::sync::Semaphore::new(8));
                let mut tasks = Vec::with_capacity(n * 2);
                for name in domains {
                    for qtype in [dns::TYPE_A, dns::TYPE_AAAA] {
                        let ctx = ctx.clone();
                        let sem = sem.clone();
                        let name = name.clone();
                        tasks.push(tokio::spawn(async move {
                            let _permit = sem.acquire().await;
                            prewarm_one(&ctx, &name, qtype).await;
                        }));
                    }
                }
                for t in tasks {
                    let _ = t.await;
                }
                log::info!("prewarm: refreshed {} domains", n);
            }
            if interval == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
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

#[cfg(test)]
mod tests {
    use super::parse_prewarm_list;

    #[test]
    fn prewarm_list_parsing() {
        let input = "\
# YouTube set
www.youtube.com
  I.Ytimg.com  

# comment
googlevideo.com
   # indented comment
";
        let got = parse_prewarm_list(input);
        assert_eq!(
            got,
            vec![
                "www.youtube.com".to_string(),
                "i.ytimg.com".to_string(), // trimmed + lowercased
                "googlevideo.com".to_string(),
            ]
        );
        // blanks, comments (including indented) dropped
        assert!(!got.iter().any(|d| d.starts_with('#') || d.is_empty()));
    }
}
