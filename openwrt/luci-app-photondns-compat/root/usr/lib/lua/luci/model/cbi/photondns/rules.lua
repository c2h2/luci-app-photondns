-- Rules (CBI) — legacy port of rules.js
-- Four file-backed text areas; photondns is restarted on save.
local fs = require "nixio.fs"

local RULE_FILES = {
	{ "hosts", "/etc/photondns/hosts.txt", translate("Hosts"),
		translate('Static host records: "<name> <ip> [ip...]" per line. Answers A/AAAA locally.') },
	{ "block", "/etc/photondns/block.txt", translate("Block List"),
		translate('Domains answered with NXDOMAIN. "example.com" blocks all subdomains, "full:example.com" exact only.') },
	{ "local_domains", "/etc/photondns/local_domains.txt", translate("Local Domains"),
		translate('Domains resolved by the "local" upstream group (configure Local-domain DNS servers first).') },
	{ "redirect", "/etc/photondns/redirect.txt", translate("Redirect"),
		translate('"<from-domain> <to-domain>" per line: answer queries for from-domain with the records of to-domain.') },
	{ "prewarm", "/etc/photondns/prewarm.txt", translate("Prewarm"),
		translate('Domains kept always-resolved (one per line) so a first visit is never a slow cold miss. Default set = YouTube/Google. Enable "Prewarm popular domains" in Basic Settings.') },
	{ "lan_hosts", "/etc/photondns/lan_hosts.txt", translate("LAN Hosts"),
		translate('Extra LAN hosts pinned by name → IP, one "name ip [ip...]" per line, for devices that do not advertise a usable DHCP name (e.g. a Mac). Each resolves bare and under the LAN suffix, and answers reverse PTR. DHCP leases are learned automatically; this file is only for pins. Enable "Resolve LAN hostnames" in Basic Settings.') },
}

local m, s, o

m = Map("photondns", translate("photondns Rules"),
	translate("Rule files are applied on service restart."))
m.apply_on_parse = true

s = m:section(NamedSection, "main", "photondns")
s.addremove = false
s.anonymous = true

for _, f in ipairs(RULE_FILES) do
	local key, path, title, descr = f[1], f[2], f[3], f[4]
	s:tab(key, title)
	o = s:taboption(key, TextValue, "_" .. key, nil, descr)
	o.rows = 20
	o.wrap = "off"
	o.rmempty = true
	o.cfgvalue = function(self, section)
		return fs.readfile(path) or ""
	end
	o.write = function(self, section, value)
		value = (value or ""):gsub("\r\n", "\n")
		value = value:gsub("^%s+", ""):gsub("%s+$", "") .. "\n"
		fs.writefile(path, value)
	end
	o.remove = function(self, section)
		fs.writefile(path, "\n")
	end
end

function m.on_after_commit(self)
	luci.sys.call("/etc/init.d/photondns restart >/dev/null 2>&1")
end

return m
