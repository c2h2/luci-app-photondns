'use strict';
'require poll';
'require rpc';
'require ui';
'require view';

const callLog = rpc.declare({
	object: 'luci.photondns',
	method: 'print_log',
	expect: { '': {} }
});

const callCleanLog = rpc.declare({
	object: 'luci.photondns',
	method: 'clean_log',
	expect: { '': {} }
});

return view.extend({
	load() {
		return L.resolveDefault(callLog(), {});
	},

	handleClean() {
		return callCleanLog().then(() => {
			const pre = document.getElementById('photondns_log');
			if (pre) pre.textContent = '';
			ui.addNotification(null, E('p', _('Log cleared.')), 'info');
		});
	},

	render(data) {
		const pre = E('pre', {
			id: 'photondns_log',
			style: 'max-height:640px; overflow-y:auto; white-space:pre-wrap; ' +
				'padding:8px; border:1px solid rgba(128,128,128,.35); border-radius:6px'
		}, (data && data.log) ? data.log : _('(empty)'));

		poll.add(() => {
			return L.resolveDefault(callLog(), {}).then(res => {
				if (res && typeof res.log === 'string') {
					const stick = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 20;
					pre.textContent = res.log || _('(empty)');
					if (stick) pre.scrollTop = pre.scrollHeight;
				}
			});
		}, 5);

		return E('div', { class: 'cbi-map' }, [
			E('h2', {}, _('photondns Logs')),
			E('div', { style: 'margin:8px 0' }, [
				E('button', {
					class: 'btn cbi-button-remove',
					click: ui.createHandlerFn(this, 'handleClean')
				}, _('Clear Log'))
			]),
			pre
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
