use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerCfg,
    #[serde(default)]
    pub cache: CacheCfg,
    #[serde(default)]
    pub api: ApiCfg,
    #[serde(default)]
    pub log: LogCfg,
    #[serde(default)]
    pub failover: FailoverCfg,
    #[serde(default)]
    pub routing: RoutingCfg,
    /// upstream groups; group "main" is the default route target
    #[serde(default, rename = "group")]
    pub groups: Vec<GroupCfg>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerCfg {
    /// listen addresses, e.g. ["0.0.0.0:5353"]
    #[serde(default = "default_listen")]
    pub listen: Vec<String>,
    #[serde(default = "default_true")]
    pub tcp: bool,
    /// parallel UDP sockets per address (SO_REUSEPORT); 0 = auto
    #[serde(default)]
    pub udp_sockets: usize,
    #[serde(default = "default_tcp_idle")]
    pub tcp_idle_timeout: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheCfg {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// maximum number of cached responses
    #[serde(default = "default_cache_size")]
    pub size: usize,
    #[serde(default)]
    pub min_ttl: u32,
    #[serde(default = "default_max_ttl")]
    pub max_ttl: u32,
    #[serde(default = "default_negative_ttl")]
    pub negative_ttl: u32,
    /// serve expired entries immediately and refresh in background (lazy cache)
    #[serde(default = "default_true")]
    pub serve_stale: bool,
    /// how long past expiry an entry may still be served (seconds)
    #[serde(default = "default_stale_ttl")]
    pub stale_ttl: u32,
    /// refresh popular entries shortly before they expire
    #[serde(default = "default_true")]
    pub prefetch: bool,
    /// start prefetch when remaining TTL falls below this many seconds
    #[serde(default = "default_prefetch_margin")]
    pub prefetch_margin: u32,
    /// minimum hits before an entry qualifies for prefetch
    #[serde(default = "default_prefetch_hits")]
    pub prefetch_min_hits: u32,
    /// persist cache across restarts ("" = disabled)
    #[serde(default)]
    pub dump_file: String,
    #[serde(default = "default_dump_interval")]
    pub dump_interval: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiCfg {
    /// "" disables the HTTP status API
    #[serde(default = "default_api_listen")]
    pub listen: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogCfg {
    #[serde(default = "default_log_level")]
    pub level: String,
    /// "" = stdout only
    #[serde(default)]
    pub file: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailoverCfg {
    /// seconds between active health probes
    #[serde(default = "default_hc_interval")]
    pub health_check_interval: u64,
    #[serde(default = "default_hc_domain")]
    pub health_check_domain: String,
    /// consecutive failures before an upstream is marked down
    #[serde(default = "default_fail_threshold")]
    pub fail_threshold: u32,
    /// consecutive successes before a down upstream is restored
    #[serde(default = "default_recover_threshold")]
    pub recover_threshold: u32,
    /// seconds a down upstream is excluded before half-open retry
    #[serde(default = "default_cooldown")]
    pub cooldown: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingCfg {
    /// hosts file: "name ip [ip...]" per line
    #[serde(default)]
    pub hosts_file: String,
    /// domains answered NXDOMAIN
    #[serde(default)]
    pub block_file: String,
    /// domains routed to the "local" group
    #[serde(default)]
    pub local_domains_file: String,
    /// "from-domain to-domain" pairs, answered with the target's records
    #[serde(default)]
    pub redirect_file: String,
    /// answer TTL for hosts entries
    #[serde(default = "default_hosts_ttl")]
    pub hosts_ttl: u32,
    /// refuse HTTPS/SVCB (type 65) queries with NXDOMAIN
    #[serde(default)]
    pub reject_type65: bool,
    /// NXDOMAIN for special-use TLDs (.local, .lan, .internal, ...) that
    /// must never reach public resolvers (mDNS noise protection)
    #[serde(default = "default_true")]
    pub block_special: bool,
    /// NXDOMAIN for PTR lookups of private/link-local address space
    #[serde(default = "default_true")]
    pub block_private_ptr: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupCfg {
    pub name: String,
    /// race | fastest | parallel | sequential | random
    #[serde(default = "default_strategy")]
    pub strategy: String,
    pub upstreams: Vec<String>,
    /// used only when every primary upstream is down
    #[serde(default)]
    pub backups: Vec<String>,
    /// max ms to wait before hedging to the next-best upstream (race)
    #[serde(default = "default_hedge_delay")]
    pub hedge_delay_ms: u64,
    /// per-attempt timeout
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// DoT/DoH certificate verification off
    #[serde(default)]
    pub insecure_skip_verify: bool,
    /// plain resolver used to look up DoT/DoH hostnames
    #[serde(default = "default_bootstrap")]
    pub bootstrap: String,
    /// idle seconds before pooled TCP/TLS connections are closed
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: u64,
}

fn default_listen() -> Vec<String> {
    // high port, clear of unicast mDNS (5353) noise
    vec!["127.0.0.1:15533".into()]
}
fn default_true() -> bool {
    true
}
fn default_tcp_idle() -> u64 {
    30
}
fn default_cache_size() -> usize {
    8192
}
fn default_max_ttl() -> u32 {
    86400
}
fn default_negative_ttl() -> u32 {
    30
}
fn default_stale_ttl() -> u32 {
    86400
}
fn default_prefetch_margin() -> u32 {
    10
}
fn default_prefetch_hits() -> u32 {
    2
}
fn default_dump_interval() -> u64 {
    3600
}
fn default_api_listen() -> String {
    "127.0.0.1:8053".into()
}
fn default_log_level() -> String {
    "info".into()
}
fn default_hc_interval() -> u64 {
    10
}
fn default_hc_domain() -> String {
    "www.gstatic.com".into()
}
fn default_fail_threshold() -> u32 {
    3
}
fn default_recover_threshold() -> u32 {
    2
}
fn default_cooldown() -> u64 {
    15
}
fn default_hosts_ttl() -> u32 {
    300
}
fn default_strategy() -> String {
    "race".into()
}
fn default_hedge_delay() -> u64 {
    250
}
fn default_timeout() -> u64 {
    2000
}
fn default_bootstrap() -> String {
    "223.5.5.5:53".into()
}
fn default_idle_timeout() -> u64 {
    30
}

impl Default for ServerCfg {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}
impl Default for CacheCfg {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}
impl Default for ApiCfg {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}
impl Default for LogCfg {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}
impl Default for FailoverCfg {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}
impl Default for RoutingCfg {
    fn default() -> Self {
        toml::from_str("").unwrap()
    }
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Config> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {}", path, e))?;
        let mut cfg: Config =
            toml::from_str(&text).map_err(|e| anyhow::anyhow!("config parse error: {}", e))?;
        if cfg.groups.is_empty() {
            cfg.groups.push(GroupCfg {
                name: "main".into(),
                strategy: default_strategy(),
                upstreams: vec!["udp://223.5.5.5".into(), "udp://119.29.29.29".into()],
                backups: vec![],
                hedge_delay_ms: default_hedge_delay(),
                timeout_ms: default_timeout(),
                insecure_skip_verify: false,
                bootstrap: default_bootstrap(),
                idle_timeout: default_idle_timeout(),
            });
        }
        for g in &cfg.groups {
            if g.upstreams.is_empty() && g.backups.is_empty() {
                anyhow::bail!("group '{}' has no upstreams", g.name);
            }
            match g.strategy.as_str() {
                "race" | "fastest" | "parallel" | "sequential" | "random" => {}
                s => anyhow::bail!("group '{}': unknown strategy '{}'", g.name, s),
            }
        }
        if cfg.cache.size == 0 {
            cfg.cache.enabled = false;
        }
        Ok(cfg)
    }
}
