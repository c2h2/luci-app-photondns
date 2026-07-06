#!/usr/bin/env python3
"""Compare a live photondns server against an independent DoH reference.

Randomly samples N domains from the Tranco top-sites list, resolves each
twice:

  reference: Google DoH JSON API (https://8.8.8.8/resolve), falling back to
             Cloudflare (https://1.1.1.1/dns-query). HTTPS transport, so the
             gateway's DNS is completely bypassed even on hijacking LANs.
  gateway:   raw DNS over UDP to <server>:<port>, TCP retry on truncation.

Both result sets are stored as JSON and the answers compared:

  match           same rcode, identical IP set
  overlap         at least one IP in common (CDN rotation)
  match-negative  both sides NXDOMAIN / empty
  disjoint        both answered, zero common IPs (geo-split CDN *or* poisoning)
  rcode-mismatch / gw-empty / ref-empty / gw-fail / ref-fail   real trouble

usage: tools/compare-dns.py [-n 200] [-s 172.16.10.4] [-p 15533]
                            [--top 100000] [-o dns-compare.json]
"""

import argparse
import concurrent.futures
import io
import json
import os
import random
import socket
import ssl
import struct
import sys
import time
import urllib.request
import zipfile

TRANCO_URL = "https://tranco-list.eu/top-1m.csv.zip"
TRANCO_CACHE = "/tmp/tranco-top1m.csv"


def load_domains(top):
    if not os.path.exists(TRANCO_CACHE):
        print(f"downloading tranco list ({TRANCO_URL}) ...", file=sys.stderr)
        with urllib.request.urlopen(TRANCO_URL, timeout=60) as r:
            blob = r.read()
        with zipfile.ZipFile(io.BytesIO(blob)) as z:
            name = z.namelist()[0]
            with open(TRANCO_CACHE, "wb") as f:
                f.write(z.read(name))
    doms = []
    with open(TRANCO_CACHE) as f:
        for line in f:
            try:
                _, dom = line.strip().split(",", 1)
            except ValueError:
                continue
            doms.append(dom)
            if len(doms) >= top:
                break
    return doms


# ------------------------------------------------------------- reference DoH
def doh_json(url, domain, qtype=1, timeout=8):
    req = urllib.request.Request(
        url + "?name=" + urllib.parse.quote(domain) + f"&type={qtype}",
        headers={"accept": "application/dns-json"},
    )
    ctx = ssl.create_default_context()
    ctx.check_hostname = False  # we connect by IP on purpose
    ctx.verify_mode = ssl.CERT_NONE
    with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
        j = json.load(r)
    ans = [a["data"] for a in j.get("Answer", []) if a.get("type") == qtype]
    return j.get("Status", -1), sorted(ans)


def ptr_suffix(ip):
    """Last two labels of the IP's PTR name ('' if none) - identifies the CDN.
    e.g. 13.249.74.2 -> ...r.cloudfront.net -> 'cloudfront.net'"""
    rev = ".".join(reversed(ip.split("."))) + ".in-addr.arpa"
    try:
        _, names = doh_json("https://8.8.8.8/resolve", rev, qtype=12)
        if names:
            return ".".join(names[0].rstrip(".").lower().split(".")[-2:])
    except Exception:
        pass
    return ""


def ref_resolve(domain):
    t0 = time.time()
    for url in ("https://8.8.8.8/resolve", "https://1.1.1.1/dns-query"):
        try:
            rcode, ips = doh_json(url, domain)
            return {"rcode": rcode, "ips": ips, "ms": round((time.time() - t0) * 1e3, 1)}
        except Exception:
            continue
    return {"rcode": None, "ips": [], "ms": round((time.time() - t0) * 1e3, 1), "error": "doh failed"}


# --------------------------------------------------------------- gateway DNS
def build_query(domain):
    qid = random.randrange(1, 0xFFFF)
    pkt = struct.pack(">HHHHHH", qid, 0x0100, 1, 0, 0, 0)
    for label in domain.rstrip(".").split("."):
        pkt += bytes([len(label)]) + label.encode()
    pkt += b"\x00" + struct.pack(">HH", 1, 1)  # A IN
    return qid, pkt


def skip_name(buf, pos):
    while True:
        ln = buf[pos]
        if ln == 0:
            return pos + 1
        if ln & 0xC0 == 0xC0:
            return pos + 2
        pos += ln + 1


def parse_answer(buf):
    rcode = buf[3] & 0x0F
    tc = bool(buf[2] & 0x02)
    qd, an = struct.unpack(">HH", buf[4:8])
    pos = 12
    for _ in range(qd):
        pos = skip_name(buf, pos) + 4
    ips = []
    for _ in range(an):
        pos = skip_name(buf, pos)
        rtype, _cls, _ttl, rdlen = struct.unpack(">HHIH", buf[pos : pos + 10])
        pos += 10
        if rtype == 1 and rdlen == 4:
            ips.append(".".join(str(b) for b in buf[pos : pos + 4]))
        pos += rdlen
    return rcode, tc, sorted(ips)


