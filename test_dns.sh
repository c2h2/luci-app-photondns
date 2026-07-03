#!/bin/sh
# test_dns.sh - exercise a photondns server with dig, with verbose + timing output.
#
# usage:   ./test_dns.sh <domain> [server] [port]
# example: ./test_dns.sh baidu.com 192.168.1.1 15533
#
# env overrides: DNS_SERVER, DNS_PORT, API (e.g. API=127.0.0.1:8053 when
# running on the router itself - then the route/upstream taken is shown too)

DOMAIN="$1"
SERVER="${2:-${DNS_SERVER:-192.168.1.1}}"
PORT="${3:-${DNS_PORT:-15533}}"
API="${API:-127.0.0.1:8053}"

[ -z "$DOMAIN" ] && { echo "usage: $0 <domain> [server] [port]"; exit 2; }

HAVE_DIG=0
command -v dig >/dev/null && HAVE_DIG=1

GREEN='\033[32m'; RED='\033[31m'; YELLOW='\033[33m'; CYAN='\033[36m'; BOLD='\033[1m'; NC='\033[0m'

hdr()  { printf "\n${BOLD}${CYAN}== %s${NC}\n" "$*"; }
run()  { printf "${YELLOW}\$ %s${NC}\n" "$*"; }

# one dig pass; prints status, answers, query time; returns time in QT_MS
QT_MS=""
digq() {
	qtype="$1"; shift
	run "dig @$SERVER -p $PORT $DOMAIN $qtype $*"
	out=$(dig "@$SERVER" -p "$PORT" "$DOMAIN" "$qtype" +tries=1 +time=5 "$@" 2>&1)
	status=$(echo "$out" | sed -n 's/.*status: \([A-Z]*\).*/\1/p' | head -1)
	QT_MS=$(echo "$out" | sed -n 's/.*Query time: \([0-9]*\) msec.*/\1/p' | head -1)
	answers=$(echo "$out" | awk '/^;; ANSWER SECTION:/,/^$/' | grep -v '^;;' | grep -v '^$')
	if [ -z "$status" ]; then
		printf "  ${RED}NO RESPONSE (timeout?)${NC}\n"
		QT_MS=""
		return 1
	fi
	if [ "$status" = "NOERROR" ]; then
		printf "  status: ${GREEN}%s${NC}   query time: ${BOLD}%s ms${NC}\n" "$status" "${QT_MS:-?}"
	else
		printf "  status: ${RED}%s${NC}   query time: ${BOLD}%s ms${NC}\n" "$status" "${QT_MS:-?}"
	fi
	if [ -n "$answers" ]; then
		echo "$answers" | sed 's/^/  /'
	else
		printf "  ${YELLOW}(no answer records)${NC}\n"
	fi
	return 0
}

printf "${BOLD}photondns test: %s via %s:%s${NC}\n" "$DOMAIN" "$SERVER" "$PORT"

# ---------------------------------------------------------------- fallback
# no dig (e.g. busybox-only router): use nslookup for answers and
# photonbench (ships with photondns) for precise timing
if [ "$HAVE_DIG" = "0" ]; then
	printf "${YELLOW}dig not found - using busybox nslookup + photonbench fallback${NC}\n"

	hdr "1. nslookup A/AAAA"
	run "nslookup -port=$PORT $DOMAIN $SERVER"
	out=$(nslookup -port="$PORT" "$DOMAIN" "$SERVER" 2>&1)
	if echo "$out" | grep -q "Address"; then
		echo "$out" | grep -A1 "^Name" | grep -v '^--$' | sed 's/^/  /'
	else
		printf "  ${RED}resolution failed:${NC}\n"
		echo "$out" | sed 's/^/  /' | head -4
	fi

	if command -v photonbench >/dev/null; then
		hdr "2. precise timing, 5 sequential queries (photonbench)"
		i=1
		while [ $i -le 5 ]; do
			r=$(photonbench "$SERVER:$PORT" "$DOMAIN" 1 1 2>/dev/null | sed -n 's/.*avg \([0-9.]*\) ms.*/\1/p')
			printf "  query %d: %s ms\n" "$i" "${r:-timeout}"
			i=$((i + 1))
		done
	else
		hdr "2. timing (time nslookup)"
		time nslookup -port="$PORT" "$DOMAIN" "$SERVER" >/dev/null 2>&1
	fi
else

hdr "1. UDP A query (cold or cached)"
digq A
t1="$QT_MS"

hdr "2. UDP A query again (should be a cache hit, ~0 ms)"
digq A
t2="$QT_MS"
if [ -n "$t1" ] && [ -n "$t2" ]; then
	printf "  ${BOLD}timing: first=%s ms, second=%s ms${NC}" "$t1" "$t2"
	[ "$t2" -le 1 ] 2>/dev/null && printf "  ${GREEN}<- cache hit${NC}"
	printf "\n"
fi

hdr "3. AAAA query"
digq AAAA

hdr "4. TCP query"
digq A +tcp

hdr "5. latency over 5 repeated queries (cache stability)"
i=1; sum=0; ok=0
while [ $i -le 5 ]; do
	ms=$(dig "@$SERVER" -p "$PORT" "$DOMAIN" A +tries=1 +time=5 2>/dev/null \
		| sed -n 's/.*Query time: \([0-9]*\) msec.*/\1/p')
	if [ -n "$ms" ]; then
		printf "  query %d: %s ms\n" "$i" "$ms"
		sum=$((sum + ms)); ok=$((ok + 1))
	else
		printf "  query %d: ${RED}timeout${NC}\n" "$i"
	fi
	i=$((i + 1))
done
[ "$ok" -gt 0 ] && printf "  ${BOLD}avg: %s ms over %d queries${NC}\n" "$((sum / ok))" "$ok"

fi # HAVE_DIG

# when the API is reachable (i.e. running on the router), show how the
# query was routed and which upstream won the race
if command -v curl >/dev/null && curl -s --max-time 2 "http://$API/health" >/dev/null 2>&1; then
	hdr "6. route taken (from query log at $API)"
	curl -s "http://$API/log?n=200" | tr '{' '\n' | grep "\"$DOMAIN\"" | head -4 | while read -r line; do
		route=$(echo "$line" | sed -n 's/.*"route":"\([^"]*\)".*/\1/p')
		up=$(echo "$line" | sed -n 's/.*"upstream":"\([^"]*\)".*/\1/p')
		rtt=$(echo "$line" | sed -n 's/.*"rtt_ms":\([0-9.]*\).*/\1/p')
		qt=$(echo "$line" | sed -n 's/.*"qtype":\([0-9]*\).*/\1/p')
		printf "  qtype=%-3s route=${GREEN}%-8s${NC} upstream=%-24s rtt=%s ms\n" "$qt" "$route" "${up:--}" "$rtt"
	done
	hdr "7. server stats snapshot"
	curl -s "http://$API/stats" | tr ',' '\n' | grep -E '"(total|qpm|hits|misses|hit_rate|stale_served|hedged)"' | sed 's/[{}"]//g; s/^/  /'
else
	printf "\n${YELLOW}(API http://%s not reachable from here - run this script on the router to also see the route/upstream taken per query)${NC}\n" "$API"
fi
echo
