//! Domain routing: hosts overrides, blocklist, redirect, and
//! local-domain -> local group dispatch. Matching is O(#labels) hash lookups.

use crate::config::{LanCfg, RoutingCfg};
use crate::lan::{self, LanHosts};
use arc_swap::ArcSwap;
use rustc_hash::{FxHashMap, FxHashSet};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

// Domain routing is the hottest hashing on the query path: every forwarded name
// walks its labels against the china/ad sets (100k+ entries each), doing several
// lookups per query. FxHash is ~3-5x faster than the default SipHash for these
// short ASCII keys, and these sets are never exposed to untrusted key flooding
// in a way that DoS-hardening (SipHash's purpose) would matter for.
#[derive(Default)]
pub struct DomainSet {
    full: FxHashSet<String>,
    suffix: FxHashSet<String>,
}

impl DomainSet {
    /// Rule formats (one per line, '#' comments):
    ///   example.com          suffix match (the domain and all subdomains)
    ///   domain:example.com   same as above
    ///   full:example.com     exact match only
    pub fn load(path: &str) -> DomainSet {
        let mut set = DomainSet::default();
        if path.is_empty() || !Path::new(path).exists() {
            return set;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            log::warn!("cannot read {}", path);
            return set;
        };
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(d) = line.strip_prefix("full:") {
                set.full.insert(normalize(d));
            } else if let Some(d) = line.strip_prefix("domain:") {
                set.suffix.insert(normalize(d));
            } else {
                set.suffix.insert(normalize(line));
            }
        }
        set
    }

    pub fn len(&self) -> usize {
        self.full.len() + self.suffix.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `name` must be lowercase without trailing dot.
    pub fn matches(&self, name: &str) -> bool {
        if self.full.contains(name) || self.suffix.contains(name) {
            return true;
        }
        let mut rest = name;
        while let Some(i) = rest.find('.') {
            rest = &rest[i + 1..];
            if self.suffix.contains(rest) {
                return true;
            }
        }
        false
    }
}

