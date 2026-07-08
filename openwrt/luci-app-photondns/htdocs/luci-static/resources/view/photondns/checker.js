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
		box.innerHTML = '';
		box.appendChild(E('div', { style: 'opacity:.6' }, _('Resolving %s...').format(name)));
		callResolve(name, type || 'A').then(res => {
			const d = parseRaw(res);
			const b = document.getElementById('checker_result');
			if (!b) return;
			b.innerHTML = '';
			if (!d) {
				b.appendChild(E('div', { class: 'alert-message warning' },
					_('No response from photondns (is it running?).')));
				return;
			}
			const rows = [];
			const kv = (k, v) => rows.push(E('tr', { class: 'tr' }, [
				E('td', { class: 'td', style: 'width:130px; font-weight:bold' }, k),
				E('td', { class: 'td', style: 'word-break:break-all' }, v)
			]));
			kv(_('Name'), d.name || name);
			kv(_('Type'), d.type || type || 'A');
			rows.push(E('tr', { class: 'tr' }, [
				E('td', { class: 'td', style: 'font-weight:bold' }, _('Route')),
				E('td', { class: 'td' }, routeBadge(d.route))
			]));
			if (d.error) kv(_('Error'), d.error);
			kv(_('Response code'), d.rcode || '-');
			const answers = (d.answers && d.answers.length)
				? E('div', {}, d.answers.map(a => E('div', { style: 'font-family:monospace' }, a)))
				: E('span', { style: 'opacity:.6' }, _('(no address records)'));
			rows.push(E('tr', { class: 'tr' }, [
				E('td', { class: 'td', style: 'font-weight:bold' }, _('Answers')),
				E('td', { class: 'td' }, answers)
			]));
			if (d.ttl != null) kv(_('TTL'), String(d.ttl) + ' s');
			if (d.upstream) kv(_('Upstream'), d.upstream);
			if (d.elapsed_ms != null) kv(_('Elapsed'), String(d.elapsed_ms) + ' ms');
			b.appendChild(E('table', { class: 'table', style: 'max-width:640px' }, rows));
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
				  'full resolve (cache-first) and shows the answers, route, TTL and timing. ' +
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
