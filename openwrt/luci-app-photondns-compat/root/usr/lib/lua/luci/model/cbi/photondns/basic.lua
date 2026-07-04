-- Basic Settings (CBI) — legacy port of basic.js
local m, s, o

m = Map("photondns", translate("photondns"),
	translate("High-performance DNS forwarder written in Rust: configurable cache, serve-stale, prefetch, " ..
		"and adaptive failover that races multiple upstreams (UDP/TCP/DoT/DoH) and takes the fastest answer."))

-- live service-status line
local sv = m:section(SimpleSection)
sv.template = "photondns/status_bar"

s = m:section(NamedSection, "main", "photondns")
s.addremove = false
s.anonymous = true

s:tab("basic",    translate("Basic Options"))
s:tab("upstream", translate("Upstreams"))
s:tab("failover", translate("Failover"))
s:tab("cache",    translate("Cache"))
s:tab("adblock",  translate("Ad Block"))

---- basic ----
o = s:taboption("basic", Flag, "enabled", translate("Enabled"))
o.default = "0"
o.rmempty = false

o = s:taboption("basic", Value, "listen_address", translate("Listen Address"))
o.default = "0.0.0.0"

o = s:taboption("basic", Value, "listen_port", translate("Listen Port"))
o.datatype = "port"
o.default = "15533"

o = s:taboption("basic", ListValue, "log_level", translate("Log Level"))
o:value("debug", translate("Debug"))
o:value("info", translate("Info"))
o:value("warn", translate("Warning"))
o:value("error", translate("Error"))
o.default = "info"

o = s:taboption("basic", Value, "log_file", translate("Log File"))
o.default = "/var/log/photondns.log"

o = s:taboption("basic", Value, "api_port", translate("API Port"),
	translate("Local HTTP API for statistics and cache control (127.0.0.1 only)"))
o.datatype = "port"
o.default = "8053"

o = s:taboption("basic", Flag, "redirect", translate("DNS Forward"),
	translate("Forward dnsmasq DNS resolution requests to photondns"))
o.default = "0"
o.rmempty = false

o = s:taboption("basic", Flag, "dns_hijack", translate("DNS Redirect (Hijack)"),
	translate("Force redirect all LAN DNS queries (UDP port 53) to photondns via firewall"))
o.default = "0"
o:depends("redirect", "1")

o = s:taboption("basic", Flag, "reject_type65", translate("Disable RR Type 65 (HTTPS/SVCB)"),
	translate("Answer HTTPS/SVCB queries with NXDOMAIN, forcing clients onto plain A/AAAA records"))
o.default = "0"

o = s:taboption("basic", Value, "query_log_size", translate("Query log size"),
	translate("Number of recent queries kept in memory for the Query Log page; 0 disables it"))
o.datatype = "and(uinteger,max(65536))"
o.default = "5000"

o = s:taboption("basic", Flag, "auto_update", translate("Auto-update lists"),
	translate("Refresh the enabled China / ad-block lists on a schedule (cron)"))
o.default = "0"

o = s:taboption("basic", ListValue, "update_day", translate("Update day"))
o:value("*", translate("Every day"))
o:value("0", translate("Sunday"))
o:value("1", translate("Monday"))
o:value("2", translate("Tuesday"))
o:value("3", translate("Wednesday"))
o:value("4", translate("Thursday"))
o:value("5", translate("Friday"))
o:value("6", translate("Saturday"))
o.default = "*"
o:depends("auto_update", "1")

o = s:taboption("basic", Value, "update_time", translate("Update hour (0-23)"))
o.datatype = "and(uinteger,max(23))"
o.default = "4"
o:depends("auto_update", "1")

---- upstream ----
o = s:taboption("upstream", DynamicList, "upstream", translate("Primary DNS servers"),
	translate("Formats: 1.2.3.4, udp://host, tcp://host, tls://host (DoT), https://host/dns-query (DoH)"))
o:value("udp://223.5.5.5", translate("AliDNS (UDP 223.5.5.5)"))
o:value("udp://119.29.29.29", translate("Tencent DNSPod (UDP 119.29.29.29)"))
o:value("udp://114.114.114.114", translate("114DNS (UDP)"))
o:value("tls://223.5.5.5", translate("AliDNS (DoT)"))
o:value("tls://1.12.12.12", translate("DNSPod (DoT)"))
o:value("https://223.5.5.5/dns-query", translate("AliDNS (DoH)"))
o:value("tls://8.8.8.8", translate("Google (DoT)"))
o:value("tls://1.1.1.1", translate("Cloudflare (DoT)"))
o:value("https://dns.google/dns-query", translate("Google (DoH)"))
o.rmempty = false