fn normalize(d: &str) -> String {
    d.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// TLDs that must never be forwarded to public resolvers (RFC 6761/6762
/// and common site-local names). Sending these upstream just produces
/// timeouts/junk - the exact failure mode that poisons failover stats.
const SPECIAL_TLDS: &[&str] = &[
    "local", "localhost", "invalid", "test", "onion", "home.arpa", "internal",
    "lan", "home", "intranet", "private", "corp",
];

/// PTR zones of private / link-local address space.
const PRIVATE_PTR: &[&str] = &[
    "10.in-addr.arpa",
    "168.192.in-addr.arpa",
    "254.169.in-addr.arpa",
    "127.in-addr.arpa",
    "16.172.in-addr.arpa", "17.172.in-addr.arpa", "18.172.in-addr.arpa",
    "19.172.in-addr.arpa", "20.172.in-addr.arpa", "21.172.in-addr.arpa",
    "22.172.in-addr.arpa", "23.172.in-addr.arpa", "24.172.in-addr.arpa",
    "25.172.in-addr.arpa", "26.172.in-addr.arpa", "27.172.in-addr.arpa",
    "28.172.in-addr.arpa", "29.172.in-addr.arpa", "30.172.in-addr.arpa",
    "31.172.in-addr.arpa",
    "d.f.ip6.arpa",
    "8.e.f.ip6.arpa", "9.e.f.ip6.arpa", "a.e.f.ip6.arpa", "b.e.f.ip6.arpa",
];

pub struct Router {
    pub hosts: FxHashMap<String, Vec<IpAddr>>,
    pub blocked: DomainSet,
    pub local_domains: DomainSet,
    pub redirects: FxHashMap<String, String>,
    pub hosts_ttl: u32,
    pub reject_type65: bool,
    special: DomainSet,
    /// LAN client names from DHCP leases + extra-hosts; swapped in on refresh.
    /// `None` disables the whole feature (no lease reads, no PTR answers).
    lan: Option<LanState>,
}

struct LanState {
    hosts: ArcSwap<LanHosts>,
    cfg: LanCfg,
}

/// Answer produced for a LAN forward/reverse hit. Owns its data so it can be
/// returned past the borrow of the swapped-in `LanHosts` snapshot.
pub enum LanAnswer {
    /// A/AAAA addresses for a LAN name
    Addrs(Vec<IpAddr>),
    /// PTR target FQDN for a reverse query
    Ptr(String),
}

pub enum Decision<'a> {
    /// answer from hosts
    Hosts(&'a Vec<IpAddr>),
    /// answer from the LAN lease/extra-hosts table (forward or reverse)
    Lan(LanAnswer, u32),
    /// NXDOMAIN
    Block,
    /// resolve this name instead and answer with its records
    Redirect(&'a str),
    /// forward to the group with this name ("local" / "main")
    Forward(&'static str),
}

impl Router {
    pub fn load_with_lan(cfg: &RoutingCfg, lan_cfg: &LanCfg) -> Router {
        let mut hosts: FxHashMap<String, Vec<IpAddr>> = FxHashMap::default();
        if !cfg.hosts_file.is_empty() && Path::new(&cfg.hosts_file).exists() {
            if let Ok(text) = std::fs::read_to_string(&cfg.hosts_file) {
                for line in text.lines() {
                    let line = line.split('#').next().unwrap_or("").trim();
                    let mut it = line.split_whitespace();
                    let Some(name) = it.next() else { continue };
                    let ips: Vec<IpAddr> = it.filter_map(|t| t.parse().ok()).collect();
                    if !ips.is_empty() {
                        hosts.entry(normalize(name)).or_default().extend(ips);
                    }
                }
            }
        }
        let mut redirects = FxHashMap::default();
        if !cfg.redirect_file.is_empty() && Path::new(&cfg.redirect_file).exists() {
            if let Ok(text) = std::fs::read_to_string(&cfg.redirect_file) {
                for line in text.lines() {
                    let line = line.split('#').next().unwrap_or("").trim();
                    let mut it = line.split_whitespace();
                    if let (Some(from), Some(to)) = (it.next(), it.next()) {
                        redirects.insert(normalize(from), normalize(to));
                    }
                }
            }
        }
        let mut blocked = DomainSet::load(&cfg.block_file);
        let mut ad_count = 0;
        if !cfg.ad_list_file.is_empty() {
            let ads = DomainSet::load(&cfg.ad_list_file);
            ad_count = ads.len();
            blocked.full.extend(ads.full);
            blocked.suffix.extend(ads.suffix);
        }
        let mut local_domains = DomainSet::load(&cfg.local_domains_file);
        if !cfg.china_list_file.is_empty() {
            let china = DomainSet::load(&cfg.china_list_file);
            local_domains.full.extend(china.full);
            local_domains.suffix.extend(china.suffix);
        }
        let mut special = DomainSet::default();
        if cfg.block_special {
            for d in SPECIAL_TLDS {
                special.suffix.insert((*d).into());
            }
        }
        if cfg.block_private_ptr {
            for d in PRIVATE_PTR {
                special.suffix.insert((*d).into());
            }
        }
        let lan = if lan_cfg.enabled {
            let h = LanHosts::load(lan_cfg);
            log::info!("router: {} lan hosts (suffix '{}')", h.len(), lan_cfg.suffix);
            Some(LanState {
                hosts: ArcSwap::from_pointee(h),
                cfg: lan_cfg.clone(),
            })
        } else {
            None
        };
        log::info!(
            "router: {} hosts, {} blocked ({} from ad lists), {} local-domains, {} redirects",
            hosts.len(),
            blocked.len(),
            ad_count,
            local_domains.len(),
            redirects.len()
        );
        Router {
            hosts,
            blocked,
            local_domains,
            redirects,
            hosts_ttl: cfg.hosts_ttl,
            reject_type65: cfg.reject_type65,
            special,
            lan,
        }
    }

    /// Re-read the lease + extra-hosts files and atomically swap the table in.
    /// No-op if the LAN feature is disabled. Returns the new host count.
    pub fn refresh_lan(&self) -> Option<usize> {
        let lan = self.lan.as_ref()?;
        let fresh = LanHosts::load(&lan.cfg);
        let n = fresh.len();
        lan.hosts.store(Arc::new(fresh));
        Some(n)
    }

    /// Seconds between LAN refreshes (0 = never), or None if disabled.
    pub fn lan_refresh_interval(&self) -> Option<u64> {
        self.lan.as_ref().map(|l| l.cfg.refresh_interval)
    }

    /// Forward/reverse LAN lookup. Returns an owned answer so the caller does
    /// not hold the arc-swap snapshot guard across await points.
    fn lan_decide(&self, qname: &str, qtype: u16) -> Option<Decision<'_>> {
        let lan = self.lan.as_ref()?;
        let ttl = lan.cfg.ttl;
        let snap = lan.hosts.load();
        if snap.is_empty() {
            return None;
        }
        if qtype == crate::dns::TYPE_PTR {
            let ip = lan::ptr_to_ip(qname)?;
            let name = snap.reverse(&ip)?;
            return Some(Decision::Lan(LanAnswer::Ptr(name.to_string()), ttl));
        }
        if qtype == crate::dns::TYPE_A || qtype == crate::dns::TYPE_AAAA {
            let ips = snap.lookup(qname)?;
            return Some(Decision::Lan(LanAnswer::Addrs(ips.clone()), ttl));
        }
        None
    }

    pub fn decide(&self, qname: &str, qtype: u16) -> Decision<'_> {
        if self.reject_type65 && qtype == crate::dns::TYPE_HTTPS {
            return Decision::Block;
        }
        if (qtype == crate::dns::TYPE_A || qtype == crate::dns::TYPE_AAAA)
            && !self.hosts.is_empty()
        {
            if let Some(ips) = self.hosts.get(qname) {
                return Decision::Hosts(ips);
            }
        }
        if !self.redirects.is_empty() {
            if let Some(to) = self.redirects.get(qname) {
                return Decision::Redirect(to);
            }
        }
        // LAN hits win over the special-TLD / private-PTR blocks below: a
        // `.lan` name or a private-range PTR that we actually know should be
        // answered, not NXDOMAIN'd as "site-local noise".
        if self.lan.is_some() {
            if let Some(d) = self.lan_decide(qname, qtype) {
                return d;
            }
        }
        if !self.blocked.is_empty() && self.blocked.matches(qname) {
            return Decision::Block;
        }
        if !self.special.is_empty() && self.special.matches(qname) {
            return Decision::Block;
        }
        if !self.local_domains.is_empty() && self.local_domains.matches(qname) {
            return Decision::Forward("local");
        }
        Decision::Forward("main")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_matching() {
        let mut s = DomainSet::default();
        s.suffix.insert("example.com".into());
        s.full.insert("only.example.org".into());
        assert!(s.matches("example.com"));
        assert!(s.matches("a.b.example.com"));
        assert!(!s.matches("notexample.com"));
        assert!(s.matches("only.example.org"));
        assert!(!s.matches("sub.only.example.org"));
    }
}