def gw_resolve(domain, server, port, timeout=12):
    t0 = time.time()
    qid, pkt = build_query(domain)
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.settimeout(timeout)
        s.sendto(pkt, (server, port))
        while True:
            buf, _ = s.recvfrom(4096)
            if len(buf) >= 12 and struct.unpack(">H", buf[:2])[0] == qid:
                break
        s.close()
        rcode, tc, ips = parse_answer(buf)
        if tc:  # truncated: retry over TCP
            t = socket.create_connection((server, port), timeout=timeout)
            t.sendall(struct.pack(">H", len(pkt)) + pkt)
            ln = struct.unpack(">H", t.recv(2))[0]
            buf = b""
            while len(buf) < ln:
                chunk = t.recv(ln - len(buf))
                if not chunk:
                    break
                buf += chunk
            t.close()
            rcode, _, ips = parse_answer(buf)
        return {"rcode": rcode, "ips": ips, "ms": round((time.time() - t0) * 1e3, 1)}
    except Exception as e:
        return {"rcode": None, "ips": [], "ms": round((time.time() - t0) * 1e3, 1), "error": str(e)}


# ----------------------------------------------------------------- comparison
def verdict(ref, gw):
    if ref["rcode"] is None:
        return "ref-fail"
    if gw["rcode"] is None:
        return "gw-fail"
    r, g = set(ref["ips"]), set(gw["ips"])
    if not r and not g:
        return "match-negative" if ref["rcode"] == gw["rcode"] else "rcode-mismatch"
    if ref["rcode"] != gw["rcode"] and not (r and g):
        return "rcode-mismatch"
    if r == g:
        return "match"
    if r & g:
        return "overlap"
    if r and not g:
        return "gw-empty"
    if g and not r:
        return "ref-empty"
    return "disjoint"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=200, help="number of domains")
    ap.add_argument("-s", "--server", default="172.16.10.4")
    ap.add_argument("-p", "--port", type=int, default=15533)
    ap.add_argument("--top", type=int, default=100000, help="sample from top-N tranco")
    ap.add_argument("-o", "--out", default="/tmp/dns-compare.json")
    ap.add_argument("--seed", type=int, default=None)
    args = ap.parse_args()

    if args.seed is not None:
        random.seed(args.seed)
    domains = random.sample(load_domains(args.top), args.n)

    def work(dom):
        return dom, ref_resolve(dom), gw_resolve(dom, args.server, args.port)

    results = []
    t0 = time.time()
    with concurrent.futures.ThreadPoolExecutor(max_workers=24) as ex:
        for dom, ref, gw in ex.map(work, domains):
            v = verdict(ref, gw)
            results.append({"domain": dom, "verdict": v, "ref": ref, "gw": gw})
            mark = "" if v in ("match", "overlap", "match-negative") else "   <-- " + v
            print(f"  {dom:45s} ref={len(ref['ips'])}ips gw={len(gw['ips'])}ips {v}{mark}")

    # disjoint answers are usually the same CDN handing out different edge
    # POPs per resolver; confirm via PTR suffix (cloudfront.net etc.)
    def cdn_check(r):
        a, b = ptr_suffix(r["ref"]["ips"][0]), ptr_suffix(r["gw"]["ips"][0])
        if a and a == b:
            r["verdict"] = "same-cdn"
            r["cdn"] = a
        return r

    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as ex:
        list(ex.map(cdn_check, (r for r in results if r["verdict"] == "disjoint")))

    counts = {}
    for r in results:
        counts[r["verdict"]] = counts.get(r["verdict"], 0) + 1
    good = sum(counts.get(k, 0) for k in ("match", "overlap", "match-negative", "same-cdn"))
    ref_ms = sorted(r["ref"]["ms"] for r in results)
    gw_ms = sorted(r["gw"]["ms"] for r in results)

    out = {
        "meta": {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "server": f"{args.server}:{args.port}",
            "reference": "google/cloudflare DoH JSON",
            "count": args.n,
            "elapsed_s": round(time.time() - t0, 1),
        },
        "summary": counts,
        "results": results,
    }
    with open(args.out, "w") as f:
        json.dump(out, f, indent=1)

    print(f"\nsummary ({args.n} domains, {out['meta']['elapsed_s']}s):")
    for k in sorted(counts, key=counts.get, reverse=True):
        print(f"  {k:15s} {counts[k]}")
    print(f"  agreement: {good}/{args.n} ({100.0*good/args.n:.1f}%)")
    print(f"  median latency: ref {ref_ms[len(ref_ms)//2]:.0f}ms  gw {gw_ms[len(gw_ms)//2]:.0f}ms")
    bad = [r for r in results if r["verdict"] not in ("match", "overlap", "match-negative", "same-cdn", "disjoint")]
    if bad:
        print("\nproblem domains:")
        for r in bad:
            print(f"  {r['domain']:40s} {r['verdict']:14s} ref={r['ref']['rcode']}:{r['ref']['ips']} gw={r['gw']['rcode']}:{r['gw']['ips']}")
    print(f"\nfull results stored in {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