o = s:taboption("upstream", DynamicList, "backup_upstream", translate("Backup DNS servers"),
	translate("Only used when primary servers fail; also raced as a last hedge candidate"))
o:value("tls://8.8.8.8", translate("Google (DoT)"))
o:value("tls://1.1.1.1", translate("Cloudflare (DoT)"))
o:value("udp://9.9.9.9", translate("Quad9 (UDP)"))

o = s:taboption("upstream", DynamicList, "local_upstream", translate("Local-domain DNS servers"),
	translate("Optional group used for domains listed in the local_domains rule file (e.g. China DNS)"))
o:value("udp://223.5.5.5", translate("AliDNS (UDP 223.5.5.5)"))
o:value("udp://119.29.29.29", translate("Tencent DNSPod (UDP 119.29.29.29)"))

o = s:taboption("upstream", Flag, "china_list", translate("China domain list (split DNS)"),
	translate("Route mainland-China domains to the Local-domain DNS servers, everything else to the primary servers. Uses the dnsmasq-china-list (~70k domains)."))
o.default = "0"

o = s:taboption("upstream", DummyValue, "_chinalist_status", translate("China list status"))
o:depends("china_list", "1")
o.rawhtml = true
o.cfgvalue = function(self, section)
	local fs = require "nixio.fs"
	local st = fs.stat("/etc/photondns/china_list.txt")
	if not st then
		return translate("not downloaded yet - use the List Updates page to download it")
	end
	local n = tonumber((luci.sys.exec("wc -l < /etc/photondns/china_list.txt 2>/dev/null") or ""):match("%d+")) or 0
	return string.format("%d domains, updated %s", n, os.date("%Y-%m-%d %H:%M:%S", st.mtime))
end

o = s:taboption("upstream", Value, "bootstrap_dns", translate("Bootstrap DNS"),
	translate("Plain DNS server used to resolve DoT/DoH hostnames"))
o.default = "223.5.5.5"

o = s:taboption("upstream", Flag, "insecure_skip_verify", translate("Disable TLS verification"),
	translate("Skip DoT/DoH certificate validation (useful if the system clock/CA store is broken)"))
o.default = "0"

o = s:taboption("upstream", Value, "idle_timeout", translate("Connection idle timeout (s)"),
	translate("How long pooled TCP/DoT/DoH connections stay open"))
o.datatype = "and(uinteger,min(5))"
o.default = "30"

---- adblock ----
o = s:taboption("adblock", Flag, "adblock", translate("Enable DNS ad blocking"),
	translate("Answer known advertising / tracker domains with NXDOMAIN"))
o.default = "0"
o.rmempty = false

o = s:taboption("adblock", DynamicList, "ad_source", translate("Ad list sources"),
	translate("Plain domain lists, mosdns domain:/full: lists or hosts-format files, one URL per entry"))
o:value("https://cdn.jsdelivr.net/gh/privacy-protection-tools/anti-AD@master/anti-ad-domains.txt", "anti-AD (jsdelivr)")
o:value("https://raw.githubusercontent.com/privacy-protection-tools/anti-AD/master/anti-ad-domains.txt", "anti-AD (github)")
o:value("https://cdn.jsdelivr.net/gh/Cats-Team/AdRules@main/mosdns_adrules.txt", "Cats-Team AdRules (jsdelivr)")
o:value("https://raw.githubusercontent.com/Cats-Team/AdRules/main/mosdns_adrules.txt", "Cats-Team AdRules (github)")
o:value("https://raw.githubusercontent.com/neodevpro/neodevhost/master/domain", "NEO DEV HOST")
o:depends("adblock", "1")

o = s:taboption("adblock", DummyValue, "_adlist_status", translate("Ad list status"))
o:depends("adblock", "1")
o.rawhtml = true
o.cfgvalue = function(self, section)
	local fs = require "nixio.fs"
	local st = fs.stat("/etc/photondns/ad_list.txt")
	if not st then
		return translate("not downloaded yet - use the List Updates page to download it")
	end
	local n = tonumber((luci.sys.exec("wc -l < /etc/photondns/ad_list.txt 2>/dev/null") or ""):match("%d+")) or 0
	return string.format("%d domains, updated %s", n, os.date("%Y-%m-%d %H:%M:%S", st.mtime))
end

---- failover ----
o = s:taboption("failover", ListValue, "strategy", translate("Strategy"),
	translate("race: hedged racing, best latency (recommended). fastest: always lowest-latency upstream. " ..
		"parallel: ask all at once. sequential: strict configured order. random: uniform spread."))
