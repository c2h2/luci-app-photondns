'use strict';
'require poll';
'require rpc';
'require ui';
'require view';

const callQueryLog = rpc.declare({
	object: 'luci.photondns',
	method: 'query_log',
	expect: { '': {} }
});

const QTYPES = {
	1: 'A', 2: 'NS', 5: 'CNAME', 6: 'SOA', 12: 'PTR', 15: 'MX', 16: 'TXT',
	28: 'AAAA', 33: 'SRV', 35: 'NAPTR', 43: 'DS', 46: 'RRSIG', 48: 'DNSKEY',
	64: 'SVCB', 65: 'HTTPS', 255: 'ANY', 257: 'CAA'
};

const ROUTE_COLORS = {
	cache: '#2ca02c',
	stale: '#17a2b8',
	hosts: '#7952b3',
	lan: '#7952b3',
	blocked: '#d43f3a',
	'aaaa-blocked': '#d43f3a',
	redirect: '#6c757d',
	servfail: '#8b0000',
	local: '#e67e22',
	main: '#0d6efd'
};

const PROTO_COLORS = {
	udp: '#607d8b',
	tcp: '#00838f',
	doh: '#5e35b1'
};

/* How many rows each "top ..." list in the stats panel shows. */
const TOP_N = 8;

function routeBadge(route) {
	const color = ROUTE_COLORS[route] || '#0d6efd';
	return E('span', {
		style: 'padding:0 7px; border-radius:8px; color:#fff; font-size:90%; background:' + color
	}, route);
}

function protoBadge(proto) {
	const p = (proto || '').toLowerCase();
	const color = PROTO_COLORS[p] || '#888';
	return E('span', {
		style: 'padding:0 5px; border-radius:6px; color:#fff; font-size:80%; background:' + color
	}, p ? p.toUpperCase() : '-');
}

// Compact styling so the in-memory log shows as many rows as possible.
const DENSE_CSS = '' +
	'#qlog_table table.cbi-section-table{border-collapse:collapse;width:100%;margin:0}' +
	'#qlog_table .th,#qlog_table .td{padding:1px 6px;font-size:12px;line-height:1.45;' +
		'vertical-align:top;border:0;white-space:nowrap}' +
	'#qlog_table td.qname{white-space:normal;word-break:break-all}' +
	'#qlog_table .table-titles .th{font-size:11px;text-transform:uppercase;' +
		'letter-spacing:.03em;opacity:.7}' +
	'#qlog_table .tr:nth-child(2n) .td{background:rgba(127,127,127,.07)}' +
	'#qlog_table .qcount{font-weight:600;color:#d43f3a;margin-left:3px}' +
	/* stats panel */
	'#qlog_stats{display:flex;flex-wrap:wrap;margin:0 -4px 6px}' +
	'#qlog_stats .qs-card{flex:1;min-width:220px;margin:4px;padding:8px 12px;' +
		'border:1px solid rgba(128,128,128,.35);border-radius:8px}' +
	'#qlog_stats h4{margin:0 0 6px;font-size:11px;text-transform:uppercase;' +
		'letter-spacing:.03em;opacity:.7}' +
	'#qlog_stats table{width:100%;border-collapse:collapse;margin:0}' +
	'#qlog_stats td{padding:1px 0;font-size:12px;line-height:1.45;border:0;vertical-align:middle}' +
	'#qlog_stats td.qs-k{opacity:.8}' +
	'#qlog_stats td.qs-v{text-align:right;font-weight:600;white-space:nowrap;padding-left:8px}' +
	'#qlog_stats td.qs-name{max-width:0;overflow:hidden;text-overflow:ellipsis;' +
		'white-space:nowrap;cursor:pointer}' +
	'#qlog_stats td.qs-name:hover{text-decoration:underline}' +
	'#qlog_stats td.qs-bar{width:56px;padding:0 6px}' +
	'#qlog_stats td.qs-bar div{height:7px;border-radius:4px;background:#0d6efd;opacity:.55}' +
	'#qlog_stats .qs-badges span{display:inline-block;margin:0 6px 4px 0}' +
	'#qlog_stats .qs-badges b{margin-left:3px}' +
	'#qlog_stats .qs-empty{opacity:.6;font-size:12px}';

