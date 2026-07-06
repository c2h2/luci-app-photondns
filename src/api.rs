//! Tiny HTTP/1.1 status + control API (LuCI backend talks to this).
//!   GET /stats   full JSON stats
//!   GET /health  200 if running
//!   GET /flush   clear the cache
//!   GET /version

use crate::server::{resolve_named, Ctx};
use anyhow::Result;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Percent-decode a query-string value (enough for domain names: %XX + '+').
fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let h = |c: u8| (c as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (h(b[i + 1]), h(b[i + 2])) {
                    out.push((hi * 16 + lo) as u8 as char);
                    i += 3;
                } else {
                    out.push('%');
                    i += 1;
                }
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
}

fn query_param<'a>(path: &'a str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(urldecode(v));
            }
        }
    }
    None
}

/// Map a DNS type token ("A", "aaaa", "28", "txt") to its numeric qtype.
fn parse_qtype(s: &str) -> Option<u16> {
    if let Ok(n) = s.parse::<u16>() {
        return Some(n);
    }
    Some(match s.to_ascii_uppercase().as_str() {
        "A" => 1,
        "NS" => 2,
        "CNAME" => 5,
        "SOA" => 6,
        "PTR" => 12,
        "MX" => 15,
        "TXT" => 16,
        "AAAA" => 28,
        "SRV" => 33,
        "DS" => 43,
        "DNSKEY" => 48,
        "SVCB" => 64,
        "HTTPS" => 65,
        "ANY" => 255,
        "CAA" => 257,
        _ => return None,
    })
}

/// Build an HTTP/1.1 JSON response. Includes a permissive CORS header so the
/// bundled test page works when opened directly from disk (file:// origin).
fn http_json(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    )
}

fn rcode_name(r: u8) -> &'static str {
    match r {
        0 => "NOERROR",
        1 => "FORMERR",
        2 => "SERVFAIL",
        3 => "NXDOMAIN",
        4 => "NOTIMP",
        5 => "REFUSED",
        _ => "OTHER",
    }
}

fn stats_json(ctx: &Arc<Ctx>) -> serde_json::Value {
    let s = &ctx.stats;
    let hits = s.cache_hits.load(Ordering::Relaxed);
    let misses = s.cache_misses.load(Ordering::Relaxed);
    let upstreams: Vec<_> = ctx
        .groups
        .iter()
        .flat_map(|g| {
            g.all_upstreams().map(move |u| {
                json!({
                    "group": g.name,
                    "addr": u.addr_str,
                    "scheme": u.scheme.as_str(),
                    "healthy": u.state.is_healthy(),
                    "ewma_ms": (u.state.ewma_ms() * 100.0).round() / 100.0,
                    "last_rtt_ms": u.state.last_rtt_us.load(Ordering::Relaxed) as f64 / 1000.0,
                    "ok": u.state.ok.load(Ordering::Relaxed),
                    "fail": u.state.fail.load(Ordering::Relaxed),
                    "down_events": u.state.down_events.load(Ordering::Relaxed),
                })
            })
        })
        .collect();
    let groups: Vec<_> = ctx
        .groups
        .iter()
        .map(|g| {
            json!({
                "name": g.name,
                "queries": g.queries.load(Ordering::Relaxed),
            })
        })
        .collect();
    json!({
        "version": env!("PHOTONDNS_VERSION"),
        "uptime": s.uptime(),
        "groups": groups,
        "queries": {
            "total": s.total.load(Ordering::Relaxed),
            "udp": s.udp.load(Ordering::Relaxed),
            "tcp": s.tcp.load(Ordering::Relaxed),
            "qpm": s.rate.last_minute(),
            "blocked": s.blocked.load(Ordering::Relaxed),
            "hosts": s.hosts_served.load(Ordering::Relaxed),
            "redirected": s.redirected.load(Ordering::Relaxed),
            "servfail": s.servfail.load(Ordering::Relaxed),
            "hedged": s.hedged.load(Ordering::Relaxed),
            "upstream_errors": s.upstream_errors.load(Ordering::Relaxed),
        },
        "cache": {
            "enabled": ctx.cache.is_some(),
            "size": ctx.cache.as_ref().map(|c| c.len()).unwrap_or(0),
            "capacity": ctx.cache.as_ref().map(|c| c.capacity).unwrap_or(0),
            "hits": hits,
            "misses": misses,
            "hit_rate": if hits + misses > 0 {
                (hits as f64 / (hits + misses) as f64 * 10000.0).round() / 100.0
            } else { 0.0 },
            "stale_served": s.stale_served.load(Ordering::Relaxed),
            "prefetches": s.prefetches.load(Ordering::Relaxed),
            "inserts": ctx.cache.as_ref().map(|c| c.inserts.load(Ordering::Relaxed)).unwrap_or(0),
            "evictions": ctx.cache.as_ref().map(|c| c.evictions.load(Ordering::Relaxed)).unwrap_or(0),
        },
        "upstreams": upstreams,
    })
}

