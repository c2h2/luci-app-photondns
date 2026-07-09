'use strict';
'require form';
'require poll';
'require rpc';
'require uci';
'require ui';
'require view';

const callServiceList = rpc.declare({
	object: 'service',
	method: 'list',
	params: ['name'],
	expect: { '': {} }
});

function getServiceStatus() {
	return L.resolveDefault(callServiceList('photondns'), {}).then(res => {
		let isRunning = false;
		try {
			isRunning = res['photondns']['instances']['photondns']['running'];
		} catch (e) { }
		return isRunning;
	});
}

function renderStatus(isRunning) {
	const spanTemp = '<em><span style="color:%s"><strong>%s %s</strong></span></em>';
	return isRunning
		? spanTemp.format('green', _('photondns'), _('RUNNING'))
		: spanTemp.format('red', _('photondns'), _('NOT RUNNING'));
}

const callChinaListStatus = rpc.declare({
	object: 'luci.photondns',
	method: 'chinalist_status',
	expect: { '': {} }
});

const callAdListStatus = rpc.declare({
	object: 'luci.photondns',
	method: 'adlist_status',
	expect: { '': {} }
});

const callUpdateChinaList = rpc.declare({
	object: 'luci.photondns',
	method: 'update_chinalist',
	expect: { '': {} }
});

const callUpdateAdList = rpc.declare({
	object: 'luci.photondns',
	method: 'update_adlist',
	expect: { '': {} }
});

function listStatusText(st) {
	if (st && st.updating)
		return E('em', { style: 'color:#c7a500' }, _('Updating...'));
	if (st && st.exists)
		return E('strong', {}, _('%d domains, updated %s').format(st.domains,
			new Date(st.mtime * 1000).toLocaleString()));
	return E('em', { style: 'color:#d43f3a' }, _('not downloaded yet'));
}

function renderListStatus(title, callStatus, callUpdate, st) {
	const statusEl = E('span', {}, [ listStatusText(st) ]);

	const setStatus = s => {
		statusEl.innerHTML = '';
		statusEl.appendChild(listStatusText(s));
	};

	const waitDone = () => new Promise(resolve => {
		const tick = () => L.resolveDefault(callStatus(), {}).then(s => {
			if (s && s.updating) return;
			poll.remove(tick);
			setStatus(s);
			resolve(s);
		});
		poll.add(tick, 2);
	});

	const btn = E('button', {
		class: 'btn cbi-button-apply',
		style: 'margin-left:12px',
		click: ui.createHandlerFn(null, () => callUpdate().then(res => {
			if (!res || !res.success) {
				ui.addNotification(null, E('p', _('Update failed to start: %s')
					.format((res && res.error) || '?')), 'error');
				return;
			}
			setStatus({ updating: true });
			return waitDone().then(s => {
				if (s && s.exists) {
					btn.textContent = _('Update Now');
					ui.addNotification(null, E('p',
						_('%s: %d domains.').format(title, s.domains || 0)), 'info');
				} else {
					ui.addNotification(null, E('p',
						_('Download failed - check the log on the List Updates page.')), 'error');
				}
			});
		}))
	}, (st && st.exists) ? _('Update Now') : _('Download Now'));

	if (st && st.updating) {
		btn.disabled = true;
		waitDone().then(() => { btn.disabled = false; });
	}

	return E('div', {}, [ statusEl, btn ]);
}

