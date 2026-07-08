export const meta = {
  name: 'bench-verify',
  description: 'Adversarially verify the photondns-vs-mosdns benchmark: fairness, network-removal, validity',
  phases: [
    { title: 'Attack', detail: '4 skeptics attack fairness / network-removal / validity / anomalies' },
    { title: 'Adjudicate', detail: 'synthesize surviving issues into a verified verdict + caveats' },
  ],
}

const CTX = `
A local DNS benchmark comparing photondns (Rust, the project under test) vs mosdns v5.3.4 (Go) was run on ONE macOS arm64 host (10 cores: 4P+6E), everything on loopback, to measure each resolver's OWN processing with WAN/network delay removed.

ALL artifacts are on disk under /Users/c2h2/gits/luci-app-photondns/bench/ — READ them directly and be concrete:
- conf/photondns.toml, conf/mosdns.yaml   (the two configs actually run)
- run.sh                                   (the runner)
- lists/china_list.txt (111,712), lists/ad_list.txt (105,793)
- queries/*.txt                            (dnsperf query files)
- stubdns.go                               (the zero-latency stub upstream)
- results/max_throughput.csv, results/fixed_load.csv, results/summary.md, results/*.tail.sorted
- results/*.r*.txt, results/*.fx*.txt      (raw dnsperf outputs per run)
- /tmp/pd.log /tmp/md.log /tmp/stubdns.log
Source of truth for photondns behavior: /Users/c2h2/gits/luci-app-photondns/src/router.rs (decide()) and src/server.rs (handle_query: router.decide() runs BEFORE the cache lookup on every query).

METHOD SUMMARY: both resolvers forward to stubdns on udp://127.0.0.1:5300 (instant, answers every A with one record, TTL60). Both load china_list (routing) + ad_list (NXDOMAIN block), cache=plain LRU 65536, 8 worker threads (mosdns GOMAXPROCS=8; photondns auto-caps min(cores,8)=8). mosdns is MATCH-FIRST (ad-match + china-match BEFORE cache via goto) to mirror photondns running decide() before cache on every query incl. hits. Metrics: MAX flat-out throughput (median of repeats) with the dnsperf->stub ceiling (~125k QPS) disclosed; FIXED-load latency at 25k qps (sub-saturation); tail p50/p90/p99/p999. Headline finding: on the forwarding paths (foreign_miss, china_miss — resolver-bound) photondns shows ~1.3-1.7x the throughput and far tighter tail latency (mosdns max ~1-2s, suspected Go GC), while cache-hit and ad-block are generator-bound (~ceiling) so both look equal there.

You may RE-RUN spot checks yourself (dnsperf is installed; daemons may need starting — see run.sh helpers), Read any file, and WebFetch mosdns docs. The machine should be quiet now; if you run load, do ONE short test at a time and do not run both resolvers under load simultaneously.
`

phase('Attack')
const LENSES = [
  { key: 'fairness', role: 'FAIRNESS AUDITOR', ask: 'Is the comparison fair, or does photondns get a structural advantage (or disadvantage)? Scrutinize: does mosdns.yaml do provably-equivalent work to photondns router.decide()->cache->forward? Is match-first correct given server.rs runs decide() before cache on EVERY query including hits (verify this in source)? Are cache semantics equal (size, no lazy/prefetch/serve-stale on either, NXDOMAIN uncached on both)? Are both really at 8 threads? Same upstream, same query files, same dnsperf flags? Any place mosdns is handicapped by a misconfig (e.g. wrong reject rcode, double-forward, concurrent, extra logging, TCP vs UDP)? Read both configs line by line and the raw result files. Report concrete unfairnesses with file:line and whether they favor pd or md.' },
  { key: 'network', role: 'NETWORK-REMOVAL AUDITOR', ask: 'Is WAN/network delay actually removed, and is the stub neutral? Verify stubdns is truly instant and identical for both (read stubdns.go; check it is not a per-resolver bottleneck; both forward to the same :5300). Is the "generator-bound / ceiling" reasoning sound — i.e. for cache-hit/ad-block is the ~125k a real dnsperf+loopback ceiling (so those scenarios genuinely cannot differentiate throughput)? Could loopback/socket-buffer behavior favor one resolver (photondns sets 1MB SO_RCVBUF, mosdns OS default)? Are the miss scenarios genuinely resolver-bound (well below ceiling, lost==0)? Confirm the "network removed" claim is honestly supported and correctly caveated.' },
  { key: 'validity', role: 'MEASUREMENT-VALIDITY AUDITOR', ask: 'Are the numbers statistically trustworthy? Check results/max_throughput.csv and fixed_load.csv: run-to-run variance/IQR, any lost>0 runs (which poison latency), whether repeats are enough to claim the gap, whether warmup/cold-cache handling is correct (miss scenarios restart for cold cache; hit scenarios warmed). Could core oversubscription (8 resolver + 4 dnsperf + stub on 10 cores) or P/E scheduling explain the throughput gap rather than real resolver speed? Is the median aggregation sound? Recompute medians from the CSVs yourself and confirm summary.md matches. Flag any run that should be discarded.' },
  { key: 'anomaly', role: 'ANOMALY & REPRODUCTION AUDITOR', ask: 'Investigate the surprising results and find the TRUE cause. (1) mosdns shows near-EXACT ~1000/2000ms max-latency spikes on forwarding scenarios (foreign_miss, china_miss, sometimes mixed/hit), even at only 15k qps, on ~1 query per run; photondns never exceeds ~45ms. The near-integer-second value suggests a 1s FORWARD RETRY after a dropped loopback UDP packet (socket-buffer overflow), NOT Go GC (GC pauses are variable 10-300ms, not exactly 1000ms). TEST BOTH hypotheses: (a) GC — run a short mosdns miss load with GODEBUG=gctrace=1 and see if pauses align with the 1s spikes (they should NOT if it is retry); (b) socket buffer / retry — check mosdns forward defaults and whether raising UDP socket buffers (net.core / kern.ipc.maxsockbuf, or a mosdns/forward option) or changing the forward config REMOVES the spikes. CRITICAL FAIRNESS QUESTION: is this a mosdns MISCONFIG on our side (photondns sets 1MB SO_RCVBUF/SNDBUF in code; mosdns uses OS defaults) that a better mosdns config would fix — in which case the tail finding is partly our artifact and must be caveated or the config fixed — or is it inherent to mosdns UDP forwarding? Determine definitively. (2) mosdns cannot sustain even 15k qps on foreign_miss (achieves ~13.7-14.3k at -Q 15000) — is that the 1s stalls consuming dnsperf client slots, or a real throughput limit? (3) Re-run ONE short foreign_miss flat-out per resolver and confirm the ~1.4x throughput ratio and the pd~0.16ms vs md~0.6ms+ latency reproduce. Report true causes, and explicitly state whether the tail-spike finding is FAIR to attribute to mosdns as-shipped or needs a config fix/caveat.' },
]

