#!/usr/bin/env python3
# Summarize benchmark CSVs into median comparison tables (console + markdown).
import csv, statistics as st, sys, os
os.chdir(os.path.dirname(os.path.abspath(__file__)))

def load(path):
    rows=[]
    with open(path) as f:
        for r in csv.DictReader(f): rows.append(r)
    return rows

def med(xs): return st.median(xs) if xs else float('nan')

def group(rows, key):
    g={}
    for r in rows:
        g.setdefault(r['label'],[]).append(float(r[key]))
    return g

SCEN=['hit','ad_block','foreign_miss','china_miss','mixed']
NICE={'hit':'cache-hit','ad_block':'ad-block NXDOMAIN','foreign_miss':'foreign miss (fwd)',
      'china_miss':'china miss (fwd)','mixed':'mixed steady'}

maxr=load('results/max_throughput.csv')
qps=group(maxr,'qps'); avg=group(maxr,'avg_ms'); mx=group(maxr,'max_ms'); lost=group(maxr,'lost')
ceil=med(qps.get('ceiling',[0]))

def cell(label,d,agg=med):
    return d.get(label)

out=[]
out.append(f"dnsperf->stub CEILING (median): {ceil:,.0f} QPS  "
           f"(scenarios within ~10% are generator-bound, compare by latency)\n")
out.append("## MAX THROUGHPUT (flat-out, network removed) — median of repeats\n")
hdr=f"{'scenario':<20} {'photondns QPS':>14} {'mosdns QPS':>12} {'pd/md':>7}   {'pd max(ms)':>10} {'md max(ms)':>10}   bound"
out.append(hdr); out.append('-'*len(hdr))
md_rows=["| scenario | photondns QPS | mosdns QPS | pd/md | pd max ms | md max ms | note |",
         "|---|--:|--:|--:|--:|--:|---|"]
for s in SCEN:
    pq=med(qps.get('pd.'+s,[float('nan')])); mq=med(qps.get('md.'+s,[float('nan')]))
    pmx=max(mx.get('pd.'+s,[0])); mmx=max(mx.get('md.'+s,[0]))
    ratio=pq/mq if mq else float('nan')
    bound = 'GEN-bound' if max(pq,mq) >= 0.90*ceil else 'resolver'
    out.append(f"{NICE[s]:<20} {pq:>14,.0f} {mq:>12,.0f} {ratio:>6.2f}x   {pmx:>10.1f} {mmx:>10.1f}   {bound}")
    note = 'generator-bound (~ceiling)' if bound=='GEN-bound' else f'{ratio:.2f}x throughput'
    md_rows.append(f"| {NICE[s]} | {pq:,.0f} | {mq:,.0f} | {ratio:.2f}x | {pmx:.1f} | {mmx:.1f} | {note} |")

# fixed-load latency
try:
    fixr=load('results/fixed_load.csv')
    favg=group(fixr,'avg_ms'); fmx=group(fixr,'max_ms'); flost=group(fixr,'lost'); fq=group(fixr,'qps')
    off=int(float(fixr[0]['offered']))
    out.append(f"\n## FIXED-LOAD LATENCY @ {off:,} qps (sub-saturation, network removed) — median of repeats\n")
    hdr2=f"{'scenario':<20} {'pd avg(ms)':>10} {'md avg(ms)':>10} {'md/pd':>7}   {'pd max(ms)':>10} {'md max(ms)':>10}   {'pd lost':>7} {'md lost':>7}"
    out.append(hdr2); out.append('-'*len(hdr2))
    md_rows.append("\n**Latency at fixed %d qps (network removed):**\n" % off)
    md_rows.append("| scenario | pd avg ms | md avg ms | md/pd | pd max ms | md max ms |")
    md_rows.append("|---|--:|--:|--:|--:|--:|")
    for s in SCEN:
        pa=med(favg.get('pd.'+s,[float('nan')])); ma=med(favg.get('md.'+s,[float('nan')]))
        pmax=max(fmx.get('pd.'+s,[0])); mmax=max(fmx.get('md.'+s,[0]))
        pl=med(flost.get('pd.'+s,[0])); ml=med(flost.get('md.'+s,[0]))
        r=ma/pa if pa else float('nan')
        out.append(f"{NICE[s]:<20} {pa:>10.3f} {ma:>10.3f} {r:>6.2f}x   {pmax:>10.1f} {mmax:>10.1f}   {int(pl):>7} {int(ml):>7}")
        md_rows.append(f"| {NICE[s]} | {pa:.3f} | {ma:.3f} | {r:.2f}x | {pmax:.1f} | {mmax:.1f} |")
except FileNotFoundError:
    pass

print("\n".join(out))
open('results/summary.md','w').write("\n".join(md_rows)+"\n")
print("\n[markdown table -> results/summary.md]")
