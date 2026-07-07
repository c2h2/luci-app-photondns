'use strict';
'require view';
'require ui';

/*
 * Bilingual (English / 简体中文) help & glossary for photondns.
 *
 * The content is intentionally self-contained data - NOT run through _() -
 * because this page shows BOTH languages on demand via an in-page toggle,
 * independent of the LuCI UI locale. Each section has an `en` and a `zh`
 * field; the toggle simply re-renders with the chosen language.
 */

var HELP = [
	{
		id: 'what',
		en: {
			title: 'What is photondns?',
			body: [
				'photondns is the program on this router that answers DNS questions for every device on your network. When your phone, TV or laptop wants to reach a website, it first asks "what is the address of this name?" - photondns answers that, quickly, and remembers the answer so the next device gets it instantly.',
				'It also decides how to look up each name: trusted encrypted servers abroad for global sites, and a fast local server for domestic ones, automatically switching away from anything that becomes slow or unreachable.'
			]
		},
		zh: {
			title: 'photondns 是什么？',
			body: [
				'photondns 是本路由器上负责为网络中每台设备解析 DNS 的程序。当你的手机、电视或电脑想访问某个网站时，它会先询问“这个名字对应的地址是什么？”——photondns 会快速给出答案，并记住结果，让下一台设备能立即拿到。',
				'它还会为每个域名选择查询方式：全球网站走境外可信加密服务器，国内网站走快速的本地服务器，并在某个上游变慢或不可达时自动切换。'
			]
		}
	},
	{
		id: 'dns',
		en: {
			title: 'DNS (Domain Name System)',
			body: [
				'The internet\'s phone book. Humans use names like youtube.com; computers need numeric addresses like 142.251.220.174 (an "IP address"). DNS is the system that translates a name into its address.',
				'Every time you open a website, app or video, your device makes one or more DNS lookups behind the scenes. If DNS is slow, everything feels slow to start - even when your actual internet speed is fine.'
			]
		},
		zh: {
			title: 'DNS（域名系统）',
			body: [
				'互联网的“电话簿”。人类使用 youtube.com 这样的名字，而计算机需要 142.251.220.174 这样的数字地址（即“IP 地址”）。DNS 就是把名字翻译成地址的系统。',
				'每当你打开一个网站、应用或视频，设备都会在后台进行一次或多次 DNS 查询。如果 DNS 很慢，一切都会“启动很慢”——哪怕你的实际网速本身没有问题。'
			]
		}
	},
	{
		id: 'ip',
		en: {
			title: 'IP address',
			body: [
				'The numeric address of a computer on the internet, e.g. 142.251.220.174 (IPv4) or 2404:6800:4003::200e (IPv6). DNS exists to find the IP address that belongs to a name.',
				'A single name can map to many IP addresses, and large sites like Google change them constantly to balance load - which is why DNS answers do not stay valid forever (see TTL).'
			]
		},
		zh: {
			title: 'IP 地址',
			body: [
				'计算机在互联网上的数字地址，例如 142.251.220.174（IPv4）或 2404:6800:4003::200e（IPv6）。DNS 的作用就是找出某个名字对应的 IP 地址。',
				'一个名字可以对应多个 IP 地址，像 Google 这样的大型网站会不断更换地址以均衡负载——这也是 DNS 答案不会永远有效的原因（见 TTL）。'
			]
		}
	},
	{
		id: 'ttl',
		en: {
			title: 'TTL (Time To Live)',
			body: [
				'A number that comes with every DNS answer, measured in seconds. It means "you may reuse this answer for this many seconds, then ask again." A record with TTL=300 may be cached for 5 minutes; TTL=1800 for 30 minutes.',
				'The domain owner sets the TTL. Short TTLs let them move to a new address quickly (useful for failover); long TTLs reduce how often everyone has to ask. photondns respects the TTL, and can raise very short ones to a floor value (the "Minimum TTL" setting) so records do not expire absurdly fast.'
			]
		},
		zh: {
			title: 'TTL（生存时间）',
			body: [
				'每个 DNS 答案都会附带的一个数字，单位为秒。含义是“你可以重复使用这个答案这么多秒，之后需要重新查询”。TTL=300 的记录可缓存 5 分钟；TTL=1800 可缓存 30 分钟。',
				'TTL 由域名所有者设定。较短的 TTL 让他们能快速切换到新地址（便于故障切换）；较长的 TTL 则减少大家重复查询的次数。photondns 会遵守 TTL，并可把过短的 TTL 提升到一个下限值（“最小 TTL”设置），避免记录过快过期。'
			]
		}
	},
	{
		id: 'cache',
		en: {
			title: 'Cache',
			body: [
				'photondns\' memory of recent answers, kept in RAM. The first time any device asks for a name, photondns queries an upstream server (this can take ~200 ms over an encrypted link abroad) and stores the result. Every device that asks afterwards gets the stored answer in about 1-2 ms.',
				'A high cache hit-rate is what makes browsing feel instant. The cache can also be saved to disk so it survives a restart (see "Persist cache to disk").'
			]
		},
		zh: {
			title: '缓存',
			body: [
				'photondns 对近期答案的“记忆”，保存在内存中。任何设备第一次查询某个名字时，photondns 会向上游服务器查询（经境外加密链路可能耗时约 200 毫秒）并保存结果。之后再查询的设备都能在约 1–2 毫秒内拿到已保存的答案。',
				'较高的缓存命中率正是让上网“瞬间响应”的原因。缓存还可以保存到磁盘，以便在重启后依然可用（见“持久化缓存到磁盘”）。'
			]
		}
	},
	{
		id: 'fresh',
		en: {
			title: 'Fresh vs. stale',
			body: [
				'A cached answer is FRESH while it is still inside its TTL - it can be reused with no hesitation.',
				'Once the TTL runs out, the answer becomes STALE: expired, but usually still correct (a server rarely changes its address the instant its TTL hits zero). Rather than delete it, photondns can keep serving the stale answer instantly while quietly fetching a fresh copy in the background. This is the single biggest reason things stay fast here - the user never waits ~200 ms for the slow upstream, they get the stale answer in ~0 ms and the refresh happens invisibly.',
				'If an answer is not asked for again for a very long time (past the "Stale lifetime"), it is finally dropped, and the next request must wait for a full fresh lookup.'
			]
		},
		zh: {
			title: '新鲜 与 过期（stale）',
			body: [
				'缓存的答案在 TTL 期限内是“新鲜（FRESH）”的——可以毫不犹豫地重复使用。',
				'一旦 TTL 到期，答案就变成“过期（STALE）”：已失效，但通常仍然正确（服务器很少会在 TTL 归零的一瞬间就更换地址）。photondns 不会立刻删除它，而是可以继续立即返回这个过期答案，同时在后台悄悄获取新副本。这正是这里能保持流畅的最大原因——用户无需为缓慢的上游等待约 200 毫秒，而是在约 0 毫秒内拿到过期答案，刷新则在后台无感完成。',
				'如果某个答案在很长时间内（超过“过期保留时长”）都没有再被查询，它最终会被丢弃，下次请求就必须等待一次完整的全新查询。'
			]
		}
	},
	{
		id: 'staleclientttl',
		en: {
			title: 'Stale answer TTL',
			body: [
				'When photondns hands out a stale answer, it stamps a small TTL on it so the asking device caches it for a little while instead of asking again on the very next connection. The default is 30 seconds (recommended by the DNS serve-stale standard, RFC 8767).',
				'Too low (e.g. 1 second) and devices that strictly honour TTL - Apple TV and iPhones especially - re-ask constantly, hammering the resolver. Too high and a device might keep using an old address for a bit longer after it has changed. 30 is a good balance for a home network.'
			]
		},
		zh: {
			title: '过期答案 TTL',
			body: [
				'当 photondns 返回一个过期答案时，会为其标记一个较小的 TTL，让查询的设备把它缓存一小段时间，而不是在下一次连接时立即重新查询。默认值为 30 秒（DNS serve-stale 标准 RFC 8767 的推荐值）。',
				'设得太低（如 1 秒），严格遵守 TTL 的设备——尤其是 Apple TV 和 iPhone——会不停地重新查询，给解析器造成很大压力。设得太高，设备在地址已经变更后可能还会继续使用旧地址一小段时间。对家庭网络而言，30 是一个良好的折中值。'
			]
		}
	},
	{
		id: 'upstream',
		en: {
			title: 'Upstream & groups',
			body: [
				'An "upstream" is a DNS server photondns forwards questions to when the answer is not already cached. photondns organises upstreams into groups: a "main" group for global names and a "local" group for domestic ones.',
				'Main group upstreams are typically encrypted (see DoT/DoH) and reached over the international link, so they are slower (~200 ms) but private and trustworthy. The local group is a fast nearby server (a few ms) used for domestic sites. photondns picks the right group per name automatically.'
			]
		},
		zh: {
			title: '上游 与 分组',
			body: [
				'“上游”是指当答案尚未缓存时，photondns 转发查询所用的 DNS 服务器。photondns 会把上游分成若干组：处理全球域名的“main（主）”组，以及处理国内域名的“local（本地）”组。',
				'主组的上游通常是加密的（见 DoT/DoH），经国际链路访问，因此较慢（约 200 毫秒），但私密且可信。本地组是附近的快速服务器（几毫秒），用于国内网站。photondns 会为每个名字自动选择合适的分组。'
			]
		}
	},
	{
		id: 'dotdoh',
		en: {
			title: 'DoT / DoH (encrypted DNS)',
			body: [
				'Plain DNS is unencrypted - anyone on the path can see, and even tamper with, what you look up. DoT (DNS over TLS) and DoH (DNS over HTTPS) wrap DNS in encryption so it is private and cannot be silently altered.',
				'photondns uses these for the main group (e.g. tls://1.1.1.1 for DoT, https://dns.google/dns-query for DoH). They cost a little more time than plain DNS but protect your lookups. "Bootstrap DNS" is a plain server used only once at startup to find the address of these encrypted servers themselves.'
			]
		},
		zh: {
			title: 'DoT / DoH（加密 DNS）',
			body: [
				'普通 DNS 是不加密的——路径上的任何人都能看到、甚至篡改你所查询的内容。DoT（DNS over TLS）和 DoH（DNS over HTTPS）用加密封装 DNS，使其私密且无法被悄悄篡改。',
				'photondns 在主组中使用它们（例如 DoT 的 tls://1.1.1.1，DoH 的 https://dns.google/dns-query）。它们比普通 DNS 稍慢，但能保护你的查询。“Bootstrap DNS（引导 DNS）”是一台普通服务器，仅在启动时使用一次，用于找出这些加密服务器本身的地址。'
			]
		}
	},
	{
		id: 'prefetch',
		en: {
			title: 'Prefetch',
			body: [
				'A trick to avoid ever going stale for popular names. When a frequently-used entry is about to expire, photondns refreshes it in advance - so the next time anyone asks, a fresh answer is already waiting and nobody pays the upstream delay.',
				'Prefetch only helps names that are asked for often and continuously. Names queried in rare bursts (like advertising domains between video ads) go fully cold in between and cannot be prefetched.'
			]
		},
		zh: {
			title: '预取（Prefetch）',
			body: [
				'一种让热门名字永不过期的技巧。当某个高频条目即将到期时，photondns 会提前刷新它——这样下次有人查询时，新鲜答案已经就位，无需承担上游延迟。',
				'预取只对被频繁且持续查询的名字有效。对于偶尔成批查询的名字（例如视频广告之间的广告域名），它们在间隔期会完全变冷，无法被预取。'
			]
		}
	},
	{
		id: 'strategy',
		en: {
			title: 'Strategy (race / parallel / ...)',
			body: [
				'The strategy decides how a group uses its upstreams for one lookup. It does NOT change what answer you get - only how fast, and how much duplicate query traffic is sent.',
				'race (default): ask the fastest upstream first; only if it has not replied within a short adaptive delay, fire the next one too. Fewest duplicate queries, first good answer wins.',
				'parallel: query the fastest upstreams effectively at the same instant and take whichever replies first. It races up to 3 upstreams (not literally every one you configured, and backups do not join in). Lowest latency and instant failover, but it sends the same lookup to several providers at once - roughly multiplying that query traffic, and each provider sees the query. Good for a jittery international link.',
				'fastest / sequential / random exist too: fastest and sequential try one at a time (failing over on error), random shuffles the order. If unsure, leave it on race; use parallel when the link is unreliable and you want the quickest possible answer.'
			]
		},
		zh: {
			title: '策略（race / parallel 等）',
			body: [
				'策略决定一个分组在单次查询中如何使用其上游。它不会改变你得到的答案，只影响快慢，以及发出多少重复查询流量。',
				'race（竞速，默认）：先询问最快的上游；只有当它在一个较短的自适应延迟内仍未回复时，才再发起下一个。重复查询最少，第一个有效答案胜出。',
				'parallel（并行）：几乎在同一瞬间向最快的几个上游发起查询，谁先回复就用谁。它最多并行 3 个上游（并非你配置的每一个，且备用上游不参与）。延迟最低、故障切换即时，但会把同一次查询同时发给多个提供商——大致成倍增加该部分查询流量，且每个提供商都会看到该查询。适合抖动较大的国际链路。',
				'此外还有 fastest / sequential / random：fastest 与 sequential 每次只尝试一个（出错时切换），random 打乱顺序。若不确定，保持 race 即可；当链路不稳定且希望尽快拿到答案时，使用 parallel。'
			]
		}
	},
	{
		id: 'hedge',
		en: {
			title: 'Hedging & hedge delay',
			body: [
				'"Hedging" is the mechanism behind the strategies above: if the current upstream has not answered within the hedge delay, photondns fires the next candidate in parallel and takes the first good reply. A dead upstream therefore costs only one hedge delay, not a full timeout - which is what makes failover feel free.',
				'The "Hedge delay" setting is the longest it will wait before hedging under the race strategy (it also adapts down to ~2x the fastest upstream\'s recent latency). Under parallel the delay is effectively zero (fire at once); under sequential it waits a full attempt.'
			]
		},
		zh: {
			title: '对冲 与 对冲延迟',
			body: [
				'“对冲（Hedging）”是上述策略背后的机制：如果当前上游在对冲延迟内没有回答，photondns 就并行发起下一个候选，并采用第一个有效回复。因此一个失效的上游只需付出一个对冲延迟的代价，而非一整个超时——这正是让故障切换“几乎无感”的原因。',
				'“对冲延迟”设置是在 race 策略下发起对冲前的最长等待时间（它还会自适应下调至约为最快上游近期延迟的 2 倍）。在 parallel 下该延迟实际为零（同时发起）；在 sequential 下则会等待一整次尝试。'
			]
		}
	},
	{
		id: 'failover',
		en: {
			title: 'Failover & health checks',
			body: [
				'photondns continuously probes each upstream in the background. If one starts failing (too slow, or not answering), it is marked "down" and taken out of rotation; when it recovers, it is brought back. This is "failover" - keeping DNS working by routing around broken upstreams automatically.',
				'The Status page shows each upstream\'s health and its recent response time (EWMA, a rolling average).'
			]
		},
		zh: {
			title: '故障切换 与 健康检查',
			body: [
				'photondns 会在后台持续探测每个上游。如果某个上游开始出问题（过慢，或无响应），它会被标记为“下线（down）”并移出轮换；恢复后再重新加入。这就是“故障切换”——通过自动绕开故障上游来保持 DNS 正常工作。',
				'状态页会显示每个上游的健康状况及其近期响应时间（EWMA，一种滚动平均值）。'
			]
		}
	},
	{
		id: 'servestale_persist',
		en: {
			title: 'Persist cache to disk',
			body: [
				'The cache normally lives only in RAM, so a restart or reboot would start with an empty (cold) cache and every name would be slow again until it re-warms. To avoid that, photondns can save the cache to a file and reload it on startup.',
				'"Cache save interval" controls how often it writes: set to 0, it saves only when the service shuts down cleanly (gentlest on flash storage); a positive value also writes every N seconds. On a device with flash/SSD storage, 0 avoids needless write wear, at the cost of losing the cache if power is cut abruptly.'
			]
		},
		zh: {
			title: '持久化缓存到磁盘',
			body: [
				'缓存通常只存在于内存中，因此重启或断电后会以空的（冷）缓存启动，每个名字在重新预热前都会再次变慢。为避免这种情况，photondns 可以把缓存保存到文件，并在启动时重新加载。',
				'“缓存保存间隔”控制写入频率：设为 0 时，仅在服务正常关闭时保存（对闪存最友好）；设为正值则还会每 N 秒写入一次。在使用闪存/SSD 的设备上，设为 0 可避免不必要的写入损耗，代价是突然断电时会丢失缓存。'
			]
		}
	},
	{
		id: 'block',
		en: {
			title: 'Block list & ad blocking',
			body: [
				'photondns can refuse to resolve certain names (answering "does not exist"), which is how DNS-based ad/tracker blocking works. This is OPTIONAL and off by default - the "Ad blocking" switch and the block list are both empty unless you enable them.',
				'If a name is blocked, apps that expect it may retry and appear to hang. If you are NOT blocking ads and something is still slow, the cause is elsewhere (usually a cold cache miss over the slow link, not blocking).'
			]
		},
		zh: {
			title: '拦截列表 与 广告拦截',
			body: [
				'photondns 可以拒绝解析某些名字（返回“不存在”），这正是基于 DNS 的广告/追踪器拦截的工作方式。此功能为可选，且默认关闭——除非你启用，否则“广告拦截”开关和拦截列表都是空的。',
				'如果某个名字被拦截，依赖它的应用可能会不断重试并看似卡住。如果你并未拦截广告但仍然很慢，原因就在别处（通常是经慢速链路的冷缓存未命中，而非拦截）。'
			]
		}
	}
];

