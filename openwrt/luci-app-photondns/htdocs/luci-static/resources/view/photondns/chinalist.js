'use strict';
'require poll';
'require rpc';
'require ui';
'require view';

const LISTS = [
	{
		key: 'chinalist',
		title: _('China Domain List'),
		descr: _('Mainland-China domains routed to the Local-domain DNS group (split DNS). Source: felixonmars/dnsmasq-china-list.'),
		file: '/etc/photondns/china_list.txt',
		update: rpc.declare({ object: 'luci.photondns', method: 'update_chinalist', expect: { '': {} } }),
		status: rpc.declare({ object: 'luci.photondns', method: 'chinalist_status', expect: { '': {} } })
	},
	{
		key: 'adlist',
		title: _('Ad Block Lists'),
		descr: _('Advertising / tracker domains answered with NXDOMAIN. Sources are configured in Basic Settings (default: anti-AD).'),
		file: '/etc/photondns/ad_list.txt',
		update: rpc.declare({ object: 'luci.photondns', method: 'update_adlist', expect: { '': {} } }),
		status: rpc.declare({ object: 'luci.photondns', method: 'adlist_status', expect: { '': {} } })
	}
];

function statusText(st) {
	if (st && st.updating)
		return E('em', { style: 'color:#c7a500' }, _('Updating...'));
	if (st && st.exists)
		return E('strong', {}, _('%d domains, updated %s').format(st.domains,
			new Date(st.mtime * 1000).toLocaleString()));
	return E('em', { style: 'color:#d43f3a' }, _('not downloaded yet'));
}

return view.extend({
	load() {
		return Promise.all(LISTS.map(l => L.resolveDefault(l.status(), {})));
	},

	refresh(list) {
		return L.resolveDefault(list.status(), {}).then(st => {
			const s = document.getElementById(list.key + '_status');
			if (s) {
				s.innerHTML = '';
				s.appendChild(statusText(st));
			}
			const pre = document.getElementById(list.key + '_log');
			if (pre && st && typeof st.log === 'string' && st.log.trim() !== '')
				pre.textContent = st.log;
			const btn = document.getElementById(list.key + '_btn');
			if (btn) btn.disabled = !!(st && st.updating);
			return st;
		});
	},

	handleUpdate(list) {
		return list.update().then(res => {
			if (!res || !res.success) {
				ui.addNotification(null, E('p', _('Update failed to start: %s')
					.format((res && res.error) || '?')), 'error');
				return;
			}
			ui.addNotification(null, E('p', _('Update started in the background.')), 'info');
			const tick = () => this.refresh(list).then(st => {
				if (st && !st.updating) {
					poll.remove(tick);
					ui.addNotification(null, E('p',
						_('%s: %d domains.').format(list.title, (st && st.domains) || 0)), 'info');
				}
			});
			poll.add(tick, 2);
		});
	},

	renderList(list, st) {
		return E('div', { class: 'cbi-section' }, [
			E('h3', {}, list.title),
			E('div', { class: 'cbi-section-descr' }, list.descr),
			E('table', { class: 'table' }, [
				E('tr', { class: 'tr' }, [
					E('td', { class: 'td left', style: 'width:220px; opacity:.7' }, _('Current list')),
					E('td', { class: 'td left', id: list.key + '_status' }, statusText(st))
				]),
				E('tr', { class: 'tr' }, [
					E('td', { class: 'td left', style: 'opacity:.7' }, _('List file')),
					E('td', { class: 'td left' }, list.file)
				])
			]),
			E('div', { style: 'margin:12px 0' }, [
				E('button', {
					id: list.key + '_btn',
					class: 'btn cbi-button-apply',
					disabled: st && st.updating ? true : null,
					click: ui.createHandlerFn(this, 'handleUpdate', list)
				}, _('Update Now'))
			]),
			E('pre', {
				id: list.key + '_log',
				style: 'max-height:200px; overflow-y:auto; white-space:pre-wrap; ' +
					'padding:8px; border:1px solid rgba(128,128,128,.35); border-radius:6px'
			}, (st && st.log && st.log.trim() !== '') ? st.log : _('(empty)'))
		]);
	},

	render(data) {
		return E('div', { class: 'cbi-map' }, [
			E('h2', {}, _('List Updates')),
			E('div', { class: 'cbi-map-descr' },
				_('Manually download or refresh the domain lists, fetched via CN-friendly mirrors. Enable the corresponding features in Basic Settings to use them.')),
			...LISTS.map((l, i) => this.renderList(l, data[i]))
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
