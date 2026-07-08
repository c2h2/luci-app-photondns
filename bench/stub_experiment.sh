#!/usr/bin/env bash
# Decisive test: are mosdns's ~1s forward retransmits stub-side drops or mosdns-side?
# Run foreign_miss flat-out on each resolver against BOTH the plain stub and the
# hardened (SO_REUSEPORT + 8MB buffers) stub, count queries >500ms, and read the
# kernel UDP "full socket buffer" drop counter delta around each run.
set -u; cd "$(dirname "$0")"
SECS="${SECS:-6}"
start_pd(){ pkill -f 'release/[p]hotondns' 2>/dev/null; sleep .3; nohup ../target/release/photondns -c conf/photondns.toml >/tmp/pd.log 2>&1 & sleep 1.2; }
start_md(){ pkill -f '[m]osdns start' 2>/dev/null; sleep .3; GOMAXPROCS=8 nohup ./mosdns start -c conf/mosdns.yaml -d . >/tmp/md.log 2>&1 & sleep 1.5; }
udp_fullbuf(){ netstat -s -p udp | sed -n 's/.*\([0-9][0-9]*\) dropped due to full socket buffers.*/\1/p'; }

run_one(){ local res=$1 port=$2 stub=$3
  [ "$res" = pd ] && start_pd || start_md
  dig @127.0.0.1 -p "$port" +short +time=2 +tries=1 warm.check A >/dev/null 2>&1
  local d0 d1 f="results/stubexp_${res}_${stub}.raw"
  d0=$(udp_fullbuf)
  dnsperf -s 127.0.0.1 -p "$port" -d queries/foreign_miss_2m.txt -c 20 -T 4 -l "$SECS" -v >"$f" 2>&1
  d1=$(udp_fullbuf)
  local qps mx over500 over50
  qps=$(grep 'Queries per second' "$f"|grep -oE '[0-9]+\.[0-9]+'|head -1)
  mx=$(grep 'Average Latency' "$f"|grep -oE '[0-9]+\.[0-9]+'|sed -n '3p')
  over500=$(grep '^> ' "$f"|grep -oE '[0-9]+\.[0-9]+$'|awk '$1>0.5'|wc -l|tr -d ' ')
  over50=$(grep '^> ' "$f"|grep -oE '[0-9]+\.[0-9]+$'|awk '$1>0.05'|wc -l|tr -d ' ')
  printf "  %-3s vs %-11s QPS=%-8.0f maxLat=%8.1fms  >50ms=%-5s >500ms=%-5s  udp_fullbuf_delta=%s\n" \
    "$res" "$stub" "${qps:-0}" "$(echo "${mx:-0}*1000"|bc -l)" "$over50" "$over500" "$((${d1:-0}-${d0:-0}))"
}

echo "== building hardened stub =="; go build -o stubdns_hard stubdns_hard.go && echo "  ok"
echo "== macOS UDP buffer limits: recvspace=$(sysctl -n net.inet.udp.recvspace) maxdgram=$(sysctl -n net.inet.udp.maxdgram) maxsockbuf=$(sysctl -n kern.ipc.maxsockbuf) =="

echo "### PLAIN stub (single socket, default buffer) ###"
pkill -f '[s]tubdns' 2>/dev/null; sleep .3; nohup ./stubdns 127.0.0.1:5300 >/tmp/stub.log 2>&1 & sleep .6
run_one pd 5353 plain
run_one md 5354 plain

echo "### HARDENED stub (reuseport, 8MB buffers) ###"
pkill -f 'stubdns' 2>/dev/null; sleep .5; nohup ./stubdns_hard 127.0.0.1:5300 >/tmp/stubh.log 2>&1 & sleep .6
run_one pd 5353 hardened
run_one md 5354 hardened
pkill -f '[d]nsperf' 2>/dev/null
echo "(interpretation: if md's >500ms count and udp_fullbuf_delta drop to ~0 with the hardened stub, the 1s spikes were stub-side; if they persist, they are mosdns-side)"