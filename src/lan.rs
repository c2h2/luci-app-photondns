//! LAN host resolution: answer names for local DHCP clients directly, without
//! forwarding to (or depending on) dnsmasq.
//!
//! Two sources are merged into one table:
//!   * the dnsmasq DHCP lease file (`/tmp/dhcp.leases`) - learned automatically,
//!     re-read periodically so new/renewed leases appear without a restart;
//!   * a static "extra hosts" file (`name ip [ip...]` per line) - for pinning
//!     names a device never advertises over DHCP (e.g. a laptop as c2h2mbp16).
//!
//! Each name resolves under both its bare label and `<name>.<suffix>` (default
//! `lan`), matching dnsmasq's `expand-hosts` behaviour. Reverse PTR lookups of
//! the mapped IPs answer with `<name>.<suffix>`.

use crate::config::LanCfg;
use rustc_hash::FxHashMap;
use std::net::IpAddr;
use std::path::Path;

/// Forward + reverse view of the LAN, rebuilt atomically on each refresh.
#[derive(Default)]
pub struct LanHosts {
    /// lowercase name (bare and `.suffix`) -> IPs
    forward: FxHashMap<String, Vec<IpAddr>>,
    /// IP -> fully-qualified `name.suffix` (for PTR)
    reverse: FxHashMap<IpAddr, String>,
}

impl LanHosts {
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// Number of distinct hosts (reverse map has one entry per IP).
    pub fn len(&self) -> usize {
        self.reverse.len()
    }

    /// Forward lookup. `name` must be lowercase, no trailing dot.
    pub fn lookup(&self, name: &str) -> Option<&Vec<IpAddr>> {
        self.forward.get(name)
    }

    /// Reverse lookup: the FQDN for an IP, if known.
    pub fn reverse(&self, ip: &IpAddr) -> Option<&str> {
        self.reverse.get(ip).map(|s| s.as_str())
    }

    /// Build the table from the configured lease + extra-hosts files.
    pub fn load(cfg: &LanCfg) -> LanHosts {
        let suffix = cfg.suffix.trim_matches('.').to_ascii_lowercase();
        let mut b = Builder::new(&suffix);
        if !cfg.extra_hosts_file.is_empty() {
            b.add_hosts_file(&cfg.extra_hosts_file);
        }
        if !cfg.leases_file.is_empty() {
            b.add_leases_file(&cfg.leases_file);
        }
        b.finish()
    }
}

struct Builder<'a> {
    suffix: &'a str,
    forward: FxHashMap<String, Vec<IpAddr>>,
    reverse: FxHashMap<IpAddr, String>,
}

impl<'a> Builder<'a> {
    fn new(suffix: &'a str) -> Self {
        Builder {
            suffix,
            forward: FxHashMap::default(),
            reverse: FxHashMap::default(),
        }
    }

    /// Register one host: forward entries for both the bare label and the
    /// FQDN, and (unless already claimed) a reverse entry per IP. The first
    /// source to claim an IP wins the PTR, so extra-hosts pins (loaded first)
    /// take precedence over an auto-learned lease for the same address.
    fn add(&mut self, name: &str, ips: &[IpAddr]) {
        let bare = sanitize(name);
        if bare.is_empty() || ips.is_empty() {
            return;
        }
        let fqdn = if self.suffix.is_empty() {
            bare.clone()
        } else {
            format!("{}.{}", bare, self.suffix)
        };
        for key in [bare.as_str(), fqdn.as_str()] {
            let e = self.forward.entry(key.to_string()).or_default();
            for &ip in ips {
                if !e.contains(&ip) {
                    e.push(ip);
                }
            }
        }
        for &ip in ips {
            self.reverse.entry(ip).or_insert_with(|| fqdn.clone());
        }
    }

    /// dnsmasq leases: `<expiry> <mac> <ip> <name> <clientid>`. A `*` name
    /// means the client sent none - skip it (nothing to resolve).
    fn add_leases_file(&mut self, path: &str) {
        if !Path::new(path).exists() {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            log::warn!("lan: cannot read leases {}", path);
            return;
        };
        for line in text.lines() {
            let mut it = line.split_whitespace();
            let (_, _, ip, name) = match (it.next(), it.next(), it.next(), it.next()) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => continue,
            };
            if name == "*" {
                continue;
            }
            if let Ok(ip) = ip.parse::<IpAddr>() {
                self.add(name, &[ip]);
            }
        }
    }

    /// Static hosts: `name ip [ip...]`, '#' comments. Same format as the
    /// routing hosts file, but scoped to the LAN suffix.
    fn add_hosts_file(&mut self, path: &str) {
        if !Path::new(path).exists() {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            log::warn!("lan: cannot read extra hosts {}", path);
            return;
        };
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let mut it = line.split_whitespace();
            let Some(name) = it.next() else { continue };
            let ips: Vec<IpAddr> = it.filter_map(|t| t.parse().ok()).collect();
            self.add(name, &ips);
        }
    }

    fn finish(self) -> LanHosts {
        LanHosts {
            forward: self.forward,
            reverse: self.reverse,
        }
    }
}

