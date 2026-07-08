#!/usr/bin/env bash
# photondns vs mosdns — local DNS benchmark with network delay removed.
#
#  * Network removed: both forward to the instant loopback stub (:5300); a
#    query's time is the resolver's own processing, not WAN RTT.
#  * Fair: identical query files/lists/cache(65536)/upstream; match-first mosdns
#    mirrors photondns router.decide()-before-cache; 8 worker threads each; ONE
#    resolver measured at a time; same dnsperf settings.
#  * Honest: the dnsperf->stub ceiling is printed; scenarios at the ceiling are
#    flagged generator-bound and compared by fixed-load latency instead.
#
# Two metrics per scenario:
#   MAX      flat-out throughput (median of REPEATS), + tail max + lost
#   FIXED    latency at a shared sub-saturation OFFERED qps (clean per-query cost)
set -u
cd "$(dirname "$0")"
PD_BIN="../target/release/photondns"; MD_BIN="./mosdns"; STUB_BIN="./stubdns"

REPEATS="${REPEATS:-6}"; MAX_SECS="${MAX_SECS:-6}"
FIXED_REP="${FIXED_REP:-3}"; FIXED_SECS="${FIXED_SECS:-10}"; OFFERED="${OFFERED:-25000}"
COOL="${COOL:-3}"
GEN_C=20; GEN_T=4
mkdir -p results; MAXCSV=results/max_throughput.csv; FIXCSV=results/fixed_load.csv
echo "label,repeat,qps,avg_ms,max_ms,lost,noerror,nxdomain" >"$MAXCSV"
echo "label,repeat,offered,qps,avg_ms,max_ms,lost,noerror,nxdomain" >"$FIXCSV"

log(){ printf '\033[36m# %s\033[0m\n' "$*"; }

# ---------- daemon management -------------------------------------------------
stop_all(){ pkill -f '[d]nsperf' 2>/dev/null; pkill -f 'release/[p]hotondns' 2>/dev/null
            pkill -f '[m]osdns start' 2>/dev/null; pkill -f '[s]tubdns' 2>/dev/null; sleep 1; }
start_stub(){ pkill -f '[s]tubdns' 2>/dev/null; sleep 0.3; nohup "$STUB_BIN" 127.0.0.1:5300 >/tmp/stubdns.log 2>&1 & sleep 0.5; }
start_pd(){ pkill -f 'release/[p]hotondns' 2>/dev/null; sleep 0.3; nohup "$PD_BIN" -c conf/photondns.toml >/tmp/pd.log 2>&1 & sleep 1.2; }
start_md(){ pkill -f '[m]osdns start' 2>/dev/null; sleep 0.3; GOMAXPROCS=8 nohup "$MD_BIN" start -c conf/mosdns.yaml -d . >/tmp/md.log 2>&1 & sleep 1.5; }
start_res(){ [ "$1" = pd ] && start_pd || start_md; }
port_of(){ [ "$1" = pd ] && echo 5353 || echo 5354; }

# ---------- dnsperf output parsing (portable; BSD awk safe) -------------------
val_qps(){ grep 'Queries per second' "$1" | grep -oE '[0-9]+\.[0-9]+' | head -1; }
val_avg(){ grep 'Average Latency' "$1" | grep -oE '[0-9]+\.[0-9]+' | head -1; }
val_max(){ grep 'Average Latency' "$1" | grep -oE '[0-9]+\.[0-9]+' | sed -n '3p'; }
val_lost(){ grep 'Queries lost' "$1" | grep -oE '[0-9]+' | head -1; }
val_ne(){ grep 'Response codes' "$1" | sed -n 's/.*NOERROR \([0-9]*\).*/\1/p'; }
val_nx(){ grep 'Response codes' "$1" | sed -n 's/.*NXDOMAIN \([0-9]*\).*/\1/p'; }
row(){ # file -> "qps avg_ms max_ms lost ne nx"
  local f=$1; awk -v q="$(val_qps "$f")" -v a="$(val_avg "$f")" -v m="$(val_max "$f")" \
    -v lo="$(val_lost "$f")" -v ne="$(val_ne "$f")" -v nx="$(val_nx "$f")" \
    'BEGIN{printf "%.0f %.3f %.2f %d %d %d", q+0, (a+0)*1000, (m+0)*1000, lo+0, ne+0, nx+0}'; }

# ---------- one measurement ---------------------------------------------------
# max_run: flat-out. args: res label queryfile [dnsperf extra...]
max_run(){ local res=$1 label=$2 qf=$3; shift 3; local port; port=$(port_of "$res")
  for r in $(seq 1 "$REPEATS"); do
    [ -n "${COLD:-}" ] && { start_res "$res"; dig @127.0.0.1 -p "$port" +short +time=2 +tries=1 warm.check A >/dev/null 2>&1; }
    local f="results/${label}.r${r}.txt"
    dnsperf -s 127.0.0.1 -p "$port" -d "$qf" -c "$GEN_C" -T "$GEN_T" "$@" >"$f" 2>&1
    read qps a m lo ne nx <<<"$(row "$f")"
    echo "$label,$r,$qps,$a,$m,$lo,$ne,$nx" >>"$MAXCSV"
    printf "    %-22s r%d  QPS=%-8s avg=%6sms max=%8sms lost=%-4s NE=%-8s NX=%s\n" "$label" "$r" "$qps" "$a" "$m" "$lo" "$ne" "$nx"
    sleep "$COOL"
  done; }