// Collapse identical consecutive queries into one row + count, as long as
// each repeat is within GROUP_WINDOW seconds of the previous one.
const GROUP_WINDOW = 3;
function groupEntries(list) {
	const out = [], active = {};
	for (const e of list) {
		const key = (e.client || '') + '|' + (e.qname || '') + '|' + e.qtype + '|' +
			(e.proto || '') + '|' + (e.route || '') + '|' + (e.upstream || '');
		const gi = active[key];
		if (gi != null && (out[gi].anchor - e.ts) <= GROUP_WINDOW) {
			out[gi].count++;
			out[gi].anchor = e.ts;
		} else {
			out.push({ e: e, count: 1, anchor: e.ts });
			active[key] = out.length - 1;
		}
	}
	return out;
}

function fmtTime(ts) {
	const d = new Date(ts * 1000);
	return ('0' + d.getHours()).slice(-2) + ':' +
		('0' + d.getMinutes()).slice(-2) + ':' +
		('0' + d.getSeconds()).slice(-2);
}

function fmtSpan(sec) {
	if (sec < 60) return '%ds'.format(sec);
	if (sec < 3600) return '%dm %ds'.format(Math.floor(sec / 60), sec % 60);
	return '%dh %dm'.format(Math.floor(sec / 3600), Math.floor((sec % 3600) / 60));
}

function matchesFilter(e, f) {
	return !f || (e.qname || '').toLowerCase().includes(f) ||
		(e.client || '').toLowerCase().includes(f) ||
		(e.route || '').toLowerCase().includes(f) ||
		(e.proto || '').toLowerCase().includes(f) ||
		(e.upstream || '').toLowerCase().includes(f);
}

// Sorted [key, count] pairs, highest count first.
function topN(counts, n) {
	return Object.keys(counts)
		.map(k => [k, counts[k]])
		.sort((a, b) => b[1] - a[1])
		.slice(0, n);
}

// Aggregate the (filtered) window into simple counters for the stats panel.
function computeStats(entries) {
	const st = {
		total: entries.length, clients: {}, domains: {}, routes: {}, qtypes: {},
		upstreams: {}, newest: 0, oldest: Infinity, forwarded: 0, fwdRtt: 0,
		cached: 0, blocked: 0
	};
	for (const e of entries) {
		st.clients[e.client] = (st.clients[e.client] || 0) + 1;
		st.domains[e.qname] = (st.domains[e.qname] || 0) + 1;
		st.routes[e.route] = (st.routes[e.route] || 0) + 1;
		const t = QTYPES[e.qtype] || String(e.qtype);
		st.qtypes[t] = (st.qtypes[t] || 0) + 1;
		if (e.route === 'cache' || e.route === 'stale')
			st.cached++;
		else if (e.route === 'blocked' || e.route === 'aaaa-blocked')
			st.blocked++;
		// "redirect" reuses the upstream field for the target name, not a server
		if (e.upstream && e.route !== 'redirect') {
			st.forwarded++;
			st.fwdRtt += e.rtt_ms;
			const u = st.upstreams[e.upstream] || (st.upstreams[e.upstream] = { n: 0, rtt: 0 });
			u.n++;
			u.rtt += e.rtt_ms;
		}
		if (e.ts > st.newest) st.newest = e.ts;
		if (e.ts < st.oldest) st.oldest = e.ts;
	}
	return st;
}