/// Normalise a host label: lowercase, drop a trailing dot, and keep only the
/// first component if a device advertised a dotted FQDN (dnsmasq stores just
/// the host part, but leases occasionally carry `host.domain`).
fn sanitize(name: &str) -> String {
    let n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    match n.split_once('.') {
        Some((head, _)) => head.to_string(),
        None => n,
    }
}

/// Turn a PTR qname (`4.10.16.172.in-addr.arpa` / nibble ip6.arpa) back into an
/// `IpAddr`, so we can look it up in the reverse map. Returns None if the name
/// is not a well-formed reverse pointer.
pub fn ptr_to_ip(qname: &str) -> Option<IpAddr> {
    if let Some(rest) = qname.strip_suffix(".in-addr.arpa") {
        let mut octets = rest.split('.').collect::<Vec<_>>();
        if octets.len() != 4 {
            return None;
        }
        octets.reverse();
        let s = octets.join(".");
        return s.parse::<std::net::Ipv4Addr>().ok().map(IpAddr::V4);
    }
    if let Some(rest) = qname.strip_suffix(".ip6.arpa") {
        let nibbles: Vec<&str> = rest.split('.').collect();
        if nibbles.len() != 32 {
            return None;
        }
        let mut hex = String::with_capacity(32);
        for n in nibbles.iter().rev() {
            if n.len() != 1 || !n.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            hex.push_str(n);
        }
        let bytes = u128::from_str_radix(&hex, 16).ok()?;
        return Some(IpAddr::V6(std::net::Ipv6Addr::from(bytes)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(suffix: &str) -> LanCfg {
        LanCfg {
            enabled: true,
            leases_file: String::new(),
            extra_hosts_file: String::new(),
            suffix: suffix.to_string(),
            refresh_interval: 30,
            ttl: 60,
        }
    }

    #[test]
    fn lease_parsing_and_suffix() {
        let mut b = Builder::new("lan");
        // named lease resolves; '*' lease is skipped
        b.add_leases_line("1783551318 a8:51:ab:12:fb:dc 172.16.11.5 Bedroom 01:a8");
        let lan = {
            b.add_leases_line("1783552194 3e:21:94:51:bb:a1 172.16.10.47 * 01:3e");
            b.finish()
        };
        let ip: IpAddr = "172.16.11.5".parse().unwrap();
        assert_eq!(lan.lookup("bedroom"), Some(&vec![ip]));
        assert_eq!(lan.lookup("bedroom.lan"), Some(&vec![ip]));
        assert_eq!(lan.reverse(&ip), Some("bedroom.lan"));
        // the '*' lease produced nothing
        assert_eq!(lan.len(), 1);
    }

    #[test]
    fn extra_host_pin_wins_ptr() {
        let mut b = Builder::new("lan");
        b.add_hosts_line("c2h2mbp16 172.16.10.92"); // pin first
        b.add_leases_line("1 mac 172.16.10.92 MacBookAir cid"); // lease same IP
        let lan = b.finish();
        let ip: IpAddr = "172.16.10.92".parse().unwrap();
        // both names resolve forward...
        assert_eq!(lan.lookup("c2h2mbp16"), Some(&vec![ip]));
        assert_eq!(lan.lookup("macbookair"), Some(&vec![ip]));
        // ...but the pin (added first) owns the PTR
        assert_eq!(lan.reverse(&ip), Some("c2h2mbp16.lan"));
    }

    #[test]
    fn ptr_roundtrip_v4() {
        let ip = ptr_to_ip("4.10.16.172.in-addr.arpa").unwrap();
        assert_eq!(ip, "172.16.10.4".parse::<IpAddr>().unwrap());
        assert!(ptr_to_ip("4.10.16.in-addr.arpa").is_none());
        assert!(ptr_to_ip("example.com").is_none());
    }

    #[test]
    fn ptr_roundtrip_v6() {
        // ::1 reversed as nibbles
        let name = "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.ip6.arpa";
        assert_eq!(ptr_to_ip(name).unwrap(), "::1".parse::<IpAddr>().unwrap());
        assert!(ptr_to_ip("z.ip6.arpa").is_none());
    }

    #[test]
    fn dotted_lease_name_uses_host_part() {
        let mut b = Builder::new("lan");
        b.add_leases_line("1 mac 10.0.0.5 laptop.example cid");
        let lan = b.finish();
        assert!(lan.lookup("laptop").is_some());
        assert!(lan.lookup("laptop.example").is_none());
    }

    // small test shims so tests can feed single lines without touching disk
    impl<'a> Builder<'a> {
        fn add_leases_line(&mut self, line: &str) {
            let mut it = line.split_whitespace();
            if let (Some(_), Some(_), Some(ip), Some(name)) =
                (it.next(), it.next(), it.next(), it.next())
            {
                if name != "*" {
                    if let Ok(ip) = ip.parse::<IpAddr>() {
                        self.add(name, &[ip]);
                    }
                }
            }
        }
        fn add_hosts_line(&mut self, line: &str) {
            let line = line.split('#').next().unwrap_or("").trim();
            let mut it = line.split_whitespace();
            if let Some(name) = it.next() {
                let ips: Vec<IpAddr> = it.filter_map(|t| t.parse().ok()).collect();
                self.add(name, &ips);
            }
        }
    }
}