# fixed_run: at OFFERED qps. args: res label queryfile [extra...]
fixed_run(){ local res=$1 label=$2 qf=$3; shift 3; local port; port=$(port_of "$res")
  for r in $(seq 1 "$FIXED_REP"); do
    [ -n "${COLD:-}" ] && { start_res "$res"; dig @127.0.0.1 -p "$port" +short +time=2 +tries=1 warm.check A >/dev/null 2>&1; }
    local f="results/${label}.fx${r}.txt"
    dnsperf -s 127.0.0.1 -p "$port" -d "$qf" -c "$GEN_C" -T "$GEN_T" -Q "$OFFERED" -l "$FIXED_SECS" "$@" >"$f" 2>&1
    read qps a m lo ne nx <<<"$(row "$f")"
    echo "$label,$r,$OFFERED,$qps,$a,$m,$lo,$ne,$nx" >>"$FIXCSV"
    printf "    %-22s fx%d QPS=%-8s avg=%6sms max=%8sms lost=%-4s\n" "$label" "$r" "$qps" "$a" "$m" "$lo"
    sleep "$COOL"
  done; }
warm(){ local res=$1 qf=$2 port; port=$(port_of "$res"); dnsperf -s 127.0.0.1 -p "$port" -d "$qf" -c 8 -T "$GEN_T" -n 1 >/dev/null 2>&1; }

# ---------- ceiling -----------------------------------------------------------
stop_all; start_stub
log "cores=$(sysctl -n hw.ncpu) THREADS=8 GEN=-c${GEN_C}-T${GEN_T} REPEATS=$REPEATS MAX_SECS=${MAX_SECS} OFFERED=$OFFERED"
log "CEILING dnsperf->stub (any resolver near this is generator-bound, not slow):"
for r in 1 2 3; do dnsperf -s 127.0.0.1 -p 5300 -d queries/hit_10k.txt -c "$GEN_C" -T "$GEN_T" -l "$MAX_SECS" -n 1000000 >results/ceiling.r$r.txt 2>&1
  read qps a m lo ne nx <<<"$(row results/ceiling.r$r.txt)"; echo "ceiling,$r,$qps,$a,$m,$lo,$ne,$nx" >>"$MAXCSV"
  printf "    ceiling               r%d  QPS=%-8s avg=%6sms max=%8sms lost=%s\n" "$r" "$qps" "$a" "$m" "$lo"; done

# ---------- MAX throughput (flat-out) ----------------------------------------
start_pd; start_md
log "MAX THROUGHPUT (flat-out, median of $REPEATS) — interleaved A/B"
for res in pd md; do warm "$res" queries/hit_10k.txt; done
COLD=""; for res in pd md; do max_run "$res" "${res}.hit"      queries/hit_10k.txt        -l "$MAX_SECS" -n 1000000; done
COLD=""; for res in pd md; do max_run "$res" "${res}.ad_block" queries/ad_block_50k.txt   -l "$MAX_SECS" -n 100; done
COLD=1;  for res in pd md; do max_run "$res" "${res}.foreign_miss" queries/foreign_miss_2m.txt -l "$MAX_SECS"; done
COLD=1;  for res in pd md; do max_run "$res" "${res}.china_miss"   queries/china_miss_2m.txt   -l "$MAX_SECS"; done
COLD=""; start_pd; start_md; for res in pd md; do warm "$res" queries/mixed.txt; done
         for res in pd md; do max_run "$res" "${res}.mixed"    queries/mixed.txt          -l "$MAX_SECS" -n 1000000; done

# ---------- FIXED-load latency (sub-saturation) ------------------------------
log "FIXED LOAD latency @ ${OFFERED} qps (median of $FIXED_REP) — clean per-query cost"
start_pd; start_md; for res in pd md; do warm "$res" queries/hit_10k.txt; done
COLD=""; for res in pd md; do fixed_run "$res" "${res}.hit"      queries/hit_10k.txt      -n 1000000; done
COLD=""; for res in pd md; do fixed_run "$res" "${res}.ad_block" queries/ad_block_50k.txt -n 1000; done
COLD=1;  for res in pd md; do fixed_run "$res" "${res}.foreign_miss" queries/foreign_miss_2m.txt; done
COLD=1;  for res in pd md; do fixed_run "$res" "${res}.china_miss"   queries/china_miss_2m.txt; done
COLD=""; start_pd; start_md; for res in pd md; do warm "$res" queries/mixed.txt; done
         for res in pd md; do fixed_run "$res" "${res}.mixed"    queries/mixed.txt        -n 1000000; done

# ---------- tail latency (per-query capture at OFFERED, mixed) ----------------
log "TAIL latency p50/p90/p99/p999 @ ${OFFERED} qps (mixed, -v capture)"
for res in pd md; do port=$(port_of "$res"); start_res "$res"; warm "$res" queries/mixed.txt
  dnsperf -s 127.0.0.1 -p "$port" -d queries/mixed.txt -c "$GEN_C" -T "$GEN_T" -Q "$OFFERED" -l "$FIXED_SECS" -v >results/${res}.tail.raw 2>&1
  grep -oE '[0-9]+\.[0-9]+$' results/${res}.tail.raw | sort -n >results/${res}.tail.sorted
  awk '{r[NR]=$1} END{n=NR; if(!n){print "    no samples"; exit} printf "    %-9s n=%d  p50=%.3fms p90=%.3fms p99=%.3fms p999=%.3fms max=%.3fms\n","'"$res"'",n,r[int(n*.5)]*1000,r[int(n*.9)]*1000,r[int(n*.99)]*1000,r[int(n*.999)]*1000,r[n]*1000}' results/${res}.tail.sorted
  sleep "$COOL"
done

pkill -f '[d]nsperf' 2>/dev/null
log "raw CSVs: $MAXCSV , $FIXCSV  (medians computed by summarize.sh)"
