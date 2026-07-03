//! Tiny HTTP/1.1 status + control API (LuCI backend talks to this).
//!   GET /stats   full JSON stats
//!   GET /health  200 if running
//!   GET /flush   clear the cache
//!   GET /version

use crate::server::Ctx;
use anyhow::Result;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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
        "version": env!("CARGO_PKG_VERSION"),
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
                let (status, body) = match path {
                    "/stats" | "/" => ("200 OK", stats_json(&ctx).to_string()),
                    "/health" => ("200 OK", json!({"status": "ok"}).to_string()),
                    "/version" => (
                        "200 OK",
                        json!({"version": env!("CARGO_PKG_VERSION")}).to_string(),
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
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });
    Ok(())
}
