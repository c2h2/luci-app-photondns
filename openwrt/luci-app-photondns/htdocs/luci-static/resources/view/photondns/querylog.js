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
	blocked: '#d43f3a',
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
	'#qlog_table .qcount{font-weight:600;color:#d43f3a;margin-left:3px}';

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

return view.extend({
	filter: '',

	load() {
		return L.resolveDefault(callQueryLog(), {});
	},

	parse(res) {
		if (!res || !res.running || !res.raw) return null;
		try { return JSON.parse(res.raw).entries || []; } catch (e) { return null; }
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
		const f = this.filter.toLowerCase();
		const filtered = entries.filter(e => !f || e.qname.includes(f) ||
			e.client.includes(f) || e.route.includes(f) ||
			(e.proto || '').includes(f) || (e.upstream || '').includes(f));
		const groups = groupEntries(filtered);
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

	refresh() {
		return L.resolveDefault(callQueryLog(), {}).then(res => {
			const entries = this.parse(res);
			const box = document.getElementById('qlog_table');
			if (box && entries) {
				box.innerHTML = '';
				box.appendChild(this.renderTable(entries));
			}
		});
	},

	render(data) {
		const entries = this.parse(data);
		const box = E('div', { id: 'qlog_table' });
		if (entries)
			box.appendChild(this.renderTable(entries));
		else
			box.appendChild(E('div', { class: 'alert-message warning' },
				_('photondns is not running (or the query log is disabled).')));

		poll.add(() => this.refresh(), 3);

		return E('div', { class: 'cbi-map' }, [
			E('style', { type: 'text/css' }, DENSE_CSS),
			E('h2', {}, _('Query Log')),
			E('div', { class: 'cbi-map-descr' },
				_('Live view of recent DNS queries: which client asked, how each query was answered (cache, hosts, blocked, or the upstream group and server that won the race) and how long it took. Kept in memory only.')),
			E('div', { style: 'margin:8px 0' }, [
				E('input', {
					type: 'text',
					class: 'cbi-input-text',
					style: 'max-width:300px',
					placeholder: _('Filter by domain / client / route...'),
					keyup: (ev) => { this.filter = ev.target.value.trim(); this.refresh(); }
				})
			]),
			box
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
