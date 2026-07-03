'use strict';
'require poll';
'require rpc';
'require ui';
'require view';

const callUpdateChinaList = rpc.declare({
	object: 'luci.photondns',
	method: 'update_chinalist',
	expect: { '': {} }
});

const callChinaListStatus = rpc.declare({
	object: 'luci.photondns',
	method: 'chinalist_status',
	expect: { '': {} }
});

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
		return L.resolveDefault(callChinaListStatus(), {});
	},

	refresh() {
		return L.resolveDefault(callChinaListStatus(), {}).then(st => {
			const s = document.getElementById('chinalist_status');
			if (s) {
				s.innerHTML = '';
				s.appendChild(statusText(st));
			}
			const pre = document.getElementById('chinalist_log');
			if (pre && typeof st.log === 'string' && st.log.trim() !== '')
				pre.textContent = st.log;
			const btn = document.getElementById('chinalist_btn');
			if (btn) btn.disabled = !!st.updating;
			return st;
		});
	},

	handleUpdate() {
		return callUpdateChinaList().then(res => {
			if (!res || !res.success) {
				ui.addNotification(null, E('p', _('Update failed to start: %s')
					.format((res && res.error) || '?')), 'error');
				return;
			}
			ui.addNotification(null, E('p', _('China list update started in the background.')), 'info');
			const tick = () => this.refresh().then(st => {
				if (st && !st.updating) {
					poll.remove(tick);
					ui.addNotification(null, E('p',
						_('China list: %d domains.').format(st.domains || 0)), 'info');
				}
			});
			poll.add(tick, 2);
		});
	},

	render(st) {
		return E('div', { class: 'cbi-map' }, [
			E('h2', {}, _('China Domain List')),
			E('div', { class: 'cbi-map-descr' },
				_('Manually download or update the mainland-China domain list used for split-DNS routing. Source: felixonmars/dnsmasq-china-list, fetched via CN-friendly mirrors. Enable "China domain list (split DNS)" and configure Local-domain DNS servers in Basic Settings to use it.')),
			E('div', { class: 'cbi-section' }, [
				E('table', { class: 'table' }, [
					E('tr', { class: 'tr' }, [
						E('td', { class: 'td left', style: 'width:220px; opacity:.7' }, _('Current list')),
						E('td', { class: 'td left', id: 'chinalist_status' }, statusText(st))
					]),
					E('tr', { class: 'tr' }, [
						E('td', { class: 'td left', style: 'opacity:.7' }, _('List file')),
						E('td', { class: 'td left' }, '/etc/photondns/china_list.txt')
					])
				]),
				E('div', { style: 'margin:12px 0' }, [
					E('button', {
						id: 'chinalist_btn',
						class: 'btn cbi-button-apply',
						disabled: st && st.updating ? true : null,
						click: ui.createHandlerFn(this, 'handleUpdate')
					}, _('Update Now'))
				]),
				E('h4', {}, _('Update log')),
				E('pre', {
					id: 'chinalist_log',
					style: 'max-height:400px; overflow-y:auto; white-space:pre-wrap; ' +
						'padding:8px; border:1px solid rgba(128,128,128,.35); border-radius:6px'
				}, (st && st.log && st.log.trim() !== '') ? st.log : _('(empty)'))
			])
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
