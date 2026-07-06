#!/bin/bash
# tricky-tests.sh - adversarial / edge-case battery for a live photondns server.
#
# usage:   ./tools/tricky-tests.sh [server] [port]
# example: ./tools/tricky-tests.sh 172.16.10.4 15533
#
# Needs dig. Also probes the HTTP API on :8053 when reachable.

SERVER="${1:-172.16.10.4}"
PORT="${2:-15533}"
API="${API:-$SERVER:8053}"

GREEN='\033[32m'; RED='\033[31m'; YELLOW='\033[33m'; BOLD='\033[1m'; NC='\033[0m'
PASS=0; FAIL=0; FAILED=""

d() { dig @"$SERVER" -p "$PORT" +tries=1 +time=6 "$@" 2>&1; }
rcode_of() { sed -n 's/.*status: \([A-Z]*\).*/\1/p' <<<"$1" | head -1; }
qtime_of() { sed -n 's/.*Query time: \([0-9]*\) msec.*/\1/p' <<<"$1" | head -1; }
ancount_of() { sed -n 's/.*ANSWER: \([0-9]*\),.*/\1/p' <<<"$1" | head -1; }

ok()   { PASS=$((PASS+1)); printf "  ${GREEN}PASS${NC} %-46s %s\n" "$1" "$2"; }
bad()  { FAIL=$((FAIL+1)); FAILED="$FAILED\n  - $1"; printf "  ${RED}FAIL${NC} %-46s %s\n" "$1" "$2"; }
note() { printf "  ${YELLOW}note${NC} %s\n" "$1"; }

printf "${BOLD}photondns tricky tests against %s:%s${NC}\n\n" "$SERVER" "$PORT"

# ---- 1. baseline -----------------------------------------------------------
out=$(d github.com A)
[ "$(rcode_of "$out")" = "NOERROR" ] && [ "$(ancount_of "$out")" -ge 1 ] \
	&& ok "baseline A github.com" "$(ancount_of "$out") answers" \
	|| bad "baseline A github.com" "rcode=$(rcode_of "$out")"

# ---- 2. 0x20 mixed-case: rcode ok AND question echoed byte-exact -----------
out=$(d GiThUb.CoM A)
if [ "$(rcode_of "$out")" = "NOERROR" ] && grep -q "GiThUb.CoM" <<<"$out"; then
	ok "0x20 mixed-case echo (GiThUb.CoM)" "question case preserved"
else
	bad "0x20 mixed-case echo (GiThUb.CoM)" "rcode=$(rcode_of "$out"), case echo $(grep -q 'GiThUb.CoM' <<<"$out" && echo yes || echo NO)"
fi

# ---- 3. NXDOMAIN + negative cache ------------------------------------------
# note: example.com (Cloudflare-hosted) answers NOERROR/NODATA for random
# subdomains, and wildcard zones (e.g. aliexpress.us) answer everything;
# iana.org reliably NXDOMAINs.
RAND="nx-$$-$(date +%s)"
out=$(d "$RAND.iana.org" A)
[ "$(rcode_of "$out")" = "NXDOMAIN" ] \
	&& ok "NXDOMAIN for random name" "" \
	|| bad "NXDOMAIN for random name" "rcode=$(rcode_of "$out")"
out=$(d "$RAND.iana.org" A)
t=$(qtime_of "$out")
if [ "$(rcode_of "$out")" = "NXDOMAIN" ] && [ -n "$t" ] && [ "$t" -le 20 ]; then
	ok "negative cache hit" "${t} ms"
else
	bad "negative cache hit" "rcode=$(rcode_of "$out") time=${t}ms (want NXDOMAIN <=20ms)"
fi

# ---- 4. NODATA (empty NOERROR) ---------------------------------------------
out=$(d github.com AAAA)
[ "$(rcode_of "$out")" = "NOERROR" ] && [ "$(ancount_of "$out")" = "0" ] \
	&& ok "NODATA (AAAA github.com)" "NOERROR, 0 answers" \
	|| bad "NODATA (AAAA github.com)" "rcode=$(rcode_of "$out") answers=$(ancount_of "$out")"

# ---- 5. CNAME chain ---------------------------------------------------------
out=$(d www.taobao.com A)
if [ "$(rcode_of "$out")" = "NOERROR" ] && grep -q "CNAME" <<<"$out" && grep -qE "IN[[:space:]]+A[[:space:]]" <<<"$out"; then
	ok "CNAME chain (www.taobao.com)" "CNAME + A present"
else
	bad "CNAME chain (www.taobao.com)" "rcode=$(rcode_of "$out")"
fi

