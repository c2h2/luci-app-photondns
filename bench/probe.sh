#!/usr/bin/env bash
# Quick landscape probe: flat-out max QPS per scenario for both resolvers,
# plus the dnsperf->stub ceiling. Self-contained: (re)starts daemons, uses
# "$@" arg passing (no word-split reliance), parses dnsperf output from files.
set -u
cd "$(dirname "$0")"
PD_BIN="../target/release/photondns"; MD_BIN="./mosdns"; STUB_BIN="./stubdns"
mkdir -p results/probe

pkill -f '[d]nsperf' 2>/dev/null; pkill -f 'release/[p]hotondns' 2>/dev/null
pkill -f '[m]osdns start' 2>/dev/null; pkill -f '[s]tubdns' 2>/dev/null; sleep 1
nohup "$STUB_BIN" 127.0.0.1:5300 >/tmp/stubdns.log 2>&1 & sleep 0.5
nohup "$PD_BIN" -c conf/photondns.toml >/tmp/pd.log 2>&1 & sleep 1.2
GOMAXPROCS=8 nohup "$MD_BIN" start -c conf/mosdns.yaml -d . >/tmp/md.log 2>&1 & sleep 1.5

pf(){ local f=$1 qps lat mx lost ne nx
  qps=$(grep 'Queries per second' "$f" | grep -oE '[0-9]+\.[0-9]+' | head -1)
  lat=$(grep 'Average Latency' "$f" | grep -oE '[0-9]+\.[0-9]+' | head -1)
  mx=$(grep 'Average Latency' "$f" | grep -oE '[0-9]+\.[0-9]+' | sed -n '3p')
  lost=$(grep 'Queries lost' "$f" | grep -oE '[0-9]+' | head -1)
  ne=$(grep 'Response codes' "$f" | sed -n 's/.*NOERROR \([0-9]*\).*/\1/p')
  nx=$(grep 'Response codes' "$f" | sed -n 's/.*NXDOMAIN \([0-9]*\).*/\1/p')
  awk -v q="${qps:-0}" -v l="${lat:-0}" -v m="${mx:-0}" -v lo="${lost:-0}" -v ne="${ne:-0}" -v nx="${nx:-0}" \
    'BEGIN{printf "QPS=%-9.0f avg=%6.3fms max=%7.2fms lost=%-5s NOERR=%-8s NX=%s", q, l*1000, m*1000, lo, ne, nx}'
}
run(){ local port=$1 qf=$2 label=$3; shift 3
  dnsperf -s 127.0.0.1 -p "$port" -d "$qf" -c 20 -T 4 "$@" > "results/probe/$label.txt" 2>&1
  printf "  %-24s %s\n" "$label" "$(pf results/probe/$label.txt)"
}

echo "### CEILING: dnsperf -> stub direct ###"
run 5300 queries/hit_10k.txt         stub_hotA        -l 5 -n 100000
run 5300 queries/foreign_miss_2m.txt stub_missnames   -l 5
for pair in photondns:5353 mosdns:5354; do
  name=${pair%%:*}; port=${pair##*:}
  echo "### $name (flat-out 5s; hot set warmed first) ###"
  dnsperf -s 127.0.0.1 -p "$port" -d queries/hit_10k.txt -c 8 -T 4 -n 1 >/dev/null 2>&1
  run "$port" queries/hit_10k.txt         "${name}_cache_hit"    -l 5 -n 1000000
  run "$port" queries/ad_block_50k.txt    "${name}_ad_block"     -l 5 -n 100
  run "$port" queries/foreign_miss_2m.txt "${name}_foreign_miss" -l 5
  run "$port" queries/china_miss_2m.txt   "${name}_china_miss"   -l 5
done
echo "(daemons left running for follow-up)"