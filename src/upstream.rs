//! Upstream transports: UDP (shared-socket demux), TCP & DoT (pipelined
//! multiplexed connections, RFC 7766), DoH (HTTP/1.1 keep-alive pool).
//! A UDP upstream transparently falls back to TCP on truncated answers.

use crate::dns;
use crate::health::UpstreamState;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scheme {
    Udp,
    Tcp,
    Tls,
    Https,
}

impl Scheme {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scheme::Udp => "udp",
            Scheme::Tcp => "tcp",
            Scheme::Tls => "tls",
            Scheme::Https => "https",
        }
    }
}

pub struct Upstream {
    pub addr_str: String,
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub state: UpstreamState,
    resolved: RwLock<Option<SocketAddr>>,
    udp: Option<UdpTransport>,
    pipe: Option<PipePool>, // tcp / dot; also TC-fallback for udp
    doh: Option<DohPool>,
}

/// Parse "udp://1.2.3.4", "tcp://1.2.3.4:53", "tls://dns.google",
/// "https://dns.alidns.com/dns-query", bare "1.2.3.4". IPv6 in brackets.
pub fn parse_addr(s: &str) -> Result<(Scheme, String, u16, String)> {
    let (scheme, rest) = match s.split_once("://") {
        Some(("udp", r)) => (Scheme::Udp, r),
        Some(("tcp", r)) => (Scheme::Tcp, r),
        Some(("tls", r)) | Some(("dot", r)) => (Scheme::Tls, r),
        Some(("https", r)) | Some(("doh", r)) => (Scheme::Https, r),
        Some((other, _)) => bail!("unsupported scheme '{}'", other),
        None => (Scheme::Udp, s),
    };
    let (hostport, path) = match rest.split_once('/') {
        Some((hp, p)) => (hp, format!("/{}", p)),
        None => (rest, "/dns-query".to_string()),
    };
    let (host, port) = if let Some(h) = hostport.strip_prefix('[') {
        // [v6]:port
        let (v6, tail) = h
            .split_once(']')
            .ok_or_else(|| anyhow!("bad IPv6 literal in '{}'", s))?;
        let port = tail
            .strip_prefix(':')
            .map(|p| p.parse::<u16>())
            .transpose()?
            .unwrap_or(0);
        (v6.to_string(), port)
    } else if hostport.matches(':').count() > 1 {
        (hostport.to_string(), 0) // bare IPv6
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().context("bad port")?)
    } else {
        (hostport.to_string(), 0)
    };
    if host.is_empty() {
        bail!("empty host in '{}'", s);
    }
    let port = if port != 0 {
        port
    } else {
        match scheme {
            Scheme::Udp | Scheme::Tcp => 53,
            Scheme::Tls => 853,
            Scheme::Https => 443,
        }
    };
    Ok((scheme, host, port, path))
}

fn tls_connector(insecure: bool) -> TlsConnector {
    let config = if insecure {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth()
    } else {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };
    TlsConnector::from(Arc::new(config))
}