o:value("race", translate("race (adaptive hedging)"))
o:value("fastest", translate("fastest (lowest EWMA)"))
o:value("parallel", translate("parallel (all at once)"))
o:value("sequential", translate("sequential (ordered)"))
o:value("random", translate("random"))
o.default = "race"

o = s:taboption("failover", Value, "hedge_delay", translate("Max hedge delay (ms)"),
	translate("Upper bound before racing the next upstream; actual delay adapts to ~2x the best upstream latency"))
o.datatype = "and(uinteger,min(10))"
o.default = "250"

o = s:taboption("failover", Value, "query_timeout", translate("Query timeout (ms)"))
o.datatype = "and(uinteger,min(100))"
o.default = "2000"

o = s:taboption("failover", Value, "health_check_interval", translate("Health check interval (s)"),
	translate("Active probes keep latency stats fresh and detect dead upstreams even when idle"))
o.datatype = "and(uinteger,min(2))"
o.default = "10"

o = s:taboption("failover", Value, "health_check_domain", translate("Health check domain"))
o.default = "www.gstatic.com"

o = s:taboption("failover", Value, "fail_threshold", translate("Failure threshold"),
	translate("Consecutive failures before an upstream is taken out of rotation"))
o.datatype = "and(uinteger,min(1))"
o.default = "3"

o = s:taboption("failover", Value, "recover_threshold", translate("Recovery threshold"),
	translate("Consecutive successes before a down upstream is restored"))
o.datatype = "and(uinteger,min(1))"
o.default = "2"

o = s:taboption("failover", Value, "cooldown", translate("Cooldown (s)"),
	translate("How long a down upstream is excluded before a half-open retry"))
o.datatype = "and(uinteger,min(1))"
o.default = "15"

---- cache ----
o = s:taboption("cache", Flag, "cache", translate("Enable DNS cache"))
o.default = "1"
o.rmempty = false

o = s:taboption("cache", ListValue, "cache_size", translate("Cache size (entries)"),
	translate("Maximum number of cached responses (sharded LRU). RAM shown is the ceiling when the cache is fully populated (~400 B/entry); actual use grows lazily."))
o:value("8192", translate("8192 (~3 MB)"))
o:value("16384", translate("16384 (~6 MB)"))
o:value("32768", translate("32768 (~13 MB)"))
o:value("65536", translate("65536 (~25 MB)"))
o:value("131072", translate("131072 (~50 MB)"))
o:value("262144", translate("262144 (~100 MB)"))
o.default = "65536"
o:depends("cache", "1")

o = s:taboption("cache", Value, "min_ttl", translate("Minimum TTL (s)"),
	translate("Raise very low TTLs to this value; 0 = no change"))
o.datatype = "and(uinteger,min(0),max(604800))"
o.default = "0"
o:depends("cache", "1")

o = s:taboption("cache", Value, "max_ttl", translate("Maximum TTL (s)"),
	translate("Cap TTLs at this value; 0 = no cap"))
o.datatype = "and(uinteger,min(0),max(604800))"
o.default = "86400"
o:depends("cache", "1")

o = s:taboption("cache", Value, "negative_ttl", translate("Negative cache TTL (s)"),
	translate("How long NXDOMAIN / empty answers are cached"))
o.datatype = "and(uinteger,min(0))"
o.default = "30"
o:depends("cache", "1")

o = s:taboption("cache", Flag, "serve_stale", translate("Serve stale (lazy cache)"),
	translate("Answer instantly from expired entries and refresh in the background - also keeps DNS working when all upstreams are down"))
o.default = "1"
o:depends("cache", "1")

o = s:taboption("cache", Value, "stale_ttl", translate("Stale lifetime (s)"),
	translate("How long past expiry an entry may still be served"))
o.datatype = "and(uinteger,min(0))"
o.default = "86400"
o:depends("serve_stale", "1")

o = s:taboption("cache", Flag, "prefetch", translate("Prefetch popular entries"),
	translate("Refresh frequently used entries shortly before they expire, so they never go stale"))
o.default = "1"
o:depends("cache", "1")

o = s:taboption("cache", Flag, "dump_cache", translate("Persist cache to disk"),
	translate("Save the cache on shutdown and periodically; restore it on startup"))
o.default = "1"
o:depends("cache", "1")

o = s:taboption("cache", Value, "dump_interval", translate("Cache save interval (s)"))
o.datatype = "and(uinteger,min(60))"
o.default = "3600"
o:depends("dump_cache", "1")

function m.on_after_commit(self)
	luci.sys.call("/etc/init.d/photondns reload >/dev/null 2>&1")
end

return m