/* Short intro shown above the glossary. */
var INTRO = {
	en: 'A plain-language guide to how photondns works and the terms used across these settings pages. Use the button to switch between English and 中文.',
	zh: 'photondns 工作原理及各设置页面所用术语的通俗说明。使用按钮可在 English 与中文之间切换。'
};

return view.extend({
	load: function () {
		return Promise.resolve();
	},

	render: function () {
		var self = this;
		var lang = 'en';

		var intro = E('p', { style: 'color:#666; margin:0 0 14px 0' }, INTRO[lang]);
		var container = E('div', {});

		function card(section) {
			var s = section[lang];
			var paras = s.body.map(function (t) {
				return E('p', { style: 'margin:6px 0; line-height:1.55' }, t);
			});
			return E('div', {
				style: 'border:1px solid rgba(128,128,128,.28); border-radius:8px; ' +
					'padding:12px 16px; margin:10px 0; background:rgba(128,128,128,.04)'
			}, [
				E('h3', { style: 'margin:0 0 6px 0' }, s.title)
			].concat(paras));
		}

		function repaint() {
			intro.textContent = INTRO[lang];
			container.innerHTML = '';
			HELP.forEach(function (section) {
				container.appendChild(card(section));
			});
		}

		function mkLangBtn(code, label) {
			return E('button', {
				class: 'btn',
				style: 'margin-right:6px',
				'data-code': code,
				click: function (ev) {
					lang = code;
					// reflect active state on both buttons
					Array.prototype.forEach.call(
						ev.target.parentNode.querySelectorAll('button'),
						function (b) {
							b.classList.toggle('cbi-button-positive', b.getAttribute('data-code') === lang);
						}
					);
					repaint();
				}
			}, label);
		}

		var enBtn = mkLangBtn('en', 'English');
		var zhBtn = mkLangBtn('zh', '中文');
		enBtn.classList.add('cbi-button-positive');

		var toolbar = E('div', { style: 'margin:0 0 8px 0' }, [
			E('span', { style: 'margin-right:10px; color:#888' }, '📖'),
			enBtn, zhBtn
		]);

		repaint();

		return E('div', { class: 'cbi-map' }, [
			E('h2', {}, 'photondns — Help & Glossary'),
			toolbar,
			intro,
			container
		]);
	},

	handleSave: null,
	handleSaveApply: null,
	handleReset: null
});
