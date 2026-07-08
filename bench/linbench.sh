#!/usr/bin/env bash
# Linux forwarding benchmark: photondns (per-packet baseline) vs photondns
# (recvmmsg/sendmmsg batched) vs mosdns, network removed (hardened loopback
# stub), cold cache each run, median flat-out QPS. Run on the Linux box.
set -u; cd "$(dirname "$0")"
REP="${REP:-5}"; SECS="${SECS:-6}"
PD=../target/release/photondns; MD=./mosdns
OUT=results/linbench.csv; mkdir -p results
echo "variant,scen,rep,qps,avg_ms,max_ms,lost" >"$OUT"

kill_all(){ pkill -f '[d]nsperf' 2>/dev/null; pkill -f 'release/[p]hotondns' 2>/dev/null
            pkill -f '[m]osdns start' 2>/dev/null; }
start_stub(){ pkill -f 'stubdns' 2>/dev/null; sleep .4
              nohup ./stubdns_hard 127.0.0.1:5300 >/tmp/stubh.log 2>&1 & sleep .6; }
start_pd(){ pkill -f 'release/[p]hotondns' 2>/dev/null; sleep .3
            PHOTONDNS_UDP_BATCH="$1" nohup "$PD" -c conf/photondns.toml >/tmp/pd.log 2>&1 & sleep 1.2; }
start_md(){ pkill -f '[m]osdns start' 2>/dev/null; sleep .3
            GOMAXPROCS=8 nohup "$MD" start -c conf/mosdns.yaml -d . >/tmp/md.log 2>&1 & sleep 1.5; }
med(){ sort -n | awk '{a[NR]=$1} END{print (NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2}'; }

# variant: name port starter-fn arg
run_variant(){ local name=$1 port=$2 start=$3 arg=$4
  echo "### $name (port $port) ###"
  for scen in foreign_miss china_miss; do
    local qf=queries/${scen}_2m.txt qlist="" alist=""
    for r in $(seq 1 $REP); do
      $start "$arg"
      dig @127.0.0.1 -p "$port" +short +time=2 +tries=1 warm.check A >/dev/null 2>&1
      local f=results/lb_${name}_${scen}.r${r}.txt
      dnsperf -s 127.0.0.1 -p "$port" -d "$qf" -c 20 -T 4 -l "$SECS" >"$f" 2>&1
      local q a m l
      q=$(grep 'Queries per second' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
      a=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
      m=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|sed -n '3p')
      l=$(grep 'Queries lost' "$f"|grep -oE '[0-9]+'|head -1)
      echo "$name,$scen,$r,${q%.*},${a:-0},${m:-0},${l:-0}" >>"$OUT"
      qlist="$qlist ${q%.*}"; alist="$alist ${a:-0}"
    done
    local mq ma; mq=$(echo "$qlist"|tr ' ' '\n'|grep -v '^$'|med); ma=$(echo "$alist"|tr ' ' '\n'|grep -v '^$'|med)
    printf "  %-13s medQPS=%-8s medAvg=%sms\n" "$scen" "$mq" "$ma"
  done
}

kill_all; start_stub
echo "== cores=$(nproc) REP=$REP SECS=$SECS (hardened stub) =="
run_variant pd_nobatch 5353 start_pd 0
run_variant pd_batch   5353 start_pd 1
run_variant mosdns     5354 start_md ""
kill_all
echo "raw -> $OUT"

echo "== SUMMARY (median flat-out QPS) =="
python3 - "$OUT" <<'PY'
import csv,statistics as st
rows=list(csv.DictReader(open(__import__('sys').argv[1])))
g={}
for r in rows: g.setdefault((r['variant'],r['scen']),[]).append(float(r['qps']))
def m(v,s): return st.median(g.get((v,s),[0]))
print(f"{'scenario':14} {'pd_nobatch':>11} {'pd_batch':>10} {'mosdns':>9} {'batch/nobatch':>14} {'pd_batch/md':>12}")
for s in ['foreign_miss','china_miss']:
    nb,b,md=m('pd_nobatch',s),m('pd_batch',s),m('mosdns',s)
    print(f"{s:14} {nb:>11.0f} {b:>10.0f} {md:>9.0f} {b/nb:>13.2f}x {(b/md if md else 0):>11.2f}x")
PY