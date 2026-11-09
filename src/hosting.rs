//! Who hosts a domain: map authoritative nameserver and CNAME target names
//! to well-known DNS / CDN providers (Cache Lookup diagnostics only).

/// (suffix or substring, provider). A rule starting with '.' matches a name
/// suffix (or the name itself without the dot); otherwise it is a substring.
const DNS_PROVIDERS: &[(&str, &str)] = &[
    (".ns.cloudflare.com", "Cloudflare"),
    ("awsdns-", "Amazon Route 53"),
    (".googledomains.com", "Google Cloud DNS"),
    (".google.com", "Google"),
    ("azure-dns.", "Azure DNS"),
    (".alidns.com", "Alibaba Cloud DNS"),
    (".hichina.com", "Alibaba Cloud DNS (HiChina)"),
    (".taobao.com", "Alibaba"),
    (".dnspod.net", "DNSPod (Tencent Cloud)"),
    (".dnspod.com", "DNSPod (Tencent Cloud)"),
    (".dnsv1.com", "DNSPod (Tencent Cloud)"),
    (".dnsv2.com", "DNSPod (Tencent Cloud)"),
    (".dnsv3.com", "DNSPod (Tencent Cloud)"),
    (".dnsv4.com", "DNSPod (Tencent Cloud)"),
    (".dnsv5.com", "DNSPod (Tencent Cloud)"),
    (".qq.com", "Tencent"),
    ("huaweicloud-dns.", "Huawei Cloud DNS"),
    (".bdydns.com", "Baidu Cloud DNS"),
    (".baidu.com", "Baidu"),
    (".jdcloud.com", "JD Cloud DNS"),
    (".akam.net", "Akamai Edge DNS"),
    (".nsone.net", "NS1"),
    ("ultradns.", "UltraDNS"),
    (".dynect.net", "Oracle Dyn"),
    (".domaincontrol.com", "GoDaddy"),
    (".registrar-servers.com", "Namecheap"),
    (".vercel-dns.com", "Vercel"),
    (".digitalocean.com", "DigitalOcean"),
    (".linode.com", "Akamai (Linode)"),
    (".hetzner.com", "Hetzner"),
    (".hetzner.de", "Hetzner"),
    (".ovh.net", "OVHcloud"),
    (".gandi.net", "Gandi"),
    (".he.net", "Hurricane Electric"),
    (".cloudns.net", "ClouDNS"),
    (".name.com", "Name.com"),
    (".dnsimple.com", "DNSimple"),
    (".dnsimple-edge.net", "DNSimple"),
    (".porkbun.com", "Porkbun"),
    (".squarespacedns.com", "Squarespace"),
    (".wixdns.net", "Wix"),
    (".github.com", "GitHub"),
    (".microsoftonline.com", "Microsoft"),
    (".msft.net", "Microsoft"),
    (".apple.com", "Apple"),
    (".facebook.com", "Meta"),
];

