//! DNS-over-HTTPS server (RFC 8484): GET ?dns=<base64url> and POST
//! application/dns-message on a configurable path.
//!
//! Runs plain HTTP when no cert is configured - meant to sit behind a TLS
//! reverse proxy (Caddy: `reverse_proxy /dns-query 127.0.0.1:8054`).
//! With server.doh_cert/doh_key it terminates TLS itself (standalone DoH).

use crate::server::{handle_query, Ctx, Transport};
use anyhow::{anyhow, Context as _, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const MAX_HEAD: usize = 8192;
const MAX_BODY: usize = 8192;

fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue, // tolerate padding
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn load_tls(cert_path: &str, key_path: &str) -> Result<TlsAcceptor> {
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(
        std::fs::File::open(cert_path).with_context(|| format!("open {}", cert_path))?,
    ))
    .collect::<std::result::Result<Vec<_>, _>>()
    .with_context(|| format!("parse certs in {}", cert_path))?;
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(
        std::fs::File::open(key_path).with_context(|| format!("open {}", key_path))?,
    ))?
    .ok_or_else(|| anyhow!("no private key found in {}", key_path))?;
    let config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

pub async fn run(ctx: Arc<Ctx>) -> Result<()> {
    let addr: SocketAddr = ctx
        .cfg
        .server
        .doh_listen
        .parse()
        .with_context(|| format!("bad doh_listen '{}'", ctx.cfg.server.doh_listen))?;
    let tls = if ctx.cfg.server.doh_cert.is_empty() {
        None
    } else {
        Some(load_tls(&ctx.cfg.server.doh_cert, &ctx.cfg.server.doh_key)?)
    };
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind doh {}", addr))?;
    log::info!(
        "DoH listening on {}://{}{}{}",
        if tls.is_some() { "https" } else { "http" },
        addr,
        ctx.cfg.server.doh_path,
        if tls.is_some() { "" } else { " (put a TLS reverse proxy in front for browsers)" }
    );
    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            };
            let ctx = ctx.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                stream.set_nodelay(true).ok();
                match tls {
                    Some(acceptor) => {
                        if let Ok(s) = acceptor.accept(stream).await {
                            serve_conn(ctx, s, peer).await;
                        }
                    }
                    None => serve_conn(ctx, stream, peer).await,
                }
            });
        }
    });
    Ok(())
}

/// HTTP/1.1 keep-alive loop for one connection.
async fn serve_conn<S: AsyncRead + AsyncWrite + Unpin>(ctx: Arc<Ctx>, mut s: S, peer: SocketAddr) {
    let idle = Duration::from_secs(ctx.cfg.server.tcp_idle_timeout.max(5));
    let path = ctx.cfg.server.doh_path.clone();
    loop {
        // read request head
        let mut head = Vec::with_capacity(512);
        loop {
            let mut b = [0u8; 1];
            match tokio::time::timeout(idle, s.read(&mut b)).await {
                Ok(Ok(1)) => head.push(b[0]),
                _ => return,
            }
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            if head.len() > MAX_HEAD {
                let _ = respond(&mut s, 431, b"", false).await;
                return;
            }
        }
        let head_text = String::from_utf8_lossy(&head);
        let mut lines = head_text.split("\r\n");
        let mut req = lines.next().unwrap_or("").split_whitespace();
        let (method, target) = (req.next().unwrap_or(""), req.next().unwrap_or(""));
        let mut content_length = 0usize;
        let mut keep = true;
        for l in lines {
            let ll = l.to_ascii_lowercase();
            if let Some(v) = ll.strip_prefix("content-length:") {
                content_length = v.trim().parse().unwrap_or(0);
            }
            if ll.starts_with("connection:") && ll.contains("close") {
                keep = false;
            }
        }
        if content_length > MAX_BODY {
            let _ = respond(&mut s, 413, b"", false).await;
            return;
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 && s.read_exact(&mut body).await.is_err() {
            return;
        }

        let (req_path, query_str) = match target.split_once('?') {
            Some((p, q)) => (p, q),
            None => (target, ""),
        };
        if req_path != path {
            if respond(&mut s, 404, b"", keep).await.is_err() || !keep {
                return;
            }
            continue;
        }
        let dns_query = match method {
            "POST" => body,
            "GET" => query_str
                .split('&')
                .find_map(|kv| kv.strip_prefix("dns="))
                .and_then(b64url_decode)
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if dns_query.len() < crate::dns::HEADER_LEN {
            if respond(&mut s, 400, b"", keep).await.is_err() || !keep {
                return;
            }
            continue;
        }
        match handle_query(&ctx, &dns_query, Transport::Doh, peer.ip()).await {
            Some(resp) => {
                if respond(&mut s, 200, &resp, keep).await.is_err() {
                    return;
                }
            }
            None => {
                if respond(&mut s, 400, b"", keep).await.is_err() {
                    return;
                }
            }
        }
        if !keep {
            return;
        }
    }
}

async fn respond<S: AsyncWrite + Unpin>(s: &mut S, code: u16, body: &[u8], keep: bool) -> std::io::Result<()> {
    let status = match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        413 => "413 Payload Too Large",
        431 => "431 Request Header Fields Too Large",
        _ => "500 Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/dns-message\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n",
        status,
        body.len(),
        if keep { "keep-alive" } else { "close" }
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    s.write_all(&out).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64url() {
        assert_eq!(b64url_decode("AAABAAABAAAAAAAA").unwrap().len(), 12);
        assert_eq!(b64url_decode("-_8").unwrap(), vec![0xFB, 0xFF]);
        assert_eq!(b64url_decode("aGk").unwrap(), b"hi");
        assert!(b64url_decode("a!b").is_none());
    }
}
