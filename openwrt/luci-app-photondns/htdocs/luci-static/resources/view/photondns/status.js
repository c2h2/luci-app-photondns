'use strict';
'require poll';
'require rpc';
'require ui';
'require view';

const callStats = rpc.declare({
	object: 'luci.photondns',
	method: 'stats',
	expect: { '': {} }
});

const callFlushCache = rpc.declare({
	object: 'luci.photondns',
	method: 'flush_cache',
	expect: { '': {} }
});

function fmtUptime(s) {
	const d = Math.floor(s / 86400);
	const h = Math.floor((s % 86400) / 3600);
	const m = Math.floor((s % 3600) / 60);
	if (d > 0) return '%dd %dh %dm'.format(d, h, m);
	if (h > 0) return '%dh %dm'.format(h, m);
	return '%dm %ds'.format(m, s % 60);
}

function card(title, rows) {
	return E('div', {
		style: 'flex:1; min-width:240px; margin:5px; padding:12px 16px; border:1px solid ' +
			'rgba(128,128,128,.35); border-radius:8px;'
	}, [
		E('h4', { style: 'margin:0 0 8px 0' }, title),
		E('table', { style: 'width:100%' }, rows.map(r =>
			E('tr', {}, [
				E('td', { style: 'opacity:.7; padding:1px 0' }, r[0]),
				E('td', { style: 'text-align:right; font-weight:bold' }, r[1])
			])
		))
	]);
}

function healthBadge(healthy) {
	return E('span', {
		style: 'padding:1px 8px; border-radius:8px; color:#fff; background:' +
			(healthy ? '#2ca02c' : '#d43f3a')
	}, healthy ? _('UP') : _('DOWN'));
}

function render_stats(container, res) {
	let stats = null;
	if (res && res.running && res.raw) {
		try { stats = JSON.parse(res.raw); } catch (e) { }
	}

	const box = E('div');
	if (!stats) {
		box.appendChild(E('div', { class: 'alert-message warning' },
			_('photondns is not running (or the API is unreachable). Enable and start it from Basic Settings.')));
		container.innerHTML = '';
		container.appendChild(box);
		return;
	}

	const q = stats.queries, c = stats.cache;
	box.appendChild(E('div', { style: 'display:flex; flex-wrap:wrap' }, [
		card(_('Service'), [
			[_('Status'), E('span', { style: 'color:#2ca02c' }, _('RUNNING'))],
			[_('Version'), stats.version],
			[_('Uptime'), fmtUptime(stats.uptime)],
			[_('Queries/min'), String(q.qpm)]
		]),
		card(_('Queries'), [
			[_('Total'), String(q.total)],
			[_('UDP / TCP'), '%d / %d'.format(q.udp, q.tcp)],
			[_('Blocked'), String(q.blocked)],
			[_('Hosts / Redirected'), '%d / %d'.format(q.hosts, q.redirected)],
			[_('Hedged (failover races)'), String(q.hedged)],
			[_('SERVFAIL'), String(q.servfail)]
		]),
		card(_('Cache'), [
			[_('Entries'), '%d / %d'.format(c.size, c.capacity)],
			[_('Hit rate'), c.hit_rate + '%'],
			[_('Hits / Misses'), '%d / %d'.format(c.hits, c.misses)],
			[_('Stale served'), String(c.stale_served)],
			[_('Prefetches'), String(c.prefetches)],
			[_('Evictions'), String(c.evictions)]
		])
	]));

	const rows = (stats.upstreams || []).map(u => [
		u.group,
		u.addr,
		healthBadge(u.healthy),
		u.ewma_ms > 0 ? '%.1f ms'.format(u.ewma_ms) : '-',
		u.last_rtt_ms > 0 ? '%.1f ms'.format(u.last_rtt_ms) : '-',
		String(u.ok),
		String(u.fail),
		String(u.down_events)
	]);
	const tbl = E('table', { class: 'table cbi-section-table' }, [
		E('tr', { class: 'tr table-titles' }, [
			E('th', { class: 'th' }, _('Group')),
			E('th', { class: 'th' }, _('Upstream')),
			E('th', { class: 'th' }, _('State')),
			E('th', { class: 'th' }, _('Latency (EWMA)')),
			E('th', { class: 'th' }, _('Last RTT')),
			E('th', { class: 'th' }, _('OK')),
			E('th', { class: 'th' }, _('Fail')),
			E('th', { class: 'th' }, _('Outages'))
		])
	]);
	rows.forEach(r => tbl.appendChild(E('tr', { class: 'tr' },
		r.map(v => E('td', { class: 'td' }, v)))));

	box.appendChild(E('h3', { style: 'margin-top:14px' }, _('Upstream Servers')));
	box.appendChild(tbl);

	container.innerHTML = '';
	container.appendChild(box);
}

return view.extend({
	load() {
		return L.resolveDefault(callStats(), {});
	},

	handleFlushCache() {
		return callFlushCache().then(res => {
			if (res && res.success)
				ui.addNotification(null, E('p', _('DNS cache flushed.')), 'info');
			else
				ui.addNotification(null, E('p', _('Flush failed, is photondns running?')), 'error');
		});
	},

	render(data) {
		const container = E('div', {}, _('Collecting data...'));
		render_stats(container, data);

		poll.add(() => {
			return L.resolveDefault(callStats(), {}).then(res => {
				render_stats(container, res);
			});
		}, 3);

		return E('div', { class: 'cbi-map' }, [
			E('h2', {}, _('photondns Status')),
			E('div', { class: 'cbi-map-descr' },
				_('High-performance Rust DNS forwarder with caching and adaptive multi-path failover.')),
			E('div', { style: 'margin:8px 0' }, [
				E('button', {
					class: 'btn cbi-button-apply',
					click: ui.createHandlerFn(this, 'handleFlushCache')
				}, _('Flush DNS Cache'))
			]),
			container
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
