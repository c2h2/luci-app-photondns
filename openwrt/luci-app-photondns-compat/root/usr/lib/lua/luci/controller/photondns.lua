-- luci-app-photondns-compat: legacy Lua/CBI controller for photondns
-- Provides the same menu + live-data endpoints as the modern JS/ucode app,
-- for old LuCI builds that lack the JS client and the ucode rpcd runtime.
module("luci.controller.photondns", package.seeall)

-- NOTE: do not cache module requires as file-scope upvalues here. In the LuCI
-- dispatcher's createtree phase (module(..., package.seeall)) those upvalues can
-- resolve to nil; the working controllers on old LuCI (mwan3, adbyby) all use
-- the nixio.fs / luci.* globals inside functions instead. We follow that pattern.

local API_FALLBACK_PORT = "8053"
local LOGFILE_FALLBACK  = "/var/log/photondns.log"

function index()
	if not nixio.fs.access("/etc/config/photondns") then
		return
	end

	local page = entry({"admin", "services", "photondns"}, firstchild(), _("photondns"), 30)
	page.dependent = false
	page.acl_depends = { "luci-app-photondns-compat" }

	entry({"admin", "services", "photondns", "status"},
		template("photondns/status"), _("Status"), 5)
	entry({"admin", "services", "photondns", "querylog"},
		template("photondns/querylog"), _("Query Log"), 7)
	entry({"admin", "services", "photondns", "basic"},
		cbi("photondns/basic"), _("Basic Settings"), 10)
	entry({"admin", "services", "photondns", "rules"},
		cbi("photondns/rules"), _("Rules"), 15)
	entry({"admin", "services", "photondns", "chinalist"},
		template("photondns/chinalist"), _("List Updates"), 17)
	entry({"admin", "services", "photondns", "logs"},
		template("photondns/logs"), _("Logs"), 20)

	-- JSON data endpoints (called by the templates via XHR)
	entry({"admin", "services", "photondns", "api", "stats"},    call("action_stats")).leaf = true
	entry({"admin", "services", "photondns", "api", "flush"},    call("action_flush")).leaf = true
	entry({"admin", "services", "photondns", "api", "running"},  call("action_running")).leaf = true
	entry({"admin", "services", "photondns", "api", "querylog"}, call("action_querylog")).leaf = true
	entry({"admin", "services", "photondns", "api", "log"},      call("action_log")).leaf = true
	entry({"admin", "services", "photondns", "api", "cleanlog"}, call("action_cleanlog")).leaf = true
	entry({"admin", "services", "photondns", "api", "listupdate"}, call("action_listupdate")).leaf = true
	entry({"admin", "services", "photondns", "api", "liststatus"}, call("action_liststatus")).leaf = true
	entry({"admin", "services", "photondns", "api", "servicestate"}, call("action_servicestate")).leaf = true
	entry({"admin", "services", "photondns", "api", "servicetoggle"}, call("action_servicetoggle")).leaf = true
end

local function api_port()
	local c = luci.model.uci.cursor()
	c:load("photondns")
	return c:get("photondns", "main", "api_port") or API_FALLBACK_PORT
end

local function logfile_path()
	local c = luci.model.uci.cursor()
	c:load("photondns")
	return c:get("photondns", "main", "log_file") or LOGFILE_FALLBACK
end

-- fetch a path from the local photondns HTTP API; returns body string or nil
local function api_get(path)
	local url = string.format("http://127.0.0.1:%s%s", api_port(), path)
	local cmd = string.format(
		"curl -s --max-time 3 %q 2>/dev/null || uclient-fetch -q -O - -T 3 %q 2>/dev/null",
		url, url)
	local body = luci.sys.exec(cmd)
	if body and #body > 0 then
		return body
	end
	return nil
end

local function write_json(tbl)
	luci.http.prepare_content("application/json")
	luci.http.write_json(tbl)
end

