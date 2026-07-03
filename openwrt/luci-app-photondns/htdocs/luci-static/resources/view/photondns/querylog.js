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

function routeBadge(route) {
	const color = ROUTE_COLORS[route] || '#0d6efd';
	return E('span', {
		style: 'padding:0 7px; border-radius:8px; color:#fff; font-size:90%; background:' + color
	}, route);
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
				E('th', { class: 'th', style: 'width:70px' }, _('Time')),
				E('th', { class: 'th', style: 'width:120px' }, _('Client')),
				E('th', { class: 'th' }, _('Domain')),
				E('th', { class: 'th', style: 'width:70px' }, _('Type')),
				E('th', { class: 'th', style: 'width:90px' }, _('Route')),
				E('th', { class: 'th' }, _('Upstream')),
				E('th', { class: 'th', style: 'width:80px; text-align:right' }, _('Time (ms)'))
			])
		]);
		const f = this.filter.toLowerCase();
		let shown = 0;
		for (const e of entries) {
			if (f && !(e.qname.includes(f) || e.client.includes(f) ||
				e.route.includes(f) || (e.upstream || '').includes(f)))
				continue;
			if (++shown > 500) break;
			tbl.appendChild(E('tr', { class: 'tr' }, [
				E('td', { class: 'td' }, fmtTime(e.ts)),
				E('td', { class: 'td' }, e.client),
				E('td', { class: 'td', style: 'word-break:break-all' }, e.qname),
				E('td', { class: 'td' }, QTYPES[e.qtype] || String(e.qtype)),
				E('td', { class: 'td' }, routeBadge(e.route)),
				E('td', { class: 'td' }, e.upstream || '-'),
				E('td', { class: 'td', style: 'text-align:right' },
					e.rtt_ms < 0.1 ? '<0.1' : String(e.rtt_ms))
			]));
		}
		if (shown === 0)
			tbl.appendChild(E('tr', { class: 'tr' }, [
				E('td', { class: 'td', colspan: 7, style: 'opacity:.6' }, _('(no entries)'))
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