/// CNAME targets that reveal a CDN / hosting platform.
const CDN_PROVIDERS: &[(&str, &str)] = &[
    (".cdn.cloudflare.net", "Cloudflare"),
    (".cloudflare.net", "Cloudflare"),
    (".cloudfront.net", "Amazon CloudFront"),
    (".elb.amazonaws.com", "AWS Elastic Load Balancing"),
    (".amazonaws.com", "Amazon Web Services"),
    (".akamaiedge.net", "Akamai"),
    (".akamai.net", "Akamai"),
    (".edgekey.net", "Akamai"),
    (".edgesuite.net", "Akamai"),
    (".akamaized.net", "Akamai"),
    (".akadns.net", "Akamai"),
    (".fastly.net", "Fastly"),
    (".fastlylb.net", "Fastly"),
    (".azureedge.net", "Azure CDN"),
    (".azurefd.net", "Azure Front Door"),
    (".trafficmanager.net", "Azure Traffic Manager"),
    (".azurewebsites.net", "Azure App Service"),
    (".ghs.googlehosted.com", "Google"),
    (".googlehosted.com", "Google"),
    (".l.google.com", "Google"),
    (".kunlun", "Alibaba Cloud CDN"),
    (".alikunlun.com", "Alibaba Cloud CDN"),
    (".alicdn.com", "Alibaba Cloud CDN"),
    (".tbcache.com", "Alibaba Cloud CDN"),
    (".aliyuncs.com", "Alibaba Cloud"),
    (".cdn.dnsv1.com", "Tencent Cloud CDN"),
    (".cdntip.com", "Tencent Cloud CDN"),
    (".tencent-cloud.net", "Tencent Cloud"),
    (".shifen.com", "Baidu"),
    (".bcebos.com", "Baidu Cloud"),
    (".wscdns.com", "Wangsu (ChinaNetCenter)"),
    (".wswebcdn.com", "Wangsu (ChinaNetCenter)"),
    (".lxdns.com", "Wangsu (ChinaNetCenter)"),
    (".cdn20.com", "Wangsu (ChinaNetCenter)"),
    (".chinacache.net", "ChinaCache"),
    (".vercel-dns.com", "Vercel"),
    (".vercel.app", "Vercel"),
    (".netlify.app", "Netlify"),
    (".netlifyglobalcdn.com", "Netlify"),
    (".github.io", "GitHub Pages"),
    (".githubusercontent.com", "GitHub"),
    (".edgecastcdn.net", "Edgio"),
    (".cdn77.org", "CDN77"),
    (".b-cdn.net", "Bunny CDN"),
    (".herokudns.com", "Heroku"),
    (".shopify.com", "Shopify"),
    (".myshopify.com", "Shopify"),
    (".squarespace.com", "Squarespace"),
    (".wordpress.com", "WordPress.com"),
];

