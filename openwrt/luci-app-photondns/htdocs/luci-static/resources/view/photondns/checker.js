'use strict';
'require rpc';
'require ui';
'require view';

// Autocomplete against the cache. q = typed text, limit = max suggestions.
const callCacheSearch = rpc.declare({
	object: 'luci.photondns',
	method: 'cache_search',
	params: ['q', 'limit'],
	expect: { '': {} }
});

// Ask every upstream of the routed group at once (speed / answer comparison).
const callCompare = rpc.declare({
	object: 'luci.photondns',
	method: 'compare',
	params: ['name', 'type'],
	expect: { '': {} }
});

// Full resolve (cache-first, may hit upstream) for a selected name.
const callResolve = rpc.declare({
	object: 'luci.photondns',
	method: 'resolve',
	params: ['name', 'type'],
	expect: { '': {} }
});

const ROUTE_COLORS = {
	cache: '#2ca02c', stale: '#17a2b8', hosts: '#7952b3', lan: '#20c997',
	blocked: '#d43f3a', redirect: '#6c757d', servfail: '#8b0000',
	failed: '#8b0000', local: '#e67e22', main: '#0d6efd'
};

function routeBadge(route) {
	const color = ROUTE_COLORS[route] || '#0d6efd';
	return E('span', {
		style: 'padding:1px 8px; border-radius:8px; color:#fff; font-size:90%; background:' + color
	}, route || '?');
}

// Human label for where a routing decision came from (Router::explain source).
function sourceLabel(src) {
	return {
		type65: _('Reject HTTPS/SVCB (type 65) setting'),
		hosts_file: _('Hosts rule file'),
		redirect_file: _('Redirect rule file'),
		lan_leases: _('LAN client name (DHCP lease / LAN hosts)'),
		lan_ptr: _('LAN client reverse lookup'),
		block_file: _('Block rule file'),
		ad_list: _('Ad block list'),
		special_tld: _('Special-use TLD protection (.local, .lan, ...)'),
		private_ptr: _('Private address PTR protection'),
		local_domains_file: _('Local Domains rule file'),
		china_list: _('China domain list (geosite:cn)'),
		'default': _('No rule matched (default group)')
	}[src] || src || '-';
}

function matchLabel(m) {
	return {
		full: _('Exact (full: entry)'),
		exact: _('Exact name'),
		suffix: _('Suffix (domain and all subdomains)'),
		none: '-'
	}[m] || m || '-';
}

function agoText(ts) {
	const s = Math.max(0, Math.floor(Date.now() / 1000) - ts);
	if (s < 60) return _('%ds ago').format(s);
	if (s < 3600) return _('%dm ago').format(Math.floor(s / 60));
	return _('%dh ago').format(Math.floor(s / 3600));
}

function mono(t) { return E('span', { style: 'font-family:monospace; word-break:break-all' }, t); }

function kvTable(rows) {
	return E('table', { class: 'table', style: 'max-width:760px' }, rows.map(r =>
		E('tr', { class: 'tr' }, [
			E('td', { class: 'td', style: 'width:150px; font-weight:bold' }, r[0]),
			E('td', { class: 'td', style: 'word-break:break-all' }, r[1])
		])));
}

// One-line summary of why `name` was routed where it was.
function explainSentence(name, x) {
	const rule = x.rule ? '"' + x.rule + '"' : '';
	if (x.action === 'forward' && x.source === 'default')
		return _('%s matched no routing rule, so it goes to the default group "%s".').format(name, x.group);
	if (x.action === 'forward')
		return _('%s matched %s rule %s in %s, so it goes to group "%s".').format(
			name, x.match === 'full' ? _('exact') : _('suffix'), rule, sourceLabel(x.source), x.group);
	if (x.action === 'blocked' && x.source === 'type65')
		return _('Type 65 (HTTPS/SVCB) queries are rejected by the "Reject type 65" setting.');
	if (x.action === 'blocked')
		return _('%s matched %s rule %s in %s, so it is answered NXDOMAIN.').format(
			name, x.match === 'full' ? _('exact') : _('suffix'), rule, sourceLabel(x.source));
	if (x.action === 'hosts')
		return _('%s is in the hosts rule file and answers %s directly.').format(name, x.target);
	if (x.action === 'redirect')
		return _('%s is redirected to %s by the redirect rule file.').format(name, x.target);
	if (x.action === 'lan')
		return _('%s is a known LAN client and answers %s locally.').format(name, x.target);
	return '';
}