#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl Upstream {
    pub fn new(
        addr: &str,
        insecure: bool,
        idle_timeout: u64,
        bootstrap: SocketAddr,
    ) -> Result<Arc<Self>> {
        let (scheme, host, port, path) = parse_addr(addr)?;
        let ip: Option<IpAddr> = host.parse().ok();
        let sni = host.clone();
        let idle = Duration::from_secs(idle_timeout.max(5));

        let up = Arc::new(Upstream {
            addr_str: addr.to_string(),
            scheme,
            host: host.clone(),
            port,
            path,
            state: UpstreamState::new(),
            resolved: RwLock::new(ip.map(|i| SocketAddr::new(i, port))),
            udp: matches!(scheme, Scheme::Udp).then(UdpTransport::new),
            pipe: matches!(scheme, Scheme::Udp | Scheme::Tcp | Scheme::Tls).then(|| {
                PipePool::new(
                    matches!(scheme, Scheme::Tls).then(|| (tls_connector(insecure), sni.clone())),
                    idle,
                )
            }),
            doh: matches!(scheme, Scheme::Https)
                .then(|| DohPool::new(tls_connector(insecure), sni, idle)),
        });

        // hostname upstreams resolve via bootstrap, refreshed periodically
        if ip.is_none() {
            let u = up.clone();
            tokio::spawn(async move {
                loop {
                    match bootstrap_resolve(&u.host, bootstrap).await {
                        Ok(addr) => {
                            let sa = SocketAddr::new(addr, u.port);
                            let changed = *u.resolved.read() != Some(sa);
                            if changed {
                                log::info!("upstream {} resolved to {}", u.addr_str, sa);
                                *u.resolved.write() = Some(sa);
                            }
                            tokio::time::sleep(Duration::from_secs(900)).await;
                        }
                        Err(e) => {
                            log::warn!("bootstrap resolve {} failed: {}", u.host, e);
                            tokio::time::sleep(Duration::from_secs(10)).await;
                        }
                    }
                }
            });
        }
        Ok(up)
    }

    fn target(&self) -> Result<SocketAddr> {
        self.resolved
            .read()
            .ok_or_else(|| anyhow!("{}: not resolved yet", self.addr_str))
    }

    /// Send `query` (any ID; transports assign their own) and return the raw response.
    pub async fn query(&self, query: &[u8], deadline: Instant) -> Result<Vec<u8>> {
        let target = self.target()?;
        match self.scheme {
            Scheme::Udp => {
                let resp = self
                    .udp
                    .as_ref()
                    .unwrap()
                    .query(target, query, deadline)
                    .await?;
                if dns::is_truncated(&resp) {
                    // blazing-fast method fallback: retry over TCP
                    log::debug!("{}: truncated, retrying over TCP", self.addr_str);
                    return self
                        .pipe
                        .as_ref()
                        .unwrap()
                        .query(target, query, deadline)
                        .await;
                }
                Ok(resp)
            }
            Scheme::Tcp | Scheme::Tls => {
                self.pipe
                    .as_ref()
                    .unwrap()
                    .query(target, query, deadline)
                    .await
            }
            Scheme::Https => {
                self.doh
                    .as_ref()
                    .unwrap()
                    .query(target, &self.host, &self.path, query, deadline)
                    .await
            }
        }
    }

    /// Query with latency + health accounting (the failover feed).
    pub async fn query_measured(
        &self,
        query: &[u8],
        deadline: Instant,
        fail_threshold: u32,
        recover_threshold: u32,
        cooldown: Duration,
    ) -> Result<Vec<u8>> {
        let start = Instant::now();
        let res = self.query(query, deadline).await;
        match &res {
            Ok(resp) => {
                let rc = dns::rcode(resp);
                if rc == dns::RCODE_SERVFAIL || rc == 5 {
                    // upstream answered but is broken -> soft failure
                    self.state
                        .record_failure(fail_threshold, cooldown, &self.addr_str);
                    bail!("{}: upstream rcode {}", self.addr_str, rc);
                }
                self.state
                    .record_success(start.elapsed(), recover_threshold, &self.addr_str);
            }
            Err(e) => {
                log::debug!("{}: query failed: {}", self.addr_str, e);
                self.state
                    .record_failure(fail_threshold, cooldown, &self.addr_str);
            }
        }
        res
    }
}