const VERDICT = {
  type:'object', additionalProperties:false,
  required:['lens','verdict','issues','confirmed','recommended_caveats'],
  properties:{
    lens:{type:'string'},
    verdict:{type:'string', enum:['sound','sound-with-caveats','flawed'], description:'overall judgment for this lens'},
    issues:{type:'array', items:{type:'object', additionalProperties:false,
      required:['severity','claim','evidence','favors'],
      properties:{
        severity:{type:'string', enum:['blocker','major','minor']},
        claim:{type:'string'}, evidence:{type:'string', description:'file:line or measured numbers'},
        favors:{type:'string', enum:['photondns','mosdns','neither'], description:'which side the issue would unfairly favor, if any'},
      }}},
    confirmed:{type:'array', items:{type:'string'}, description:'things you positively verified as correct/fair'},
    recommended_caveats:{type:'array', items:{type:'string'}, description:'caveats the writeup MUST state'},
  },
}

const attacks = await parallel(LENSES.map(L => () =>
  agent(`You are a skeptical ${L.role} reviewing a DNS benchmark for publication. Assume it is flawed until proven otherwise; find the flaws.\n\n${L.ask}\n\nCONTEXT:\n${CTX}\n\nBe concrete and evidence-based (cite file:line or measured numbers). Do NOT rubber-stamp. Return the structured verdict.`,
    { label:`verify:${L.key}`, phase:'Attack', schema:VERDICT, effort:'high' })
))
const got = attacks.filter(Boolean)
log(`verification: ${got.length}/4 lenses reported`)

phase('Adjudicate')
const FINAL = {
  type:'object', additionalProperties:false,
  required:['overall_verdict','headline_is_trustworthy','surviving_issues','required_caveats','corrections_needed','confidence'],
  properties:{
    overall_verdict:{type:'string', enum:['sound','sound-with-caveats','flawed']},
    headline_is_trustworthy:{type:'boolean', description:'is "photondns ~1.3-1.7x throughput + tighter tails on forwarding paths, tie (generator-bound) on cache-hit/block" defensible?'},
    surviving_issues:{type:'array', items:{type:'object', additionalProperties:false,
      required:['severity','issue','favors','disposition'],
      properties:{severity:{type:'string'}, issue:{type:'string'}, favors:{type:'string'},
        disposition:{type:'string', description:'fix before publishing / caveat in writeup / dismissed-because'}}}},
    required_caveats:{type:'array', items:{type:'string'}},
    corrections_needed:{type:'array', items:{type:'string'}, description:'concrete changes to configs/runner/writeup, or [] if none'},
    confidence:{type:'string', enum:['high','medium','low']},
  },
}
const final = await agent(
  `You are the adjudicator. ${got.length} adversarial auditors reviewed the benchmark. Merge their findings: keep only issues that are real and evidenced, dismiss the rest with reason, decide whether the headline conclusion is trustworthy, and list the exact caveats the writeup must carry and any corrections needed before publishing.\n\nAUDITOR REPORTS (JSON):\n${JSON.stringify(got)}\n\nCONTEXT:\n${CTX}\n\nReturn the structured object.`,
  { label:'adjudicate', phase:'Adjudicate', schema:FINAL, effort:'high' })

return { attacks: got, final }