fn lookup(name: &str, table: &[(&str, &'static str)]) -> Option<&'static str> {
    let name = name.trim_end_matches('.');
    table.iter().find_map(|(rule, prov)| {
        let hit = if let Some(sfx) = rule.strip_prefix('.') {
            name == sfx || name.ends_with(rule)
        } else {
            name.contains(rule)
        };
        hit.then_some(*prov)
    })
}

/// Providers behind the authoritative nameservers of `zone`, in first-seen
/// order and deduplicated (multi-provider zones list each, "NS1 + Amazon
/// Route 53"). Unrecognised nameservers inside the zone itself
/// (ns1.example.com for example.com) count as self-hosted.
pub fn dns_provider(zone: &str, nameservers: &[String]) -> Option<String> {
    let suffix = format!(".{}", zone);
    let mut out: Vec<&str> = Vec::new();
    for ns in nameservers {
        let p = match lookup(ns, DNS_PROVIDERS) {
            Some(p) => p,
            None if !zone.is_empty() && ns.ends_with(&suffix) => "self-hosted",
            None => continue,
        };
        if !out.contains(&p) {
            out.push(p);
        }
    }
    (!out.is_empty()).then(|| out.join(" + "))
}

/// Cloudflare's published edge ranges (cloudflare.com/ips). Answers inside
/// them mean the site is proxied by Cloudflare even without a CNAME.
const CLOUDFLARE_V4: &[(u32, u8)] = &[
    (0xADF5_3000, 20), // 173.245.48.0/20
    (0x6715_F400, 22), // 103.21.244.0/22
    (0x6716_C800, 22), // 103.22.200.0/22
    (0x671F_0400, 22), // 103.31.4.0/22
    (0x8D65_4000, 18), // 141.101.64.0/18
    (0x6CA2_C000, 18), // 108.162.192.0/18
    (0xBE5D_F000, 20), // 190.93.240.0/20
    (0xBC72_6000, 20), // 188.114.96.0/20
    (0xC5EA_F000, 22), // 197.234.240.0/22
    (0xC629_8000, 17), // 198.41.128.0/17
    (0xA29E_0000, 15), // 162.158.0.0/15
    (0x6810_0000, 13), // 104.16.0.0/13
    (0x6818_0000, 14), // 104.24.0.0/14
    (0xAC40_0000, 13), // 172.64.0.0/13
    (0x8300_4800, 22), // 131.0.72.0/22
];
const CLOUDFLARE_V6: &[(u128, u8)] = &[
    (0x2400_cb00 << 96, 32),
    (0x2606_4700 << 96, 32),
    (0x2803_f800 << 96, 32),
    (0x2405_b500 << 96, 32),
    (0x2405_8100 << 96, 32),
    (0x2a06_98c0 << 96, 29),
    (0x2c0f_f248 << 96, 32),
];

/// Network that announces `ip`, for the few providers with stable published
/// ranges (currently Cloudflare).
pub fn ip_network(ip: &std::net::IpAddr) -> Option<&'static str> {
    let hit = match ip {
        std::net::IpAddr::V4(v4) => {
            let a = u32::from(*v4);
            CLOUDFLARE_V4.iter().any(|&(net, len)| a >> (32 - len) == net >> (32 - len))
        }
        std::net::IpAddr::V6(v6) => {
            let a = u128::from(*v6);
            CLOUDFLARE_V6.iter().any(|&(net, len)| a >> (128 - len) == net >> (128 - len))
        }
    };
    hit.then_some("Cloudflare")
}

/// CDN / platform revealed by the last recognisable CNAME in the chain.
pub fn cdn_provider(cname_targets: &[String]) -> Option<&'static str> {
    cname_targets.iter().rev().find_map(|t| lookup(t, CDN_PROVIDERS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers() {
        let ns = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            dns_provider("example.org", &ns(&["ada.ns.cloudflare.com", "bob.ns.cloudflare.com"])).as_deref(),
            Some("Cloudflare")
        );
        assert_eq!(
            dns_provider("amazon.com", &ns(&["ns-1.awsdns-01.org"])).as_deref(),
            Some("Amazon Route 53")
        );
        assert_eq!(
            dns_provider("example.cn", &ns(&["f1g1ns1.dnspod.net"])).as_deref(),
            Some("DNSPod (Tencent Cloud)")
        );
        assert_eq!(
            dns_provider("example.com", &ns(&["ns1.example.com", "ns2.example.com"])).as_deref(),
            Some("self-hosted")
        );
        assert_eq!(dns_provider("example.com", &ns(&["ns1.unknown-dns.example"])), None);
        assert_eq!(
            dns_provider("github.com", &ns(&["dns1.p08.nsone.net", "dns2.p08.nsone.net", "ns-1.awsdns-32.org"])).as_deref(),
            Some("NS1 + Amazon Route 53")
        );
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        assert_eq!(ip_network(&ip("104.16.132.229")), Some("Cloudflare"));
        assert_eq!(ip_network(&ip("172.67.1.1")), Some("Cloudflare"));
        assert_eq!(ip_network(&ip("2606:4700::6810:84e5")), Some("Cloudflare"));
        assert_eq!(ip_network(&ip("8.8.8.8")), None);
        assert_eq!(ip_network(&ip("104.24.0.0")), Some("Cloudflare"));
        assert_eq!(ip_network(&ip("104.28.0.0")), None);
        // "notcloudflare.com" must not match the ".cloudflare.com" suffix rule
        assert_eq!(dns_provider("x.com", &ns(&["ns.notns.cloudflare.com.evil"])), None);
        assert_eq!(
            cdn_provider(&ns(&["www.a.shifen.com", "www.wshifen.com"])),
            Some("Baidu")
        );
        assert_eq!(cdn_provider(&ns(&["d1.cloudfront.net"])), Some("Amazon CloudFront"));
        assert_eq!(cdn_provider(&ns(&["foo.example.net"])), None);
    }
}