/// Resolve a hostname via the bootstrap plain-DNS server (A, then AAAA).
async fn bootstrap_resolve(host: &str, bootstrap: SocketAddr) -> Result<IpAddr> {
    let sock = UdpSocket::bind(if bootstrap.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .await?;
    for qtype in [dns::TYPE_A, dns::TYPE_AAAA] {
        let q = dns::build_query(host, qtype, 1).ok_or_else(|| anyhow!("bad name"))?;
        for _ in 0..2 {
            sock.send_to(&q, bootstrap).await?;
            let mut buf = vec![0u8; 2048];
            match tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await {
                Ok(Ok((n, from))) if from == bootstrap => {
                    buf.truncate(n);
                    if let Some(meta) = dns::parse_query(&buf) {
                        let ips = dns::extract_ips(&buf, meta.question_end);
                        if let Some(ip) = ips.first() {
                            return Ok(*ip);
                        }
                    }
                    break; // valid empty answer -> try next qtype
                }
                _ => continue,
            }
        }
    }
    bail!("no address for {}", host)
}

// ---------------------------------------------------------------- UDP

const UDP_SOCKETS: usize = 4;

struct UdpSock {
    sock: Arc<UdpSocket>,
    pending: Mutex<HashMap<u16, oneshot::Sender<Vec<u8>>>>,
}

struct UdpTransport {
    socks: tokio::sync::OnceCell<Vec<Arc<UdpSock>>>,
}

impl UdpTransport {
    fn new() -> Self {
        Self {
            socks: tokio::sync::OnceCell::new(),
        }
    }

    /// Lazily create the shared sockets + reader tasks on first use.
    async fn socks(&self) -> Result<&Vec<Arc<UdpSock>>> {
        self.socks
            .get_or_try_init(|| async {
                let mut socks = Vec::with_capacity(UDP_SOCKETS);
                for _ in 0..UDP_SOCKETS {
                    let sock = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
                    let us = Arc::new(UdpSock {
                        sock,
                        pending: Mutex::new(HashMap::new()),
                    });
                    let us2 = us.clone();
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 4096];
                        loop {
                            match us2.sock.recv_from(&mut buf).await {
                                Ok((n, _from)) if n >= dns::HEADER_LEN => {
                                    let id = dns::get_id(&buf[..n]);
                                    if let Some(tx) = us2.pending.lock().remove(&id) {
                                        let _ = tx.send(buf[..n].to_vec());
                                    }
                                }
                                Ok(_) => {}
                                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                            }
                        }
                    });
                    socks.push(us);
                }
                Ok::<_, anyhow::Error>(socks)
            })
            .await
    }

    async fn query(&self, target: SocketAddr, query: &[u8], deadline: Instant) -> Result<Vec<u8>> {
        let socks = self.socks().await?;
        let us = &socks[fastrand::usize(..socks.len())];
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut pending = us.pending.lock();
            // reap entries whose callers were cancelled (hedged losers)
            if pending.len() > 32 {
                pending.retain(|_, tx| !tx.is_closed());
            }
            let mut id = fastrand::u16(..);
            let mut tries = 0;
            while pending.contains_key(&id) {
                id = fastrand::u16(..);
                tries += 1;
                if tries > 32 {
                    bail!("udp id space exhausted");
                }
            }
            pending.insert(id, tx);
            id
        };
        let mut pkt = query.to_vec();
        dns::set_id(&mut pkt, id);
        if let Err(e) = us.sock.send_to(&pkt, target).await {
            us.pending.lock().remove(&id);
            return Err(e.into());
        }
        match tokio::time::timeout_at(deadline.into(), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => {
                us.pending.lock().remove(&id);
                bail!("udp response channel closed")
            }
            Err(_) => {
                us.pending.lock().remove(&id);
                bail!("udp query timeout")
            }
        }
    }
}

// ------------------------------------------------- TCP / DoT (pipelined)

trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

type PipeReq = (Vec<u8>, oneshot::Sender<Vec<u8>>);

struct PipeConn {
    tx: mpsc::UnboundedSender<PipeReq>,
    alive: Arc<AtomicBool>,
}

struct PipeShared {
    pending: Mutex<HashMap<u16, oneshot::Sender<Vec<u8>>>>,
    alive: Arc<AtomicBool>,
    next_id: AtomicU16,
}

impl PipeConn {
    fn spawn(stream: Box<dyn Io>, idle: Duration) -> PipeConn {
        let (tx, mut rx) = mpsc::unbounded_channel::<PipeReq>();
        let alive = Arc::new(AtomicBool::new(true));
        let shared = Arc::new(PipeShared {
            pending: Mutex::new(HashMap::new()),
            alive: alive.clone(),
            next_id: AtomicU16::new(fastrand::u16(..)),
        });
        let (mut rd, mut wr) = tokio::io::split(stream);

        // writer + idle watchdog
        let ws = shared.clone();
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    m = rx.recv() => m,
                    _ = tokio::time::sleep(idle) => {
                        if ws.pending.lock().is_empty() {
                            None // idle close
                        } else {
                            continue;
                        }
                    }
                };
                let Some((mut buf, resp_tx)) = msg else { break };
                let id = {
                    let mut pending = ws.pending.lock();
                    let mut id = ws.next_id.fetch_add(1, Ordering::Relaxed);
                    while pending.contains_key(&id) {
                        id = ws.next_id.fetch_add(1, Ordering::Relaxed);
                    }
                    pending.insert(id, resp_tx);
                    id
                };
                dns::set_id(&mut buf, id);
                let mut frame = Vec::with_capacity(buf.len() + 2);
                frame.extend_from_slice(&(buf.len() as u16).to_be_bytes());
                frame.extend_from_slice(&buf);
                if wr.write_all(&frame).await.is_err() {
                    break;
                }
            }
            ws.alive.store(false, Ordering::Release);
            ws.pending.lock().clear();
            let _ = wr.shutdown().await;
        });

        // reader
        let rs = shared.clone();
        tokio::spawn(async move {
            let mut lenbuf = [0u8; 2];
            loop {
                if rd.read_exact(&mut lenbuf).await.is_err() {
                    break;
                }
                let len = u16::from_be_bytes(lenbuf) as usize;
                let mut buf = vec![0u8; len];
                if rd.read_exact(&mut buf).await.is_err() {
                    break;
                }
                if len >= dns::HEADER_LEN {
                    let id = dns::get_id(&buf);
                    if let Some(tx) = rs.pending.lock().remove(&id) {
                        let _ = tx.send(buf);
                    }
                }
            }
            rs.alive.store(false, Ordering::Release);
            rs.pending.lock().clear();
        });

        PipeConn { tx, alive }
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire) && !self.tx.is_closed()
    }
}