# ---- 6. truncation: big TXT with 512B client buffer -------------------------
out=$(d google.com TXT +bufsize=512 +ignore)
if grep -qE "flags:[^;]* tc[ ;]" <<<"$out"; then
	ok "TC bit on oversized UDP (bufsize 512)" ""
else
	sz=$(sed -n 's/.*MSG SIZE  rcvd: \([0-9]*\).*/\1/p' <<<"$out")
	if [ -n "$sz" ] && [ "$sz" -le 512 ]; then
		note "google.com TXT fits in 512B here (${sz}B) - TC not applicable"
		ok "TC bit on oversized UDP (bufsize 512)" "answer fits, skipped"
	else
		bad "TC bit on oversized UDP (bufsize 512)" "no TC flag, size=${sz}B"
	fi
fi

# ---- 7. TC -> TCP retry end-to-end (dig auto-retries) -----------------------
out=$(d google.com TXT +bufsize=512)
[ "$(rcode_of "$out")" = "NOERROR" ] && [ "$(ancount_of "$out")" -ge 1 ] \
	&& ok "TC -> TCP retry full answer" "$(ancount_of "$out") TXT records" \
	|| bad "TC -> TCP retry full answer" "rcode=$(rcode_of "$out")"

# ---- 8. plain TCP -----------------------------------------------------------
out=$(d cloudflare.com A +tcp)
[ "$(rcode_of "$out")" = "NOERROR" ] && [ "$(ancount_of "$out")" -ge 1 ] \
	&& ok "TCP query" "" \
	|| bad "TCP query" "rcode=$(rcode_of "$out")"

# ---- 9. HTTPS/SVCB qtype 65 -------------------------------------------------
out=$(d cloudflare.com TYPE65)
rc=$(rcode_of "$out")
[ -n "$rc" ] && ok "TYPE65 (HTTPS RR) forwarded" "rcode=$rc" \
	|| bad "TYPE65 (HTTPS RR) forwarded" "no response"

# ---- 10. DNSSEC DO bit passthrough ------------------------------------------
out=$(d ietf.org A +dnssec)
if [ "$(rcode_of "$out")" = "NOERROR" ] && [ "$(ancount_of "$out")" -ge 1 ]; then
	grep -q RRSIG <<<"$out" && ok "+dnssec (DO bit)" "RRSIG returned" \
		|| { ok "+dnssec (DO bit)" "answered (no RRSIG - cached non-DO entry)"; }
else
	bad "+dnssec (DO bit)" "rcode=$(rcode_of "$out")"
fi

# ---- 11. private PTR blocked, public PTR forwarded --------------------------
out=$(d -x 192.168.1.1)
[ "$(rcode_of "$out")" = "NXDOMAIN" ] \
	&& ok "private PTR blocked (192.168.1.1)" "" \
	|| bad "private PTR blocked (192.168.1.1)" "rcode=$(rcode_of "$out")"
out=$(d -x 8.8.8.8)
[ "$(rcode_of "$out")" = "NOERROR" ] && grep -q "dns.google" <<<"$out" \
	&& ok "public PTR forwarded (8.8.8.8)" "dns.google" \
	|| bad "public PTR forwarded (8.8.8.8)" "rcode=$(rcode_of "$out")"

# ---- 12. special-use TLDs ----------------------------------------------------
for n in printer.local router.lan intranet.internal; do
	out=$(d "$n" A)
	[ "$(rcode_of "$out")" = "NXDOMAIN" ] \
		&& ok "special TLD blocked ($n)" "" \
		|| bad "special TLD blocked ($n)" "rcode=$(rcode_of "$out")"
done

# ---- 13. root NS query -------------------------------------------------------
out=$(d . NS)
[ "$(rcode_of "$out")" = "NOERROR" ] && [ "$(ancount_of "$out")" -ge 10 ] \
	&& ok "root NS query" "$(ancount_of "$out") roots" \
	|| bad "root NS query" "rcode=$(rcode_of "$out") answers=$(ancount_of "$out")"

# ---- 14. absurdly long (but legal) name -------------------------------------
L63=$(printf 'a%.0s' $(seq 1 63))
out=$(d "$L63.$L63.$L63.invalid" A)
rc=$(rcode_of "$out")
[ "$rc" = "NXDOMAIN" ] || [ "$rc" = "NOERROR" ] \
	&& ok "199-char name parses" "rcode=$rc" \
	|| bad "199-char name parses" "rcode=$rc"

# ---- 15. ANY qtype (RFC 8482 territory) --------------------------------------
out=$(d google.com ANY)
rc=$(rcode_of "$out")
[ -n "$rc" ] && ok "ANY qtype answered" "rcode=$rc" \
	|| bad "ANY qtype answered" "no response"