function parseRaw(res) {
	if (!res || !res.raw) return null;
	try { return JSON.parse(res.raw); } catch (e) { return null; }
}

return view.extend({
	// currently highlighted suggestion index (-1 = none)
	sel: -1,
	items: [],
	timer: null,

	// Render the full resolve result of `name` into the result box.
	showResult(name, type) {
		const box = document.getElementById('checker_result');
		if (!box) return;
		type = type || 'A';
		box.innerHTML = '';
		box.appendChild(E('div', { style: 'opacity:.6' }, _('Resolving %s...').format(name)));
		callResolve(name, type).then(res => {
			const d = parseRaw(res);
			const b = document.getElementById('checker_result');
			if (!b) return;
			b.innerHTML = '';
			if (!d) {
				b.appendChild(E('div', { class: 'alert-message warning' },
					_('No response from photondns (is it running?).')));
				return;
			}

			/* ---- answer ---- */
			const rows = [];
			rows.push([_('Name'), d.name || name]);
			rows.push([_('Type'), d.type || type]);
			rows.push([_('Route'), routeBadge(d.route)]);
			if (d.error) rows.push([_('Error'), d.error]);
			rows.push([_('Response code'), d.rcode || '-']);
			rows.push([_('Answers'), (d.answers && d.answers.length)
				? E('div', {}, d.answers.map(a => E('div', { style: 'font-family:monospace' }, a)))
				: E('span', { style: 'opacity:.6' }, _('(no address records)'))]);
			if (d.ttl != null) rows.push([_('TTL'), String(d.ttl) + ' s']);
			// who answered: live winner, else who last filled the cached entry
			let answeredBy = d.upstream || '';
			if (d.upstream && d.route !== 'redirect') {
				rows.push([_('Answered by'), E('span', {}, [mono(d.upstream), ' ',
					E('span', { style: 'opacity:.6' }, _('(won the race just now)'))])]);
			} else if (d.upstream) {
				rows.push([_('Redirect target'), mono(d.upstream)]);
				answeredBy = '';
			} else if (d.last_upstream && (d.route === 'cache' || d.route === 'stale')) {
				answeredBy = d.last_upstream.upstream;
				rows.push([_('Answered by'), E('span', {}, [mono(answeredBy), ' ',
					E('span', { style: 'opacity:.6' }, _('(cached; filled %s, %s ms)').format(
						agoText(d.last_upstream.ts), d.last_upstream.rtt_ms))])]);
			}
			if (d.elapsed_ms != null) rows.push([_('Elapsed'), String(d.elapsed_ms) + ' ms']);
			b.appendChild(kvTable(rows));

			/* ---- why this route ---- */
			const x = d.explain;
			if (x) {
				b.appendChild(E('h3', { style: 'margin-top:18px' }, _('Why this route')));
				b.appendChild(E('div', { style: 'margin:6px 0 8px' }, explainSentence(d.name || name, x)));
				const er = [];
				er.push([_('Decision'), x.action === 'forward'
					? E('span', {}, [_('forward to group'), ' ', routeBadge(x.group)])
					: routeBadge(x.action)]);
				er.push([_('Rule source'), sourceLabel(x.source)]);
				er.push([_('Match type'), matchLabel(x.match)]);
				if (x.rule) er.push([_('Matched rule'), mono(x.rule)]);
				if (x.target) er.push([_('Rule target'), mono(x.target)]);
				if (x.action === 'forward') {
					if (x.group_fallback)
						er.push([_('Note'), _('No "local" group is configured, so the default group is used.')]);
					er.push([_('Strategy'), x.strategy || '-']);
					er.push([_('Group upstreams'), E('div', {},
						(x.upstreams || []).map(u => E('div', {}, mono(u))).concat(
						(x.backups || []).map(u => E('div', {}, [mono(u), ' ',
							E('span', { style: 'opacity:.6' }, _('(backup)'))]))))]);
					if (x.aaaa_mode)
						er.push([_('IPv6 (AAAA) mode'), _('%s - clients may get an empty AAAA answer').format(x.aaaa_mode)]);
				}
				b.appendChild(kvTable(er));
			}

			/* ---- upstream comparison ---- */
			const cmpBox = E('div', { id: 'checker_compare' });
			b.appendChild(E('h3', { style: 'margin-top:18px' }, [
				_('Upstream comparison'), ' ',
				E('button', {
					class: 'cbi-button cbi-button-action', style: 'margin-left:8px',
					click: () => this.runCompare(d.name || name, type, answeredBy)
				}, _('Run again'))
			]));
			b.appendChild(E('div', { class: 'cbi-section-descr', style: 'margin-bottom:6px' },
				x && x.action !== 'forward'
					? _('This name never reaches an upstream; showing how the default group would answer it.')
					: _('Asks every upstream of the group at the same time, bypassing the cache, so you can compare speed and answers. Does not affect failover health.')));
			b.appendChild(cmpBox);
			this.runCompare(d.name || name, type, answeredBy);
		});
	},

	// Query every upstream of the routed group and draw the comparison table.
	runCompare(name, type, answeredBy) {
		const box = document.getElementById('checker_compare');
		if (!box) return;
		box.innerHTML = '';
		box.appendChild(E('div', { style: 'opacity:.6' }, _('Querying all upstreams...')));
		callCompare(name, type).then(res => {
			const d = parseRaw(res);
			const bx = document.getElementById('checker_compare');
			if (!bx) return;
			bx.innerHTML = '';
			if (!d || !d.results) {
				bx.appendChild(E('div', { class: 'alert-message warning' },
					(d && d.error) || _('No response from photondns (is it running?).')));
				return;
			}
			const okMs = d.results.filter(r => r.ok).map(r => r.ms);
			const maxMs = Math.max.apply(null, okMs.concat([1]));
			const fastest = okMs.length ? Math.min.apply(null, okMs) : null;
			const head = E('tr', { class: 'tr table-titles' }, [
				_('Upstream'), _('Time'), _('Response code'), _('Answers'), _('Avg (EWMA)'), _('Health')
			].map(t => E('th', { class: 'th' }, t)));
			const rows = d.results.map(r => {
				const tags = [];
				if (r.upstream === answeredBy)
					tags.push(E('span', { style: 'margin-left:6px; padding:0 6px; border-radius:8px; color:#fff; font-size:85%; background:#0d6efd' }, _('answered')));
				if (r.ok && r.ms === fastest)
					tags.push(E('span', { style: 'margin-left:6px; padding:0 6px; border-radius:8px; color:#fff; font-size:85%; background:#2ca02c' }, _('fastest')));
				if (r.backup)
					tags.push(E('span', { style: 'margin-left:6px; opacity:.6' }, _('(backup)')));
				const pct = r.ok ? Math.max(2, Math.round(r.ms / maxMs * 100)) : 0;
				const time = r.ok
					? E('div', { style: 'min-width:140px' }, [
						E('div', { style: 'font-family:monospace' }, r.ms.toFixed(1) + ' ms'),
						E('div', { style: 'height:6px; border-radius:3px; background:' +
							(r.ms === fastest ? '#2ca02c' : '#0d6efd') + '; width:' + pct + '%' })])
					: E('span', { style: 'color:#d43f3a' }, r.error || _('failed'));
				return E('tr', { class: 'tr' }, [
					E('td', { class: 'td' }, [mono(r.upstream)].concat(tags)),
					E('td', { class: 'td' }, time),
					E('td', { class: 'td' }, r.rcode || '-'),
					E('td', { class: 'td', style: 'font-family:monospace; font-size:90%' },
						(r.answers && r.answers.length) ? r.answers.join(' ') : '-'),
					E('td', { class: 'td' }, r.ewma_ms ? r.ewma_ms + ' ms' : '-'),
					E('td', { class: 'td' }, E('span', { style: 'color:' + (r.healthy ? '#2ca02c' : '#d43f3a') },
						r.healthy ? _('healthy') : _('down')))
				]);
			});
			bx.appendChild(E('div', { style: 'margin-bottom:4px' },
				_('Group "%s" - %d upstreams').format(d.group, d.results.length)));
			bx.appendChild(E('table', { class: 'table' }, [head].concat(rows)));
		});
	},

	// Rebuild the suggestion dropdown from the current items list.
	renderSuggestions() {
		const drop = document.getElementById('checker_drop');
		if (!drop) return;
		drop.innerHTML = '';
		if (!this.items.length) { drop.style.display = 'none'; return; }
		drop.style.display = 'block';
		this.items.forEach((it, i) => {
			const active = (i === this.sel);
			const row = E('div', {
				'class': 'checker-sugg',
				'style': 'padding:5px 10px; cursor:pointer; display:flex; justify-content:space-between; gap:12px;' +
					(active ? 'background:#e8f0fe;' : ''),
				'data-idx': i,
				'click': ui.createHandlerFn(this, function() { this.choose(i); }),
				'mouseover': () => { this.sel = i; this.highlight(); }
			}, [
				E('span', { style: 'font-family:monospace; word-break:break-all' }, it.name),
				E('span', { style: 'opacity:.7; white-space:nowrap' }, [
					it.type + '  ',
					E('span', { style: 'color:' + (it.fresh ? '#2ca02c' : '#e67e22') },
						it.fresh ? _('fresh') : _('stale'))
				])
			]);
			drop.appendChild(row);
		});
	},

	highlight() {
		const drop = document.getElementById('checker_drop');
		if (!drop) return;
		Array.prototype.forEach.call(drop.children, (c, i) => {
			c.style.background = (i === this.sel) ? '#e8f0fe' : '';
		});
	},

	// Pick suggestion i: fill the input and show its full result.
	choose(i) {
		const it = this.items[i];
		if (!it) return;
		const input = document.getElementById('checker_input');
		if (input) input.value = it.name;
		this.items = [];
		this.sel = -1;
		this.renderSuggestions();
		this.showResult(it.name, it.type);
	},

	// Debounced query as the user types.
	onType(val) {
		if (this.timer) { window.clearTimeout(this.timer); this.timer = null; }
		const q = (val || '').trim();
		this.timer = window.setTimeout(L.bind(function() {
			callCacheSearch(q, 15).then(res => {
				const d = parseRaw(res);
				this.items = (d && d.items) ? d.items : [];
				this.sel = -1;
				this.renderSuggestions();
				const cs = document.getElementById('checker_cachesize');
				if (cs && d && d.cache_size != null)
					cs.textContent = _('%d entries cached').format(d.cache_size);
			});
		}, this), 120);
	},

	onKey(ev) {
		const n = this.items.length;
		if (ev.key === 'ArrowDown' && n) {
			this.sel = (this.sel + 1) % n; this.highlight(); ev.preventDefault();
		} else if (ev.key === 'ArrowUp' && n) {
			this.sel = (this.sel - 1 + n) % n; this.highlight(); ev.preventDefault();
		} else if (ev.key === 'Enter') {
			ev.preventDefault();
			if (this.sel >= 0) this.choose(this.sel);
			else {
				// no highlight: resolve exactly what was typed
				const v = ev.target.value.trim();
				if (v) { this.items = []; this.renderSuggestions(); this.showResult(v, 'A'); }
			}
		} else if (ev.key === 'Escape') {
			this.items = []; this.sel = -1; this.renderSuggestions();
		}
	},

	render() {
		const input = E('input', {
			id: 'checker_input',
			type: 'text',
			class: 'cbi-input-text',
			style: 'width:100%; max-width:520px',
			autocomplete: 'off',
			spellcheck: 'false',
			placeholder: _('Start typing a domain (e.g. github.com)...'),
			keyup: (ev) => {
				if (['ArrowDown', 'ArrowUp', 'Enter', 'Escape'].indexOf(ev.key) < 0)
					this.onType(ev.target.value);
			},
			keydown: (ev) => this.onKey(ev)
		});

		const drop = E('div', {
			id: 'checker_drop',
			style: 'display:none; position:absolute; z-index:50; background:var(--background-color-high,#fff);' +
				'border:1px solid #ccc; border-radius:0 0 4px 4px; max-width:520px; width:100%;' +
				'box-shadow:0 4px 12px rgba(0,0,0,.15); max-height:360px; overflow:auto'
		});

		// hide dropdown on outside click
		document.addEventListener('click', (ev) => {
			const wrap = document.getElementById('checker_wrap');
			if (wrap && !wrap.contains(ev.target)) {
				this.items = []; this.renderSuggestions();
			}
		});

		return E('div', { class: 'cbi-map' }, [
			E('h2', {}, _('Cache Lookup')),
			E('div', { class: 'cbi-map-descr' },
				_('Type a domain to search what photondns currently has cached. The 15 closest cached ' +
				  'records appear as you type (↓/↑ to move, Enter to pick); selecting one runs a ' +
				  'full resolve (cache-first) and shows the answers, route, TTL and timing, which ' +
				  'routing rule sent it there (suffix / exact match, China list, ad list, ...), ' +
				  'which upstream answered and a live speed comparison of every upstream in the group. ' +
				  'Pressing Enter without a selection resolves exactly what you typed.')),
			E('div', { id: 'checker_wrap', style: 'position:relative; margin:10px 0' }, [
				input,
				drop
			]),
			E('div', { id: 'checker_cachesize', style: 'font-size:90%; opacity:.6; margin-bottom:14px' }, ''),
			E('div', { id: 'checker_result' })
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