struct PipePool {
    tls: Option<(TlsConnector, String)>,
    idle: Duration,
    conn: tokio::sync::Mutex<Option<Arc<PipeConn>>>,
}

impl PipePool {
    fn new(tls: Option<(TlsConnector, String)>, idle: Duration) -> Self {
        Self {
            tls,
            idle,
            conn: tokio::sync::Mutex::new(None),
        }
    }

    async fn get_conn(&self, target: SocketAddr, deadline: Instant) -> Result<Arc<PipeConn>> {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_ref() {
            if c.is_alive() {
                return Ok(c.clone());
            }
        }
        let connect = async {
            let tcp = TcpStream::connect(target).await?;
            tcp.set_nodelay(true).ok();
            let stream: Box<dyn Io> = match &self.tls {
                None => Box::new(tcp),
                Some((connector, sni)) => {
                    let name = ServerName::try_from(sni.clone())
                        .map_err(|_| anyhow!("bad SNI '{}'", sni))?;
                    Box::new(connector.connect(name, tcp).await?)
                }
            };
            Ok::<_, anyhow::Error>(stream)
        };
        let stream = tokio::time::timeout_at(deadline.into(), connect)
            .await
            .map_err(|_| anyhow!("connect timeout"))??;
        let conn = Arc::new(PipeConn::spawn(stream, self.idle));
        *guard = Some(conn.clone());
        Ok(conn)
    }

    async fn query(&self, target: SocketAddr, query: &[u8], deadline: Instant) -> Result<Vec<u8>> {
        for attempt in 0..2 {
            let conn = self.get_conn(target, deadline).await?;
            let (tx, rx) = oneshot::channel();
            if conn.tx.send((query.to_vec(), tx)).is_err() {
                continue; // conn died between get and send
            }
            match tokio::time::timeout_at(deadline.into(), rx).await {
                Ok(Ok(resp)) => return Ok(resp),
                Ok(Err(_)) if attempt == 0 => continue, // conn broke mid-flight
                Ok(Err(_)) => bail!("stream connection closed"),
                Err(_) => bail!("stream query timeout"),
            }
        }
        bail!("stream connection failed twice")
    }
}

// ------------------------------------------------------------- DoH (h1.1)

struct DohConn {
    stream: Box<dyn Io>,
    last_use: Instant,
}

struct DohPool {
    connector: TlsConnector,
    sni: String,
    idle: Duration,
    conns: Mutex<Vec<DohConn>>,
}

impl DohPool {
    fn new(connector: TlsConnector, sni: String, idle: Duration) -> Self {
        Self {
            connector,
            sni,
            idle,
            conns: Mutex::new(Vec::new()),
        }
    }

    async fn dial(&self, target: SocketAddr) -> Result<Box<dyn Io>> {
        let tcp = TcpStream::connect(target).await?;
        tcp.set_nodelay(true).ok();
        let name = ServerName::try_from(self.sni.clone())
            .map_err(|_| anyhow!("bad SNI '{}'", self.sni))?;
        Ok(Box::new(self.connector.connect(name, tcp).await?))
    }