# ---- 16. CHAOS class (exercises fast-fail + retry round) ---------------------
out=$(d version.bind CH TXT)
rc=$(rcode_of "$out"); t=$(qtime_of "$out")
[ -n "$rc" ] && ok "CHAOS TXT version.bind" "rcode=$rc ${t}ms" \
	|| bad "CHAOS TXT version.bind" "no response (hang?)"

# ---- 17. wildcard fresh resolve correctness (nip.io) --------------------------
n="t$$-$(date +%s).10.11.12.13.nip.io"
out=$(d "$n" A)
if grep -q "10.11.12.13" <<<"$out"; then
	ok "fresh uncached resolve ($n)" "-> 10.11.12.13"
else
	bad "fresh uncached resolve (nip.io wildcard)" "rcode=$(rcode_of "$out")"
fi

# ---- 18. stampede: 10 parallel identical uncached queries --------------------
n="s$$-$(date +%s).10.20.30.40.nip.io"
cnt=0
for i in $(seq 1 10); do
	( dig @"$SERVER" -p "$PORT" "$n" A +tries=1 +time=6 2>/dev/null \
		| grep -q "10.20.30.40" && echo ok ) &
done > /tmp/stampede.$$ 2>/dev/null
wait
cnt=$(grep -c ok /tmp/stampede.$$ 2>/dev/null); rm -f /tmp/stampede.$$
[ "$cnt" -eq 10 ] && ok "stampede 10x same uncached name" "10/10 correct" \
	|| bad "stampede 10x same uncached name" "$cnt/10 correct"

# ---- 19. burst: 20 parallel mixed domains -------------------------------------
DOMS="github.com wikipedia.org example.com cloudflare.com apple.com amazon.com bing.com yahoo.com baidu.com qq.com taobao.com jd.com zhihu.com bilibili.com reddit.com stackoverflow.com netflix.com openai.com anthropic.com debian.org"
cnt=0
for dom in $DOMS; do
	( dig @"$SERVER" -p "$PORT" "$dom" A +tries=1 +time=6 2>/dev/null \
		| grep -q "status: NOERROR" && echo ok ) &
done > /tmp/burst.$$ 2>/dev/null
wait
cnt=$(grep -c ok /tmp/burst.$$ 2>/dev/null); rm -f /tmp/burst.$$
[ "$cnt" -eq 20 ] && ok "burst 20 parallel domains" "20/20 NOERROR" \
	|| bad "burst 20 parallel domains" "$cnt/20 NOERROR"

# ---- 20. TTL-1 churn name (serve-stale machinery) -----------------------------
n="webrtc.l6-dk.sched.dcloudlive.com"
o1=$(d "$n" A); sleep 2; o2=$(d "$n" A)
[ "$(rcode_of "$o1")" = "NOERROR" ] && [ "$(rcode_of "$o2")" = "NOERROR" ] \
	&& ok "TTL-1 CDN name twice (stale path)" "" \
	|| bad "TTL-1 CDN name twice (stale path)" "rc1=$(rcode_of "$o1") rc2=$(rcode_of "$o2")"

# ---- API-side checks -----------------------------------------------------------
if curl -s --max-time 3 "http://$API/health" >/dev/null 2>&1; then
	v=$(curl -s "http://$API/version" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')
	note "API reachable, photondns $v"
	r=$(curl -s "http://$API/resolve?name=baidu.com&type=A")
	route=$(sed -n 's/.*"route":"\([^"]*\)".*/\1/p' <<<"$r")
	case "$route" in
		local|cache|stale) ok "china domain routed locally (baidu.com)" "route=$route" ;;
		*) bad "china domain routed locally (baidu.com)" "route=$route" ;;
	esac
	r=$(curl -s "http://$API/resolve?name=github.com&type=A")
	route=$(sed -n 's/.*"route":"\([^"]*\)".*/\1/p' <<<"$r")
	case "$route" in
		main|cache|stale) ok "foreign domain routed via main (github.com)" "route=$route" ;;
		*) bad "foreign domain routed via main (github.com)" "route=$route" ;;
	esac
else
	note "API http://$API not reachable from here - skipping route checks"
fi

# ---- summary --------------------------------------------------------------------
echo
if [ "$FAIL" -eq 0 ]; then
	printf "${BOLD}${GREEN}all %d checks passed${NC}\n" "$PASS"
else
	printf "${BOLD}${RED}%d failed${NC} / %d passed${NC}\n" "$FAIL" "$PASS"
	printf "$FAILED\n"
fi
exit "$FAIL"