return view.extend({
	load() {
		return Promise.all([uci.load('photondns')]);
	},

	render() {
		let m, s, o;

		m = new form.Map('photondns', _('photondns'),
			_('High-performance DNS forwarder written in Rust: configurable cache, serve-stale, prefetch, ' +
			  'and adaptive failover that races multiple upstreams (UDP/TCP/DoT/DoH) and takes the fastest answer.'));

		s = m.section(form.TypedSection);
		s.anonymous = true;
		s.render = () => {
			poll.add(() => {
				return L.resolveDefault(getServiceStatus()).then(res => {
					const view = document.getElementById('service_status');
					if (view) view.innerHTML = renderStatus(res);
				});
			});
			return E('div', { class: 'cbi-section', id: 'status_bar' }, [
				E('p', { id: 'service_status' }, _('Collecting data...'))
			]);
		};

		s = m.section(form.NamedSection, 'main', 'photondns');

		s.tab('basic', _('Basic Options'));
		s.tab('upstream', _('Upstreams'));
		s.tab('failover', _('Failover & Advanced'));
		s.tab('cache', _('Cache'));
		s.tab('adblock', _('Ad Block'));

		/* ---- basic ---- */
		o = s.taboption('basic', form.Flag, 'enabled', _('Enabled'));
		o.default = o.disabled;
		o.rmempty = false;

		o = s.taboption('basic', form.Value, 'listen_address', _('Listen Address'));
		o.default = '0.0.0.0';

		o = s.taboption('basic', form.Value, 'listen_port', _('Listen Port'));
		o.datatype = 'port';
		o.default = '15533';

		o = s.taboption('basic', form.Flag, 'udp', _('Listen on UDP'),
			_('Serve classic DNS over UDP on the listen port'));
		o.default = '1';
		o.rmempty = false;

		o = s.taboption('basic', form.Flag, 'tcp', _('Listen on TCP'),
			_('Serve DNS over TCP on the listen port (needed for large answers)'));
		o.default = '1';
		o.rmempty = false;

		o = s.taboption('basic', form.Flag, 'doh', _('DoH Server'),
			_('Serve DNS-over-HTTPS (RFC 8484) on the DoH port. Runs plain HTTP unless a certificate ' +
			  'is set below - put a TLS reverse proxy (Caddy/nginx) in front, or set cert + key to serve HTTPS directly'));
		o.default = false;
		o.rmempty = false;

		o = s.taboption('basic', form.Value, 'doh_port', _('DoH Port'));
		o.datatype = 'port';
		o.default = '8054';
		o.depends('doh', '1');

		o = s.taboption('basic', form.Value, 'doh_path', _('DoH Path'));
		o.default = '/dns-query';
		o.depends('doh', '1');

		o = s.taboption('basic', form.Value, 'doh_cert', _('DoH TLS Certificate'),
			_('Path to a PEM certificate chain; leave empty when behind a reverse proxy'));
		o.depends('doh', '1');

		o = s.taboption('basic', form.Value, 'doh_key', _('DoH TLS Key'),
			_('Path to the PEM private key'));
		o.depends('doh', '1');

		o = s.taboption('basic', form.ListValue, 'log_level', _('Log Level'),
			_('Verbosity of the daemon log.'));
		o.value('debug', _('Debug'));
		o.value('info', _('Info'));
		o.value('warn', _('Warning'));
		o.value('error', _('Error'));
		o.default = 'info';

		o = s.taboption('basic', form.Value, 'log_file', _('Log File'),
			_('Daemon log path. Leave empty to log to the system log (logread) only - no file. Note: /var on OpenWrt is RAM (tmpfs), so a file here lives in memory and is cleared on reboot.'));
		o.default = '/var/log/photondns.log';
		o.rmempty = false;

		o = s.taboption('basic', form.Value, 'api_port', _('API Port'),
			_('Local HTTP API for statistics and cache control (127.0.0.1 only)'));
		o.datatype = 'port';
		o.default = '8053';

		o = s.taboption('basic', form.Flag, 'redirect', _('DNS Forward'),
			_('Forward dnsmasq DNS resolution requests to photondns'));
		o.default = false;
		o.rmempty = false;

		o = s.taboption('basic', form.Flag, 'dns_hijack', _('DNS Redirect (Hijack)'),
			_('Force redirect all LAN DNS queries (UDP port 53) to photondns via firewall'));
		o.default = false;
		o.depends('redirect', '1');

		o = s.taboption('basic', form.Flag, 'reject_type65', _('Disable RR Type 65 (HTTPS/SVCB)'),
			_('Answer HTTPS/SVCB queries with NXDOMAIN, forcing clients onto plain A/AAAA records'));
		o.default = false;

		o = s.taboption('basic', form.ListValue, 'aaaa_mode', _('IPv6 (AAAA) handling'),
			_('Whether to answer IPv6 (AAAA) lookups. "Block if IPv4 exists" suppresses IPv6 only for names that also have an IPv4 address, so IPv6-only sites still work; "Block all" forces every client onto IPv4.'));
		o.value('allow', _('Allow (default)'));
		o.value('block_if_ipv4', _('Block if IPv4 exists'));
		o.value('block_all', _('Block all IPv6'));
		o.default = 'allow';

		o = s.taboption('basic', form.Flag, 'prewarm', _('Prewarm popular domains'),
			_('Keep a list of domains (default: YouTube/Google) always resolved so a first visit is never a slow cold miss. Edit the list under Rules.'));
		o.default = true;

		o = s.taboption('basic', form.Value, 'prewarm_interval', _('Prewarm interval (s)'),
			_('How often to refresh the prewarm domains; keep it below the cache stale lifetime so they never fully expire. 0 = only at startup.'));
		o.datatype = 'and(uinteger,min(0),max(86400))';
		o.default = '3000';
		o.depends('prewarm', '1');

		o = s.taboption('basic', form.Value, 'query_log_size', _('Query log size'),
			_('Number of recent queries kept in memory for the Query Log page; 0 disables it'));
		o.datatype = 'and(uinteger,max(65536))';
		o.default = '5000';

		o = s.taboption('basic', form.Flag, 'auto_update', _('Auto-update lists'),
			_('Refresh the enabled China / ad-block lists on a schedule (cron)'));
		o.default = false;

		o = s.taboption('basic', form.ListValue, 'update_day', _('Update day'));
		o.value('*', _('Every day'));
		o.value('0', _('Sunday'));
		o.value('1', _('Monday'));
		o.value('2', _('Tuesday'));
		o.value('3', _('Wednesday'));
		o.value('4', _('Thursday'));
		o.value('5', _('Friday'));
		o.value('6', _('Saturday'));
		o.default = '*';
		o.depends('auto_update', '1');

		o = s.taboption('basic', form.Value, 'update_time', _('Update hour (0-23)'));
		o.datatype = 'and(uinteger,max(23))';
		o.default = '4';
		o.depends('auto_update', '1');

		/* ---- upstream ---- */
		o = s.taboption('upstream', form.ListValue, 'china_list', _('Routing mode'),
			_('Split mode resolves mainland-China domains (dnsmasq-china-list, ~70k entries) via the China DNS group and everything else via the overseas group'));
		o.value('1', _('China / overseas split DNS (recommended)'));
		o.value('0', _('Single group - all domains use the same servers'));
		o.default = '1';
		o.rmempty = false;

		o = s.taboption('upstream', form.DynamicList, 'local_upstream', _('China DNS servers'),
			_('Used for mainland-China domains and the Local Domains rule file'));
		o.value('udp://223.5.5.5', _('AliDNS (UDP 223.5.5.5)'));
		o.value('udp://119.29.29.29', _('Tencent DNSPod (UDP 119.29.29.29)'));
		o.value('udp://114.114.114.114', _('114DNS (UDP)'));
		o.value('tls://223.5.5.5', _('AliDNS (DoT)'));
		o.value('tls://1.12.12.12', _('DNSPod (DoT)'));
		o.default = 'udp://223.5.5.5';
		o.depends('china_list', '1');

		o = s.taboption('upstream', form.DummyValue, '_chinalist_status', _('China list status'));
		o.depends('china_list', '1');
		o.cfgvalue = () => L.resolveDefault(callChinaListStatus(), {}).then(st =>
			renderListStatus(_('China Domain List'), callChinaListStatus, callUpdateChinaList, st));

		o = s.taboption('upstream', form.DynamicList, 'upstream', _('Overseas DNS servers'),
			_('Used for all other domains (all domains in single-group mode). Formats: 1.2.3.4, udp://host, tcp://host, tls://host (DoT), https://host/dns-query (DoH)'));
		o.value('tcp://1.1.1.1', _('Cloudflare (TCP)'));
		o.value('tls://1.1.1.1', _('Cloudflare (DoT)'));
		o.value('tcp://8.8.8.8', _('Google (TCP)'));
		o.value('tls://8.8.8.8', _('Google (DoT)'));
		o.value('https://dns.google/dns-query', _('Google (DoH)'));
		o.value('udp://9.9.9.9', _('Quad9 (UDP)'));
		o.default = 'tcp://1.1.1.1';
		o.rmempty = false;

		/* ---- adblock ---- */
		o = s.taboption('adblock', form.Flag, 'adblock', _('Enable DNS ad blocking'),
			_('Answer known advertising / tracker domains with NXDOMAIN'));
		o.default = false;
		o.rmempty = false;

		o = s.taboption('adblock', form.DynamicList, 'ad_source', _('Ad list sources'),
			_('Plain domain lists, mosdns domain:/full: lists or hosts-format files, one URL per entry'));
		o.value('https://cdn.jsdelivr.net/gh/privacy-protection-tools/anti-AD@master/anti-ad-domains.txt', 'anti-AD (jsdelivr)');
		o.value('https://raw.githubusercontent.com/privacy-protection-tools/anti-AD/master/anti-ad-domains.txt', 'anti-AD (github)');
		o.value('https://cdn.jsdelivr.net/gh/Cats-Team/AdRules@main/mosdns_adrules.txt', 'Cats-Team AdRules (jsdelivr)');
		o.value('https://raw.githubusercontent.com/Cats-Team/AdRules/main/mosdns_adrules.txt', 'Cats-Team AdRules (github)');
		o.value('https://raw.githubusercontent.com/neodevpro/neodevhost/master/domain', 'NEO DEV HOST');
		o.depends('adblock', '1');

		o = s.taboption('adblock', form.DummyValue, '_adlist_status', _('Ad list status'));
		o.depends('adblock', '1');
		o.cfgvalue = () => L.resolveDefault(callAdListStatus(), {}).then(st =>
			renderListStatus(_('Ad Block Lists'), callAdListStatus, callUpdateAdList, st));

		/* ---- failover ---- */
		o = s.taboption('failover', form.ListValue, 'strategy', _('Strategy'),
			_('race: hedged racing, best latency (recommended). fastest: always lowest-latency upstream. ' +
			  'parallel: ask all at once. sequential: strict configured order. random: uniform spread.'));
		o.value('race', _('race (adaptive hedging)'));
		o.value('fastest', _('fastest (lowest EWMA)'));
		o.value('parallel', _('parallel (all at once)'));
		o.value('sequential', _('sequential (ordered)'));
		o.value('random', _('random'));
		o.default = 'race';

		o = s.taboption('failover', form.Value, 'hedge_delay', _('Max hedge delay (ms)'),
			_('Upper bound before racing the next upstream; actual delay adapts to ~2x the best upstream latency'));
		o.datatype = 'and(uinteger,min(10))';
		o.default = '250';

		o = s.taboption('failover', form.Value, 'query_timeout', _('Query timeout (ms)'));
		o.datatype = 'and(uinteger,min(100))';
		o.default = '5000';

		o = s.taboption('failover', form.Value, 'health_check_interval', _('Health check interval (s)'),
			_('Active probes keep latency stats fresh and detect dead upstreams even when idle'));
		o.datatype = 'and(uinteger,min(2))';
		o.default = '10';

		o = s.taboption('failover', form.Value, 'health_check_domain', _('Health check domain'));
		o.default = 'www.gstatic.com';

		o = s.taboption('failover', form.Value, 'fail_threshold', _('Failure threshold'),
			_('Consecutive failures before an upstream is taken out of rotation'));
		o.datatype = 'and(uinteger,min(1))';
		o.default = '3';

		o = s.taboption('failover', form.Value, 'recover_threshold', _('Recovery threshold'),
			_('Consecutive successes before a down upstream is restored'));
		o.datatype = 'and(uinteger,min(1))';
		o.default = '2';

		o = s.taboption('failover', form.Value, 'cooldown', _('Cooldown (s)'),
			_('How long a down upstream is excluded before a half-open retry'));
		o.datatype = 'and(uinteger,min(1))';
		o.default = '15';

		o = s.taboption('failover', form.DynamicList, 'backup_upstream', _('Backup DNS servers'),
			_('Only used when primary servers fail; also raced as a last hedge candidate'));
		o.value('tls://223.5.5.5', _('AliDNS (DoT)'));
		o.value('tls://8.8.8.8', _('Google (DoT)'));
		o.value('tls://1.1.1.1', _('Cloudflare (DoT)'));
		o.value('udp://9.9.9.9', _('Quad9 (UDP)'));

		o = s.taboption('failover', form.Value, 'bootstrap_dns', _('Bootstrap DNS'),
			_('Plain DNS server used to resolve DoT/DoH hostnames'));
		o.default = '223.5.5.5';

		o = s.taboption('failover', form.Flag, 'insecure_skip_verify', _('Disable TLS verification'),
			_('Skip DoT/DoH certificate validation (useful if the system clock/CA store is broken)'));
		o.default = false;

		o = s.taboption('failover', form.Value, 'idle_timeout', _('Connection idle timeout (s)'),
			_('How long pooled TCP/DoT/DoH connections stay open'));
		o.datatype = 'and(uinteger,min(5))';
		o.default = '30';

		/* ---- cache ---- */
		o = s.taboption('cache', form.Flag, 'cache', _('Enable DNS cache'));
		o.default = true;
		o.rmempty = false;

		o = s.taboption('cache', form.ListValue, 'cache_size', _('Cache size (entries)'),
			_('Maximum number of cached responses (sharded LRU). RAM shown is the ceiling when the cache is fully populated (~400 B/entry); actual use grows lazily.'));
		o.value('8192', _('8192 (~3 MB)'));
		o.value('16384', _('16384 (~6 MB)'));
		o.value('32768', _('32768 (~13 MB)'));
		o.value('65536', _('65536 (~25 MB)'));
		o.value('131072', _('131072 (~50 MB)'));
		o.value('262144', _('262144 (~100 MB)'));
		o.default = '65536';
		o.depends('cache', '1');

		o = s.taboption('cache', form.Value, 'min_ttl', _('Minimum TTL (s)'),
			_('Raise very low TTLs to this value; 0 = no change'));
		o.datatype = 'and(uinteger,min(0),max(604800))';
		o.default = '0';
		o.depends('cache', '1');

		o = s.taboption('cache', form.Value, 'max_ttl', _('Maximum TTL (s)'),
			_('Cap TTLs at this value; 0 = no cap'));
		o.datatype = 'and(uinteger,min(0),max(604800))';
		o.default = '86400';
		o.depends('cache', '1');

		o = s.taboption('cache', form.Value, 'negative_ttl', _('Negative cache TTL (s)'),
			_('How long NXDOMAIN / empty answers are cached'));
		o.datatype = 'and(uinteger,min(0))';
		o.default = '30';
		o.depends('cache', '1');

		o = s.taboption('cache', form.Flag, 'serve_stale', _('Serve stale (lazy cache)'),
			_('Answer instantly from expired entries and refresh in the background - also keeps DNS working when all upstreams are down'));
		o.default = true;
		o.depends('cache', '1');

		o = s.taboption('cache', form.Value, 'stale_ttl', _('Stale lifetime (s)'),
			_('How long past expiry an entry may still be served'));
		o.datatype = 'and(uinteger,min(0))';
		o.default = '86400';
		o.depends('serve_stale', '1');

		o = s.taboption('cache', form.Value, 'stale_client_ttl', _('Stale answer TTL (s)'),
			_('TTL stamped on stale answers so clients can cache them briefly; RFC 8767 recommends 30'));
		o.datatype = 'and(uinteger,min(1),max(600))';
		o.default = '30';
		o.depends('serve_stale', '1');

		o = s.taboption('cache', form.Flag, 'prefetch', _('Prefetch popular entries'),
			_('Refresh frequently used entries shortly before they expire, so they never go stale'));
		o.default = true;
		o.depends('cache', '1');

		o = s.taboption('cache', form.Flag, 'dump_cache', _('Persist cache to disk'),
			_('Save the cache on shutdown (and optionally at intervals); restore it on startup'));
		o.default = true;
		o.depends('cache', '1');

		o = s.taboption('cache', form.Value, 'dump_interval', _('Cache save interval (s)'),
			_('0 = save only on shutdown (spares flash wear); otherwise dump every N seconds'));
		o.datatype = 'and(uinteger,min(0))';
		o.default = '0';
		o.depends('dump_cache', '1');

		return m.render();
	}
});
