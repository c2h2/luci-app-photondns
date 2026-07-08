#!/usr/bin/env bash
# Clean sub-saturation latency for the forwarding (miss) scenarios at a load
# well below mosdns's ~30k saturation, so latency reflects per-query cost, not
# queueing. Cold cache each run (restart). Both resolvers, interleaved.
set -u; cd "$(dirname "$0")"
OFFERED="${OFFERED:-15000}"; SECS="${SECS:-12}"; REP="${REP:-4}"
CSV=results/miss_latency_${OFFERED}.csv; echo "label,rep,offered,qps,avg_ms,max_ms,lost" >"$CSV"
start_pd(){ pkill -f 'release/[p]hotondns' 2>/dev/null; sleep .3; nohup ../target/release/photondns -c conf/photondns.toml >/tmp/pd.log 2>&1 & sleep 1.2; }
start_md(){ pkill -f '[m]osdns start' 2>/dev/null; sleep .3; GOMAXPROCS=8 nohup ./mosdns start -c conf/mosdns.yaml -d . >/tmp/md.log 2>&1 & sleep 1.5; }
pkill -f '[s]tubdns' 2>/dev/null; sleep .3; nohup ./stubdns 127.0.0.1:5300 >/tmp/stubdns.log 2>&1 & sleep .5
one(){ local res=$1 port=$2 label=$3 qf=$4 r=$5
  [ "$res" = pd ] && start_pd || start_md
  dig @127.0.0.1 -p "$port" +short +time=2 +tries=1 warm.check A >/dev/null 2>&1
  local f=results/${label}.lat${OFFERED}.r${r}.txt
  dnsperf -s 127.0.0.1 -p "$port" -d "$qf" -c 20 -T 4 -Q "$OFFERED" -l "$SECS" >"$f" 2>&1
  local qps avg mx lost
  qps=$(grep 'Queries per second' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
  avg=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
  mx=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|sed -n '3p')
  lost=$(grep 'Queries lost' "$f"|grep -oE '[0-9]+'|head -1)
  awk -v L="$label" -v r="$r" -v o="$OFFERED" -v q="${qps:-0}" -v a="${avg:-0}" -v m="${mx:-0}" -v lo="${lost:-0}" \
    'BEGIN{printf "%s,%d,%d,%.0f,%.3f,%.2f,%d\n",L,r,o,q,a*1000,m*1000,lo}' | tee -a "$CSV"
}
echo "== clean miss latency @ ${OFFERED} qps (sub-saturation) =="
for r in $(seq 1 $REP); do
  for pair in pd:5353 md:5354; do res=${pair%%:*}; port=${pair##*:}
    one "$res" "$port" "${res}.foreign_miss" queries/foreign_miss_2m.txt "$r"; sleep 2; done
done
for r in $(seq 1 $REP); do
  for pair in pd:5353 md:5354; do res=${pair%%:*}; port=${pair##*:}
    one "$res" "$port" "${res}.china_miss" queries/china_miss_2m.txt "$r"; sleep 2; done
done
echo "== medians =="
python3 - "$CSV" <<'PY'
import csv,statistics as st,sys
rows=list(csv.DictReader(open(sys.argv[1])))
g={}
for r in rows: g.setdefault(r['label'],{'q':[],'a':[],'m':[]}); g[r['label']]['q'].append(float(r['qps'])); g[r['label']]['a'].append(float(r['avg_ms'])); g[r['label']]['m'].append(float(r['max_ms']))
for s in ['foreign_miss','china_miss']:
  p=g['pd.'+s]; m=g['md.'+s]
  pa,ma=st.median(p['a']),st.median(m['a'])
  print(f"  {s:14} pd avg={pa:.3f}ms (q~{st.median(p['q']):.0f}) | md avg={ma:.3f}ms (q~{st.median(m['q']):.0f}) | md/pd={ma/pa:.2f}x | pd max={max(p['m']):.1f} md max={max(m['m']):.1f}")
PY