-- raw passthrough of an API JSON body (so the client parses upstream JSON directly)
function action_stats()
	local raw = api_get("/stats")
	if raw then
		write_json({ running = true, raw = raw })
	else
		write_json({ running = false, raw = "" })
	end
end

function action_flush()
	local raw = api_get("/flush")
	if raw then
		write_json({ success = true, raw = raw })
	else
		write_json({ success = false, error = "photondns API unreachable" })
	end
end

function action_running()
	local out = luci.sys.exec("ubus call service list '{\"name\":\"photondns\"}' 2>/dev/null")
	local running = out and out:find('"running": true', 1, true) ~= nil
	write_json({ running = running })
end

function action_querylog()
	local raw = api_get("/log?n=500")
	if raw then
		write_json({ running = true, raw = raw })
	else
		write_json({ running = false, raw = "" })
	end
end

function action_log()
	local path = logfile_path()
	local content = ""
	if path and nixio.fs.access(path) then
		content = nixio.fs.readfile(path) or ""
		-- keep the UI snappy: last ~200KB only
		if #content > 204800 then
			content = content:sub(#content - 204800)
		end
	end
	write_json({ log = content })
end

function action_cleanlog()
	local path = logfile_path()
	if path then
		nixio.fs.writefile(path, "")
		write_json({ success = true })
	else
		write_json({ success = false, error = "log path unknown" })
	end
end

-- which: "chinalist" or "adlist"
local LISTS = {
	chinalist = {
		script = "/usr/bin/photondns-chinalist",
		log    = "/var/log/photondns-chinalist.log",
		lock   = "/var/lock/photondns-chinalist.lock",
		file   = "/etc/photondns/china_list.txt",
	},
	adlist = {
		script = "/usr/bin/photondns-adlist",
		log    = "/var/log/photondns-adlist.log",
		lock   = "/var/lock/photondns-adlist.lock",
		file   = "/etc/photondns/ad_list.txt",
	},
}

function action_listupdate()
	local which = luci.http.formvalue("list")
	local l = LISTS[which]
	if not l then
		write_json({ success = false, error = "unknown list" })
		return
	end
	if nixio.fs.access(l.lock) then
		write_json({ success = false, error = "Another update is already in progress." })
		return
	end
	os.execute(string.format("%s > %s 2>&1 &", l.script, l.log))
	write_json({ success = true })
end

function action_liststatus()
	local which = luci.http.formvalue("list")
	local l = LISTS[which]
	if not l then
		write_json({ error = "unknown list" })
		return
	end
	local st = nixio.fs.stat(l.file)
	local count = 0
	if st then
		local out = luci.sys.exec(string.format("wc -l < %q 2>/dev/null", l.file))
		count = tonumber((out or ""):match("%d+")) or 0
	end
	local logtxt = ""
	if nixio.fs.access(l.log) then
		logtxt = nixio.fs.readfile(l.log) or ""
	end
	write_json({
		exists   = st ~= nil,
		domains  = count,
		mtime    = st and st.mtime or 0,
		updating = nixio.fs.access(l.lock) ~= nil,
		log      = logtxt,
	})
end

-- persistent on/off switch: read uci enabled + whether the API answers
function action_servicestate()
	local c = luci.model.uci.cursor()
	c:load("photondns")
	local enabled = (c:get("photondns", "main", "enabled") == "1")
	local running = api_get("/health") ~= nil
	write_json({ enabled = enabled, running = running })
end

-- set uci enabled, commit, then start/stop so it takes effect immediately
function action_servicetoggle()
	local enable = luci.http.formvalue("enable") == "1"
	local c = luci.model.uci.cursor()
	c:load("photondns")
	c:set("photondns", "main", "enabled", enable and "1" or "0")
	c:commit("photondns")
	if enable then
		os.execute("/etc/init.d/photondns enable; /etc/init.d/photondns restart >/dev/null 2>&1")
	else
		os.execute("/etc/init.d/photondns stop >/dev/null 2>&1; /etc/init.d/photondns disable")
	end
	write_json({ success = true, enabled = enable })
end
