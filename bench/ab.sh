#!/usr/bin/env bash
# A/B micro-harness for photondns optimization work. Measures the two
# resolver-bound (forwarding) scenarios flat-out against the DROP-PROOF
# hardened stub, cold cache each run, median QPS over REP runs. Network is
# removed (loopback stub answers in microseconds), so QPS ~= 1/(per-query CPU).
#
#   ./ab.sh <tag>     # e.g. ./ab.sh baseline   ./ab.sh opt
set -u; cd "$(dirname "$0")"
TAG="${1:-run}"; REP="${REP:-5}"; SECS="${SECS:-6}"
PD=../target/release/photondns
OUT=results/ab_${TAG}.csv; echo "tag,scen,rep,qps,avg_ms,max_ms,lost" >"$OUT"

start_pd(){ pkill -f 'release/[p]hotondns' 2>/dev/null; sleep .3
  nohup "$PD" -c conf/photondns.toml >/tmp/pd.log 2>&1 & sleep 1.2; }
med(){ sort -n | awk '{a[NR]=$1} END{print (NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2}'; }

pkill -f 'stubdns' 2>/dev/null; sleep .4
nohup ./stubdns_hard 127.0.0.1:5300 >/tmp/stubh.log 2>&1 & sleep .6
echo "== A/B tag=$TAG  REP=$REP SECS=$SECS  (hardened stub) =="

for scen in foreign_miss china_miss; do
  qf=queries/${scen}_2m.txt; qlist=""; alist=""
  for r in $(seq 1 $REP); do
    start_pd
    dig @127.0.0.1 -p 5353 +short +time=2 +tries=1 warm.check A >/dev/null 2>&1
    f=results/ab_${TAG}_${scen}.r${r}.txt
    dnsperf -s 127.0.0.1 -p 5353 -d "$qf" -c 20 -T 4 -l "$SECS" >"$f" 2>&1
    q=$(grep 'Queries per second' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
    a=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
    m=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|sed -n '3p')
    l=$(grep 'Queries lost' "$f"|grep -oE '[0-9]+'|head -1)
    echo "$TAG,$scen,$r,${q%.*},${a:-0},${m:-0},${l:-0}" >>"$OUT"
    qlist="$qlist ${q%.*}"; alist="$alist ${a:-0}"
    printf "  %-13s r%d  QPS=%-8s avg=%sms max=%sms lost=%s\n" "$scen" "$r" "${q%.*}" "${a:-0}" "${m:-0}" "${l:-0}"
  done
  mq=$(echo "$qlist"|tr ' ' '\n'|grep -v '^$'|med)
  ma=$(echo "$alist"|tr ' ' '\n'|grep -v '^$'|med)
  printf "  \033[1m%-13s median QPS=%-8s  median avg=%sms\033[0m\n" "$scen" "$mq" "$ma"
done
pkill -f '[d]nsperf' 2>/dev/null; pkill -f 'release/[p]hotondns' 2>/dev/null; pkill -f 'stubdns' 2>/dev/null
echo "raw -> $OUT"