return view.extend({
	filter: '',
	entries: null,
	filterInput: null,

	load() {
		return L.resolveDefault(callQueryLog(), {});
	},

	parse(res) {
		if (!res || !res.running || !res.raw) return null;
		try { return JSON.parse(res.raw).entries || []; } catch (e) { return null; }
	},

	setFilter(v) {
		this.filter = (v || '').trim();
		if (this.filterInput) this.filterInput.value = this.filter;
		this.redraw();
	},

	card(title, body) {
		return E('div', { class: 'qs-card' }, [ E('h4', {}, title), body ]);
	},

	// name / bar / count rows; clicking a name filters the log by it
	topTable(pairs, fmtCount) {
		if (!pairs.length)
			return E('div', { class: 'qs-empty' }, _('(no data)'));
		const max = pairs[0][1] || 1;
		return E('table', {}, pairs.map(p => E('tr', {}, [
			E('td', { class: 'qs-name', title: p[0], click: () => this.setFilter(p[0]) }, p[0]),
			E('td', { class: 'qs-bar' }, E('div', { style: 'width:' + Math.round(p[1] * 100 / max) + '%' })),
			E('td', { class: 'qs-v' }, fmtCount ? fmtCount(p) : String(p[1]))
		])));
	},

	renderStats(entries) {
		const st = computeStats(entries);
		const wrap = E('div', { id: 'qlog_stats' });
		if (!st.total) return wrap;

		const pct = n => Math.round(n * 100 / st.total) + '%';
		const span = st.newest - st.oldest;
		const rate = span > 0 ? (st.total * 60 / span).toFixed(1) : '-';
		const kv = rows => E('table', {}, rows.map(r => E('tr', {}, [
			E('td', { class: 'qs-k' }, r[0]),
			E('td', { class: 'qs-v' }, r[1])
		])));

		wrap.appendChild(this.card(_('Summary'), kv([
			[ _('Queries'), String(st.total) ],
			[ _('Time span'), fmtSpan(span) ],
			[ _('Queries / min'), rate ],
			[ _('Unique clients'), String(Object.keys(st.clients).length) ],
			[ _('Unique domains'), String(Object.keys(st.domains).length) ],
			[ _('Cache hit rate'), pct(st.cached) ],
			[ _('Blocked'), st.blocked + ' (' + pct(st.blocked) + ')' ],
			[ _('Forwarded'), st.forwarded + ' (' + pct(st.forwarded) + ')' ],
			[ _('Avg upstream RTT'), st.forwarded ? (st.fwdRtt / st.forwarded).toFixed(1) + ' ms' : '-' ]
		])));

		wrap.appendChild(this.card(_('Top clients'),
			this.topTable(topN(st.clients, TOP_N), p => p[1] + ' (' + pct(p[1]) + ')')));

		wrap.appendChild(this.card(_('Top domains'),
			this.topTable(topN(st.domains, TOP_N))));

		const routes = E('div', { class: 'qs-badges' }, topN(st.routes, 99).map(p =>
			E('span', { style: 'cursor:pointer', click: () => this.setFilter(p[0]) },
				[ routeBadge(p[0]), E('b', {}, p[1] + ' (' + pct(p[1]) + ')') ])));
		const types = E('div', { class: 'qs-badges', style: 'margin-top:4px' }, topN(st.qtypes, 99).map(p =>
			E('span', {}, [ E('span', { style: 'opacity:.8' }, p[0]), E('b', {}, String(p[1])) ])));
		wrap.appendChild(this.card(_('Routes'), E('div', {}, [
			routes,
			E('h4', { style: 'margin-top:6px' }, _('Query types')),
			types
		])));

		const ups = Object.keys(st.upstreams)
			.map(k => [k, st.upstreams[k].n, st.upstreams[k].rtt / st.upstreams[k].n])
			.sort((a, b) => b[1] - a[1])
			.slice(0, TOP_N);
		wrap.appendChild(this.card(_('Top upstreams'),
			this.topTable(ups, p => p[1] + ' · ' + p[2].toFixed(1) + ' ms')));

		return wrap;
	},

	renderTable(entries) {
		const tbl = E('table', { class: 'table cbi-section-table' }, [
			E('tr', { class: 'tr table-titles' }, [
				E('th', { class: 'th', style: 'width:64px' }, _('Time')),
				E('th', { class: 'th', style: 'width:110px' }, _('Client')),
				E('th', { class: 'th', style: 'width:48px' }, _('Proto')),
				E('th', { class: 'th' }, _('Domain')),
				E('th', { class: 'th', style: 'width:56px' }, _('Type')),
				E('th', { class: 'th', style: 'width:74px' }, _('Route')),
				E('th', { class: 'th' }, _('Upstream')),
				E('th', { class: 'th', style: 'width:66px; text-align:right' }, _('ms'))
			])
		]);
		const groups = groupEntries(entries);
		let shown = 0;
		for (const grp of groups) {
			if (++shown > 500) break;
			const e = grp.e;
			const qcell = grp.count > 1
				? [e.qname, E('span', { class: 'qcount' }, '(' + grp.count + ')')]
				: e.qname;
			tbl.appendChild(E('tr', { class: 'tr' }, [
				E('td', { class: 'td' }, fmtTime(e.ts)),
				E('td', { class: 'td' }, e.client),
				E('td', { class: 'td' }, protoBadge(e.proto)),
				E('td', { class: 'td qname' }, qcell),
				E('td', { class: 'td' }, QTYPES[e.qtype] || String(e.qtype)),
				E('td', { class: 'td' }, routeBadge(e.route)),
				E('td', { class: 'td' }, e.upstream || '-'),
				E('td', { class: 'td', style: 'text-align:right' },
					e.rtt_ms < 0.1 ? '<0.1' : String(e.rtt_ms))
			]));
		}
		if (shown === 0)
			tbl.appendChild(E('tr', { class: 'tr' }, [
				E('td', { class: 'td', colspan: 8, style: 'opacity:.6' }, _('(no entries)'))
			]));
		return tbl;
	},

	// Re-render stats + table from the last fetched entries (filter applied).
	redraw() {
		const statsBox = document.getElementById('qlog_stats_box');
		const tableBox = document.getElementById('qlog_table');
		if (!statsBox || !tableBox || !this.entries) return;
		const f = this.filter.toLowerCase();
		const filtered = f ? this.entries.filter(e => matchesFilter(e, f)) : this.entries;
		statsBox.innerHTML = '';
		statsBox.appendChild(this.renderStats(filtered));
		tableBox.innerHTML = '';
		tableBox.appendChild(this.renderTable(filtered));
	},

	refresh() {
		return L.resolveDefault(callQueryLog(), {}).then(res => {
			const entries = this.parse(res);
			if (entries) {
				this.entries = entries;
				this.redraw();
			}
		});
	},

	render(data) {
		this.entries = this.parse(data);
		const statsBox = E('div', { id: 'qlog_stats_box' });
		const tableBox = E('div', { id: 'qlog_table' });
		if (!this.entries)
			tableBox.appendChild(E('div', { class: 'alert-message warning' },
				_('photondns is not running (or the query log is disabled).')));

		this.filterInput = E('input', {
			type: 'text',
			class: 'cbi-input-text',
			style: 'max-width:300px',
			placeholder: _('Filter by domain / client / route...'),
			keyup: (ev) => { this.filter = ev.target.value.trim(); this.redraw(); }
		});

		poll.add(() => this.refresh(), 3);

		const root = E('div', { class: 'cbi-map' }, [
			E('style', { type: 'text/css' }, DENSE_CSS),
			E('h2', {}, _('Query Log')),
			E('div', { class: 'cbi-map-descr' },
				_('Live view of recent DNS queries: which client asked, how each query was answered (cache, hosts, blocked, or the upstream group and server that won the race) and how long it took. Kept in memory only.') + ' ' +
				_('The statistics cover the queries currently shown (the filter applies to them too). Click a client, domain or route to filter by it.')),
			E('div', { style: 'margin:8px 0' }, [ this.filterInput ]),
			statsBox,
			tableBox
		]);

		// initial draw happens after the boxes exist in the tree
		if (this.entries) {
			statsBox.appendChild(this.renderStats(this.entries));
			tableBox.appendChild(this.renderTable(this.entries));
		}
		return root;
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