pub async fn run(ctx: Arc<Ctx>, listen: String) -> Result<()> {
    let listener = TcpListener::bind(&listen).await?;
    log::info!("API listening on http://{}", listen);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                continue;
            };
            let ctx = ctx.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 2048];
                let Ok(Ok(n)) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    stream.read(&mut buf),
                )
                .await
                else {
                    return;
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/");
                if path == "/log" || path.starts_with("/log?") {
                    let n = path
                        .split_once("n=")
                        .and_then(|(_, v)| {
                            v.split(&['&', ' '][..]).next().and_then(|x| x.parse::<usize>().ok())
                        })
                        .unwrap_or(500)
                        .min(5000);
                    let body = json!({ "entries": ctx.qlog.snapshot(n) }).to_string();
                    let _ = stream.write_all(http_json("200 OK", &body).as_bytes()).await;
                    return;
                }

                // GET /resolve?name=<domain>&type=<A|AAAA|...>  -> dig-like JSON
                if path.starts_with("/resolve?") || path == "/resolve" {
                    let name = query_param(path, "name").unwrap_or_default();
                    let qt = query_param(path, "type").unwrap_or_else(|| "A".into());
                    let name = name.trim().trim_end_matches('.').to_string();
                    let body = if name.is_empty() {
                        json!({"error": "missing name"}).to_string()
                    } else if let Some(qtype) = parse_qtype(&qt) {
                        match resolve_named(&ctx, &name, qtype).await {
                            Ok(r) => json!({
                                "name": name,
                                "type": qt.to_ascii_uppercase(),
                                "route": r.route,
                                "upstream": r.upstream,
                                "rcode": rcode_name(r.rcode),
                                "answers": r.ips.iter().map(|ip| ip.to_string()).collect::<Vec<_>>(),
                                "ttl": r.ttl,
                                "elapsed_ms": (r.elapsed.as_micros() as f64 / 1000.0 * 100.0).round() / 100.0,
                            })
                            .to_string(),
                            Err(e) => json!({
                                "name": name,
                                "type": qt.to_ascii_uppercase(),
                                "route": "failed",
                                "error": e.to_string(),
                            })
                            .to_string(),
                        }
                    } else {
                        json!({"error": format!("unknown type '{}'", qt)}).to_string()
                    };
                    let _ = stream.write_all(http_json("200 OK", &body).as_bytes()).await;
                    return;
                }
                let (status, body) = match path {
                    "/stats" | "/" => ("200 OK", stats_json(&ctx).to_string()),
                    "/health" => ("200 OK", json!({"status": "ok"}).to_string()),
                    "/version" => (
                        "200 OK",
                        json!({"version": env!("PHOTONDNS_VERSION")}).to_string(),
                    ),
                    "/flush" => {
                        let flushed = if let Some(c) = &ctx.cache {
                            let n = c.len();
                            c.flush();
                            n
                        } else {
                            0
                        };
                        ("200 OK", json!({"success": true, "flushed": flushed}).to_string())
                    }
                    _ => ("404 Not Found", json!({"error": "not found"}).to_string()),
                };
                let _ = stream.write_all(http_json(status, &body).as_bytes()).await;
            });
        }
    });
    Ok(())
}
