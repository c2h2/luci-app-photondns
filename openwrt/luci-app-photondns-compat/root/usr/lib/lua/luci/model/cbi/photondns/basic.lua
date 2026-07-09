-- Basic Settings (CBI) — legacy port of basic.js
local m, s, o

-- inline list status + download/update button; polls the controller's JSON
-- endpoints (api/listupdate, api/liststatus) the same way the modern JS app
-- polls rpcd
local function list_widget(which, file)
	local fs = require "nixio.fs"
	local disp = require "luci.dispatcher"
	local st = fs.stat(file)
	local status_html
	if st then
		local n = tonumber((luci.sys.exec("wc -l < " .. file .. " 2>/dev/null") or ""):match("%d+")) or 0
		status_html = string.format("<strong>%s</strong>",
			translatef("%d domains, updated %s", n, os.date("%Y-%m-%d %H:%M:%S", st.mtime)))
	else
		status_html = string.format("<em style=\"color:#d43f3a\">%s</em>", translate("not downloaded yet"))
	end
	local btn_label = st and translate("Update Now") or translate("Download Now")
	local upd_url = disp.build_url("admin/services/photondns/api/listupdate") .. "?list=" .. which
	local st_url = disp.build_url("admin/services/photondns/api/liststatus") .. "?list=" .. which
	local script = [==[
<script type="text/javascript">//<![CDATA[
if (!window.photondnsListUpdate) {
	window.photondnsI18n = {
		updating: ']==] .. translate("Updating...") .. [==[',
		done: ']==] .. translate("%d domains, updated %s") .. [==[',
		fail: ']==] .. translate("Download failed - check the log on the List Updates page.") .. [==[',
		startfail: ']==] .. translate("Update failed to start: %s") .. [==[',
		updatenow: ']==] .. translate("Update Now") .. [==['
	};
	window.photondnsListUpdate = function(key, updUrl, stUrl) {
		var I = window.photondnsI18n;
		var btn = document.getElementById(key + '_btn');
		var span = document.getElementById(key + '_st');
		var G = function(url, cb) {
			var x = new XMLHttpRequest();
			x.open('GET', url, true);
			x.onreadystatechange = function() {
				if (x.readyState != 4) return;
				var d = null;
				try { d = JSON.parse(x.responseText); } catch (e) {}
				cb(d);
			};
			x.send(null);
		};
		btn.disabled = true;
		G(updUrl, function(res) {
			if (!res || !res.success) {
				btn.disabled = false;
				alert(I.startfail.replace('%s', (res && res.error) || '?'));
				return;
			}
			span.innerHTML = '<em style="color:#c7a500">' + I.updating + '</em>';
			var t = setInterval(function() {
				G(stUrl, function(st) {
					if (!st || st.updating) return;
					clearInterval(t);
					btn.disabled = false;
					if (st.exists) {
						btn.value = I.updatenow;
						span.innerHTML = '<strong>' + I.done.replace('%d', st.domains)
							.replace('%s', new Date(st.mtime * 1000).toLocaleString()) + '</strong>';
					} else {
						span.innerHTML = '<em style="color:#d43f3a">' + I.fail + '</em>';
					}
				});
			}, 2000);
		});
	};
}
//]]></script>]==]
	return string.format(
		'<span id="%s_st">%s</span>' ..
		'<input type="button" class="cbi-button cbi-button-apply" id="%s_btn" ' ..
		'style="margin-left:12px" value="%s" onclick="photondnsListUpdate(\'%s\',\'%s\',\'%s\')" />%s',
		which, status_html, which, btn_label, which, upd_url, st_url, script)
end

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
s:tab("failover", translate("Failover & Advanced"))
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

o = s:taboption("basic", Flag, "udp", translate("Listen on UDP"),
	translate("Serve classic DNS over UDP on the listen port"))
o.default = "1"
o.rmempty = false

o = s:taboption("basic", Flag, "tcp", translate("Listen on TCP"),
	translate("Serve DNS over TCP on the listen port (needed for large answers)"))
o.default = "1"
o.rmempty = false

o = s:taboption("basic", Flag, "doh", translate("DoH Server"),
	translate("Serve DNS-over-HTTPS (RFC 8484) on the DoH port. Runs plain HTTP unless a certificate is set below - put a TLS reverse proxy (Caddy/nginx) in front, or set cert + key to serve HTTPS directly"))
o.default = "0"
o.rmempty = false

o = s:taboption("basic", Value, "doh_port", translate("DoH Port"))
o.datatype = "port"
o.default = "8054"
o:depends("doh", "1")

o = s:taboption("basic", Value, "doh_path", translate("DoH Path"))
o.default = "/dns-query"
o:depends("doh", "1")

o = s:taboption("basic", Value, "doh_cert", translate("DoH TLS Certificate"),
	translate("Path to a PEM certificate chain; leave empty when behind a reverse proxy"))
o:depends("doh", "1")

o = s:taboption("basic", Value, "doh_key", translate("DoH TLS Key"),
	translate("Path to the PEM private key"))
o:depends("doh", "1")

o = s:taboption("basic", ListValue, "log_level", translate("Log Level"),
	translate("Verbosity of the daemon log."))
o:value("debug", translate("Debug"))
o:value("info", translate("Info"))
o:value("warn", translate("Warning"))
o:value("error", translate("Error"))
o.default = "info"

o = s:taboption("basic", Value, "log_file", translate("Log File"),
	translate("Daemon log path. Leave empty to log to the system log (logread) only - no file. Note: /var on OpenWrt is RAM (tmpfs), so a file here lives in memory and is cleared on reboot."))
o.default = "/var/log/photondns.log"
o.rmempty = false

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

o = s:taboption("basic", ListValue, "aaaa_mode", translate("IPv6 (AAAA) handling"),
	translate("Whether to answer IPv6 (AAAA) lookups. \"Block if IPv4 exists\" suppresses IPv6 only for names that also have an IPv4 address, so IPv6-only sites still work; \"Block all\" forces every client onto IPv4."))
o:value("allow", translate("Allow (default)"))
o:value("block_if_ipv4", translate("Block if IPv4 exists"))
o:value("block_all", translate("Block all IPv6"))
o.default = "allow"

o = s:taboption("basic", Flag, "prewarm", translate("Prewarm popular domains"),
	translate("Keep a list of domains (default: YouTube/Google) always resolved so a first visit is never a slow cold miss. Edit the list under Rules."))
o.default = "1"

o = s:taboption("basic", Value, "prewarm_interval", translate("Prewarm interval (s)"),
	translate("How often to refresh the prewarm domains; keep it below the cache stale lifetime so they never fully expire. 0 = only at startup."))
o.datatype = "and(uinteger,min(0),max(86400))"
o.default = "3000"
o:depends("prewarm", "1")

o = s:taboption("basic", Flag, "lan_hosts", translate("Resolve LAN hostnames"),
	translate("Answer DNS for local network devices by name (learned from DHCP leases) plus a manual pin list, without depending on dnsmasq. Names resolve both bare and under the LAN suffix, and reverse (PTR) lookups work too. Edit the pin list under Rules."))
o.default = "1"
o.rmempty = false

o = s:taboption("basic", Value, "lan_suffix", translate("LAN domain suffix"),
	translate('Domain LAN names also answer under, e.g. "lan" makes a host "nas" resolve as both nas and nas.lan.'))
o.default = "lan"
o:depends("lan_hosts", "1")

o = s:taboption("basic", Value, "lan_leases", translate("DHCP lease file"),
	translate("dnsmasq lease file to learn hostname → IP mappings from; re-read periodically."))
o.default = "/tmp/dhcp.leases"
o:depends("lan_hosts", "1")

o = s:taboption("basic", Value, "lan_refresh", translate("LAN refresh interval (s)"),
	translate("How often to re-read the lease file so new devices appear; 0 = only at startup."))
o.datatype = "and(uinteger,min(0),max(86400))"
o.default = "30"
o:depends("lan_hosts", "1")

o = s:taboption("basic", Value, "lan_ttl", translate("LAN record TTL (s)"),
	translate("Answer TTL for LAN forward/reverse records."))
o.datatype = "and(uinteger,min(1),max(86400))"
o.default = "60"
o:depends("lan_hosts", "1")

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
o = s:taboption("upstream", ListValue, "china_list", translate("Routing mode"),
	translate("Split mode resolves mainland-China domains (dnsmasq-china-list, ~70k entries) via the China DNS group and everything else via the overseas group"))
o:value("1", translate("China / overseas split DNS (recommended)"))
o:value("0", translate("Single group - all domains use the same servers"))
o.default = "1"
o.rmempty = false

o = s:taboption("upstream", DynamicList, "local_upstream", translate("China DNS servers"),
	translate("Used for mainland-China domains and the Local Domains rule file"))
o:value("udp://223.5.5.5", translate("AliDNS (UDP 223.5.5.5)"))
o:value("udp://119.29.29.29", translate("Tencent DNSPod (UDP 119.29.29.29)"))
o:value("udp://114.114.114.114", translate("114DNS (UDP)"))
o:value("tls://223.5.5.5", translate("AliDNS (DoT)"))
o:value("tls://1.12.12.12", translate("DNSPod (DoT)"))
o.default = "udp://223.5.5.5"
o:depends("china_list", "1")

o = s:taboption("upstream", DummyValue, "_chinalist_status", translate("China list status"))
o:depends("china_list", "1")
o.rawhtml = true
o.cfgvalue = function(self, section)
	return list_widget("chinalist", "/etc/photondns/china_list.txt")
end

o = s:taboption("upstream", DynamicList, "upstream", translate("Overseas DNS servers"),
	translate("Used for all other domains (all domains in single-group mode). Formats: 1.2.3.4, udp://host, tcp://host, tls://host (DoT), https://host/dns-query (DoH)"))
o:value("tcp://1.1.1.1", translate("Cloudflare (TCP)"))
o:value("tls://1.1.1.1", translate("Cloudflare (DoT)"))
o:value("tcp://8.8.8.8", translate("Google (TCP)"))
o:value("tls://8.8.8.8", translate("Google (DoT)"))
o:value("https://dns.google/dns-query", translate("Google (DoH)"))
o:value("udp://9.9.9.9", translate("Quad9 (UDP)"))
o.default = "tcp://1.1.1.1"
o.rmempty = false

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
	return list_widget("adlist", "/etc/photondns/ad_list.txt")
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
o.default = "5000"

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

o = s:taboption("failover", DynamicList, "backup_upstream", translate("Backup DNS servers"),
	translate("Only used when primary servers fail; also raced as a last hedge candidate"))
o:value("tls://223.5.5.5", translate("AliDNS (DoT)"))
o:value("tls://8.8.8.8", translate("Google (DoT)"))
o:value("tls://1.1.1.1", translate("Cloudflare (DoT)"))
o:value("udp://9.9.9.9", translate("Quad9 (UDP)"))

o = s:taboption("failover", Value, "bootstrap_dns", translate("Bootstrap DNS"),
	translate("Plain DNS server used to resolve DoT/DoH hostnames"))
o.default = "223.5.5.5"

o = s:taboption("failover", Flag, "insecure_skip_verify", translate("Disable TLS verification"),
	translate("Skip DoT/DoH certificate validation (useful if the system clock/CA store is broken)"))
o.default = "0"

o = s:taboption("failover", Value, "idle_timeout", translate("Connection idle timeout (s)"),
	translate("How long pooled TCP/DoT/DoH connections stay open"))
o.datatype = "and(uinteger,min(5))"
o.default = "30"

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

o = s:taboption("cache", Value, "stale_client_ttl", translate("Stale answer TTL (s)"),
	translate("TTL stamped on stale answers so clients can cache them briefly; RFC 8767 recommends 30"))
o.datatype = "and(uinteger,min(1),max(600))"
o.default = "30"
o:depends("serve_stale", "1")

o = s:taboption("cache", Flag, "prefetch", translate("Prefetch popular entries"),
	translate("Refresh frequently used entries shortly before they expire, so they never go stale"))
o.default = "1"
o:depends("cache", "1")

o = s:taboption("cache", Flag, "dump_cache", translate("Persist cache to disk"),
	translate("Save the cache on shutdown (and optionally at intervals); restore it on startup"))
o.default = "1"
o:depends("cache", "1")

o = s:taboption("cache", Value, "dump_interval", translate("Cache save interval (s)"),
	translate("0 = save only on shutdown (spares flash wear); otherwise dump every N seconds"))
o.datatype = "and(uinteger,min(0))"
o.default = "0"
o:depends("dump_cache", "1")

function m.on_after_commit(self)
	luci.sys.call("/etc/init.d/photondns reload >/dev/null 2>&1")
end

return m
