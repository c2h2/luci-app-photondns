mod api;
mod cache;
mod config;
mod dns;
mod doh;
mod group;
mod health;
mod logger;
mod qlog;
mod router;
mod server;
mod stats;
mod upstream;

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn usage() -> ! {
    eprintln!("photondns {} - high-performance DNS forwarder", env!("PHOTONDNS_VERSION"));
    eprintln!("usage: photondns [-c /etc/photondns/config.toml] [-t] [-V]");
    eprintln!("  -c <file>  config file (TOML)");
    eprintln!("  -t         test configuration and exit");
    eprintln!("  -V         print version");
    std::process::exit(2)
}

fn main() -> Result<()> {
    let mut config_path = "/etc/photondns/config.toml".to_string();
    let mut test_only = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-c" | "--config" => config_path = args.next().unwrap_or_else(|| usage()),
            "-t" | "--test" => test_only = true,
            "-V" | "--version" | "version" => {
                println!("photondns {}", env!("PHOTONDNS_VERSION"));
                return Ok(());
            }
            _ => usage(),
        }
    }

    let cfg = config::Config::load(&config_path)?;
    if test_only {
        println!("config ok: {}", config_path);
        return Ok(());
    }
    logger::init(&cfg.log.level, &cfg.log.file)?;

    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(2)
        .min(8);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()?;
    rt.block_on(run(cfg))
}

async fn run(cfg: config::Config) -> Result<()> {
    log::info!(
        "photondns {} starting (cache: {} entries, {} groups)",
        env!("PHOTONDNS_VERSION"),
        if cfg.cache.enabled { cfg.cache.size } else { 0 },
        cfg.groups.len()
    );

    let groups: Vec<_> = cfg
        .groups
        .iter()
        .map(|g| group::Group::from_config(g, &cfg.failover))
        .collect::<Result<_>>()?;
    for g in &groups {
        log::info!(
            "group '{}': strategy={:?} upstreams={:?} backups={:?}",
            g.name,
            g.strategy,
            g.upstreams.iter().map(|u| u.addr_str.as_str()).collect::<Vec<_>>(),
            g.backups.iter().map(|u| u.addr_str.as_str()).collect::<Vec<_>>()
        );
    }

    let cache = cfg.cache.enabled.then(|| Arc::new(cache::DnsCache::new(cfg.cache.size)));
    if let (Some(c), path) = (&cache, cfg.cache.dump_file.clone()) {
        if !path.is_empty() {
            match c.load(&path) {
                Ok(n) => log::info!("restored {} cache entries from {}", n, path),
                Err(e) => log::info!("no cache dump restored ({})", e),
            }
        }
    }

    let router = router::Router::load(&cfg.routing);
    let (refresh_tx, refresh_rx) = mpsc::channel(4096);
    let ctx = Arc::new(server::Ctx {
        cache: cache.clone(),
        router,
        groups: groups.clone(),
        stats: Arc::new(stats::Stats::new()),
        refresh_tx,
        qlog: Arc::new(qlog::QueryLog::new(cfg.log.query_log_size)),
        cfg,
    });

    server::spawn_refresher(ctx.clone(), refresh_rx);
    group::spawn_prober(groups, &ctx.cfg.failover);

    for addr_str in &ctx.cfg.server.listen {
        let addr: std::net::SocketAddr = addr_str
            .parse()
            .with_context(|| format!("bad listen address '{}'", addr_str))?;
        if ctx.cfg.server.udp {
            server::run_udp(ctx.clone(), addr).await?;
        }
        if ctx.cfg.server.tcp {
            server::run_tcp(ctx.clone(), addr).await?;
        }
    }
    if !ctx.cfg.server.doh_listen.is_empty() {
        doh::run(ctx.clone()).await?;
    }

    if !ctx.cfg.api.listen.is_empty() {
        api::run(ctx.clone(), ctx.cfg.api.listen.clone()).await?;
    }

    // periodic cache dump
    if let Some(c) = &cache {
        let path = ctx.cfg.cache.dump_file.clone();
        let interval = ctx.cfg.cache.dump_interval;
        if !path.is_empty() && interval > 0 {
            let c = c.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(interval)).await;
                    match c.dump(&path) {
                        Ok(n) => log::debug!("dumped {} cache entries", n),
                        Err(e) => log::warn!("cache dump failed: {}", e),
                    }
                }
            });
        }
    }

    log::info!("photondns ready");

    // graceful shutdown: dump cache on SIGTERM/SIGINT
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    tokio::select! {
        _ = sigterm.recv() => log::info!("SIGTERM received"),
        _ = sigint.recv() => log::info!("SIGINT received"),
    }
    if let Some(c) = &cache {
        let path = &ctx.cfg.cache.dump_file;
        if !path.is_empty() {
            match c.dump(path) {
                Ok(n) => log::info!("dumped {} cache entries to {}", n, path),
                Err(e) => log::warn!("final cache dump failed: {}", e),
            }
        }
    }
    log::info!("bye");
    Ok(())
}
