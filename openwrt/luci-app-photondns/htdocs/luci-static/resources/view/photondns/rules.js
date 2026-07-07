'use strict';
'require form';
'require fs';
'require ui';
'require view';

const RULE_FILES = [
	['hosts', '/etc/photondns/hosts.txt', _('Hosts'),
		_('Static host records: "<name> <ip> [ip...]" per line. Answers A/AAAA locally.')],
	['block', '/etc/photondns/block.txt', _('Block List'),
		_('Domains answered with NXDOMAIN. "example.com" blocks all subdomains, "full:example.com" exact only.')],
	['local_domains', '/etc/photondns/local_domains.txt', _('Local Domains'),
		_('Domains resolved by the "local" upstream group (configure Local-domain DNS servers first).')],
	['redirect', '/etc/photondns/redirect.txt', _('Redirect'),
		_('"<from-domain> <to-domain>" per line: answer queries for from-domain with the records of to-domain.')],
	['prewarm', '/etc/photondns/prewarm.txt', _('Prewarm'),
		_('Domains kept always-resolved (one per line) so a first visit is never a slow cold miss. Default set = YouTube/Google. Enable "Prewarm popular domains" in Basic Settings.')]
];

return view.extend({
	load() {
		return Promise.all(RULE_FILES.map(f =>
			L.resolveDefault(fs.read(f[1]), '')
		));
	},

	render(data) {
		let m, s, o;

		m = new form.Map('photondns', _('photondns Rules'),
			_('Rule files are applied on service restart.'));

		s = m.section(form.NamedSection, 'main', 'photondns');

		RULE_FILES.forEach((f, i) => {
			s.tab(f[0], f[2]);
			o = s.taboption(f[0], form.TextValue, '_' + f[0], null, f[3]);
			o.rows = 20;
			o.cfgvalue = () => data[i] || '';
			o.write = (section_id, value) => {
				const cur = data[i] || '';
				const next = (value || '').trim().replace(/\r\n/g, '\n') + '\n';
				if (cur === next) return;
				return fs.write(f[1], next);
			};
			o.remove = () => fs.write(f[1], '\n');
		});

		return m.render();
	},

	handleSaveApply(ev) {
		return this.handleSave(ev).then(() => {
			return fs.exec('/etc/init.d/photondns', ['restart']);
		}).then(() => {
			ui.addNotification(null, E('p', _('Rules saved and photondns restarted.')), 'info');
		});
	}
});
