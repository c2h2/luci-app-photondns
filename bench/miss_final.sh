#!/usr/bin/env bash
# Definitive forwarding-path measurement on the HARDENED stub (drop-proof, neutral
# for both resolvers). Per (resolver, scenario): median flat-out QPS over REP cold
# runs + a -v per-query distribution (p50/p90/p99/p999, max, stall counts).
set -u; cd "$(dirname "$0")"
REP="${REP:-6}"; SECS="${SECS:-6}"
CSV=results/miss_final.csv; echo "res,scen,rep,qps,avg_ms,max_ms,lost" >"$CSV"
start_pd(){ pkill -f 'release/[p]hotondns' 2>/dev/null; sleep .3; nohup ../target/release/photondns -c conf/photondns.toml >/tmp/pd.log 2>&1 & sleep 1.2; }
start_md(){ pkill -f '[m]osdns start' 2>/dev/null; sleep .3; GOMAXPROCS=8 nohup ./mosdns start -c conf/mosdns.yaml -d . >/tmp/md.log 2>&1 & sleep 1.5; }
restart(){ [ "$1" = pd ] && start_pd || start_md; dig @127.0.0.1 -p "$2" +short +time=2 +tries=1 warm.check A >/dev/null 2>&1; }

pkill -f 'stubdns' 2>/dev/null; sleep .5; nohup ./stubdns_hard 127.0.0.1:5300 >/tmp/stubh.log 2>&1 & sleep .6
echo "== hardened stub up; REP=$REP SECS=$SECS =="

med(){ sort -n | awk '{a[NR]=$1} END{print (NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2}'; }
for scen in foreign_miss china_miss; do
  qf=queries/${scen}_2m.txt
  for res in pd md; do port=$([ "$res" = pd ] && echo 5353 || echo 5354)
    qlist=""
    for r in $(seq 1 $REP); do
      restart "$res" "$port"
      f=results/mf_${res}_${scen}.r${r}.txt
      dnsperf -s 127.0.0.1 -p "$port" -d "$qf" -c 20 -T 4 -l "$SECS" >"$f" 2>&1
      q=$(grep 'Queries per second' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
      a=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
      m=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|sed -n '3p')
      l=$(grep 'Queries lost' "$f"|grep -oE '[0-9]+'|head -1)
      echo "$res,$scen,$r,${q%.*},$(echo "${a:-0}*1000"|bc -l),$(echo "${m:-0}*1000"|bc -l),${l:-0}" >>"$CSV"
      qlist="$qlist ${q%.*}"
    done
    mq=$(echo "$qlist"|tr ' ' '\n'|grep -v '^$'|med)
    # one -v distribution run (cold)
    restart "$res" "$port"
    vf=results/mf_${res}_${scen}.vraw
    dnsperf -s 127.0.0.1 -p "$port" -d "$qf" -c 20 -T 4 -l "$SECS" -v >"$vf" 2>&1
    grep '^> ' "$vf"|grep -oE '[0-9]+\.[0-9]+$'|sort -n >results/mf_${res}_${scen}.vsorted
    read p50 p90 p99 p999 mx n o50 o500 < <(awk '{r[NR]=$1} END{n=NR;
        o50=0;o500=0; for(i=1;i<=n;i++){if(r[i]>0.05)o50++; if(r[i]>0.5)o500++}
        printf "%.3f %.3f %.3f %.3f %.1f %d %d %d",
          r[int(n*.5)]*1000,r[int(n*.9)]*1000,r[int(n*.99)]*1000,r[int(n*.999)]*1000,r[n]*1000,n,o50,o500}' results/mf_${res}_${scen}.vsorted)
    printf "  %-3s %-13s medQPS=%-7s | p50=%-6s p90=%-6s p99=%-7s p999=%-8s max=%-8s | >50ms=%-5s >500ms=%-5s (n=%s)\n" \
      "$res" "$scen" "$mq" "$p50" "$p90" "$p99" "$p999" "$mx" "$o50" "$o500" "$n"
  done
done
pkill -f '[d]nsperf' 2>/dev/null
echo "raw -> $CSV"