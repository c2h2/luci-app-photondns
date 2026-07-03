'use strict';
'require form';
'require poll';
'require rpc';
'require uci';
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
		s.tab('failover', _('Failover'));
		s.tab('cache', _('Cache'));

		/* ---- basic ---- */
		o = s.taboption('basic', form.Flag, 'enabled', _('Enabled'));
		o.default = o.disabled;
		o.rmempty = false;

		o = s.taboption('basic', form.Value, 'listen_address', _('Listen Address'));
		o.default = '0.0.0.0';

		o = s.taboption('basic', form.Value, 'listen_port', _('Listen Port'));
		o.datatype = 'port';
		o.default = '5335';

		o = s.taboption('basic', form.ListValue, 'log_level', _('Log Level'));
		o.value('debug', _('Debug'));
		o.value('info', _('Info'));
		o.value('warn', _('Warning'));
		o.value('error', _('Error'));
		o.default = 'info';

		o = s.taboption('basic', form.Value, 'log_file', _('Log File'));
		o.default = '/var/log/photondns.log';

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

		/* ---- upstream ---- */
		o = s.taboption('upstream', form.DynamicList, 'upstream', _('Primary DNS servers'),
			_('Formats: 1.2.3.4, udp://host, tcp://host, tls://host (DoT), https://host/dns-query (DoH)'));
		o.value('udp://223.5.5.5', _('AliDNS (UDP 223.5.5.5)'));
		o.value('udp://119.29.29.29', _('Tencent DNSPod (UDP 119.29.29.29)'));
		o.value('udp://114.114.114.114', _('114DNS (UDP)'));
		o.value('tls://223.5.5.5', _('AliDNS (DoT)'));
		o.value('tls://1.12.12.12', _('DNSPod (DoT)'));
		o.value('https://223.5.5.5/dns-query', _('AliDNS (DoH)'));
		o.value('tls://8.8.8.8', _('Google (DoT)'));
		o.value('tls://1.1.1.1', _('Cloudflare (DoT)'));
		o.value('https://dns.google/dns-query', _('Google (DoH)'));
		o.rmempty = false;

		o = s.taboption('upstream', form.DynamicList, 'backup_upstream', _('Backup DNS servers'),
			_('Only used when primary servers fail; also raced as a last hedge candidate'));
		o.value('tls://8.8.8.8', _('Google (DoT)'));
		o.value('tls://1.1.1.1', _('Cloudflare (DoT)'));
		o.value('udp://9.9.9.9', _('Quad9 (UDP)'));

		o = s.taboption('upstream', form.DynamicList, 'local_upstream', _('Local-domain DNS servers'),
			_('Optional group used for domains listed in the local_domains rule file (e.g. China DNS)'));
		o.value('udp://223.5.5.5', _('AliDNS (UDP 223.5.5.5)'));
		o.value('udp://119.29.29.29', _('Tencent DNSPod (UDP 119.29.29.29)'));

		o = s.taboption('upstream', form.Value, 'bootstrap_dns', _('Bootstrap DNS'),
			_('Plain DNS server used to resolve DoT/DoH hostnames'));
		o.default = '223.5.5.5';

		o = s.taboption('upstream', form.Flag, 'insecure_skip_verify', _('Disable TLS verification'),
			_('Skip DoT/DoH certificate validation (useful if the system clock/CA store is broken)'));
		o.default = false;

		o = s.taboption('upstream', form.Value, 'idle_timeout', _('Connection idle timeout (s)'),
			_('How long pooled TCP/DoT/DoH connections stay open'));
		o.datatype = 'and(uinteger,min(5))';
		o.default = '30';

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
		o.default = '2000';

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

		/* ---- cache ---- */
		o = s.taboption('cache', form.Flag, 'cache', _('Enable DNS cache'));
		o.default = true;
		o.rmempty = false;

		o = s.taboption('cache', form.Value, 'cache_size', _('Cache size (entries)'),
			_('Maximum number of cached responses (sharded LRU)'));
		o.datatype = 'and(uinteger,min(0))';
		o.default = '8192';
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

		o = s.taboption('cache', form.Flag, 'prefetch', _('Prefetch popular entries'),
			_('Refresh frequently used entries shortly before they expire, so they never go stale'));
		o.default = true;
		o.depends('cache', '1');

		o = s.taboption('cache', form.Flag, 'dump_cache', _('Persist cache to disk'),
			_('Save the cache on shutdown and periodically; restore it on startup'));
		o.default = true;
		o.depends('cache', '1');

		o = s.taboption('cache', form.Value, 'dump_interval', _('Cache save interval (s)'));
		o.datatype = 'and(uinteger,min(60))';
		o.default = '3600';
		o.depends('dump_cache', '1');

		return m.render();
	}
});