    async fn query(
        &self,
        target: SocketAddr,
        host: &str,
        path: &str,
        query: &[u8],
        deadline: Instant,
    ) -> Result<Vec<u8>> {
        // RFC 8484 wants ID 0 for cache friendliness
        let mut body = query.to_vec();
        dns::set_id(&mut body, 0);

        let work = async {
            for attempt in 0..2 {
                let pooled = {
                    let mut conns = self.conns.lock();
                    let mut got = None;
                    while let Some(c) = conns.pop() {
                        if c.last_use.elapsed() < self.idle {
                            got = Some(c.stream);
                            break;
                        }
                        // expired conns are simply dropped
                    }
                    got
                };
                let reused = pooled.is_some();
                let mut s = match pooled {
                    Some(s) => s,
                    None => self.dial(target).await?,
                };
                let req = format!(
                    "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/dns-message\r\nAccept: application/dns-message\r\nContent-Length: {}\r\nUser-Agent: photondns\r\n\r\n",
                    path, host, body.len()
                );
                let mut out = req.into_bytes();
                out.extend_from_slice(&body);
                if s.write_all(&out).await.is_err() {
                    if reused && attempt == 0 {
                        continue; // stale keep-alive conn; retry fresh
                    }
                    bail!("doh write failed");
                }
                match read_h1_response(&mut s).await {
                    Ok((resp, keep)) => {
                        if keep {
                            self.conns.lock().push(DohConn {
                                stream: s,
                                last_use: Instant::now(),
                            });
                        }
                        return Ok(resp);
                    }
                    Err(e) => {
                        if reused && attempt == 0 {
                            continue;
                        }
                        return Err(e);
                    }
                }
            }
            bail!("doh failed")
        };
        tokio::time::timeout_at(deadline.into(), work)
            .await
            .map_err(|_| anyhow!("doh timeout"))?
    }
}

/// Minimal HTTP/1.1 response reader (content-length or chunked).
async fn read_h1_response(s: &mut Box<dyn Io>) -> Result<(Vec<u8>, bool)> {
    let mut head = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16384 {
            bail!("doh headers too large");
        }
        let n = s.read(&mut byte).await?;
        if n == 0 {
            bail!("doh connection closed");
        }
        head.push(byte[0]);
    }
    let head_text = String::from_utf8_lossy(&head).to_ascii_lowercase();
    let status = head_text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    if status != 200 {
        bail!("doh http status {}", status);
    }
    let keep = !head_text.contains("connection: close");
    let content_length = head_text.lines().find_map(|l| {
        l.strip_prefix("content-length:")
            .map(|v| v.trim().parse::<usize>().ok())
            .flatten()
    });
    let chunked = head_text.contains("transfer-encoding: chunked");
    if let Some(len) = content_length {
        if len > 65535 {
            bail!("doh body too large");
        }
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).await?;
        Ok((body, keep))
    } else if chunked {
        let mut body = Vec::with_capacity(1024);
        loop {
            let mut line = Vec::new();
            loop {
                s.read_exact(&mut byte).await?;
                line.push(byte[0]);
                if line.ends_with(b"\r\n") {
                    break;
                }
                if line.len() > 32 {
                    bail!("bad chunk header");
                }
            }
            let sz = usize::from_str_radix(
                std::str::from_utf8(&line[..line.len() - 2])
                    .unwrap_or("")
                    .trim()
                    .split(';')
                    .next()
                    .unwrap_or(""),
                16,
            )
            .map_err(|_| anyhow!("bad chunk size"))?;
            if sz == 0 {
                let mut crlf = [0u8; 2];
                s.read_exact(&mut crlf).await.ok();
                break;
            }
            if body.len() + sz > 65535 {
                bail!("doh body too large");
            }
            let start = body.len();
            body.resize(start + sz, 0);
            s.read_exact(&mut body[start..]).await?;
            let mut crlf = [0u8; 2];
            s.read_exact(&mut crlf).await?;
        }
        Ok((body, keep))
    } else {
        bail!("doh response without length")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_parsing() {
        let (s, h, p, _) = parse_addr("8.8.8.8").unwrap();
        assert!(matches!(s, Scheme::Udp));
        assert_eq!((h.as_str(), p), ("8.8.8.8", 53));

        let (s, h, p, _) = parse_addr("tls://dns.google").unwrap();
        assert!(matches!(s, Scheme::Tls));
        assert_eq!((h.as_str(), p), ("dns.google", 853));

        let (s, h, p, path) = parse_addr("https://dns.alidns.com/dns-query").unwrap();
        assert!(matches!(s, Scheme::Https));
        assert_eq!(
            (h.as_str(), p, path.as_str()),
            ("dns.alidns.com", 443, "/dns-query")
        );

        let (s, h, p, _) = parse_addr("tcp://[2001:4860:4860::8888]:53").unwrap();
        assert!(matches!(s, Scheme::Tcp));
        assert_eq!((h.as_str(), p), ("2001:4860:4860::8888", 53));

        let (_, h, p, _) = parse_addr("udp://223.5.5.5:5353").unwrap();
        assert_eq!((h.as_str(), p), ("223.5.5.5", 5353));

        assert!(parse_addr("ftp://x").is_err());
    }
}
