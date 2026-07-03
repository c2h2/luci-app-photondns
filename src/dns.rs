//! Raw DNS wire-format helpers. photondns forwards packets without a full
//! re-encode: we parse just enough for routing/caching and patch bytes
//! (ID, TTLs, question section) in place.

pub const HEADER_LEN: usize = 12;
pub const TYPE_A: u16 = 1;
pub const TYPE_SOA: u16 = 6;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_OPT: u16 = 41;
pub const TYPE_HTTPS: u16 = 65;
pub const RCODE_NOERROR: u8 = 0;
pub const RCODE_FORMERR: u8 = 1;
pub const RCODE_SERVFAIL: u8 = 2;
pub const RCODE_NXDOMAIN: u8 = 3;

#[derive(Debug, Clone)]
pub struct QueryMeta {
    pub id: u16,
    /// lowercase qname without trailing dot ("" for root)
    pub qname: String,
    pub qtype: u16,
    pub qclass: u16,
    /// byte offset one past the end of the (first) question
    pub question_end: usize,
    /// max UDP payload the client advertised via EDNS (512 if none)
    pub udp_size: u16,
}

pub fn get_id(buf: &[u8]) -> u16 {
    u16::from_be_bytes([buf[0], buf[1]])
}

pub fn set_id(buf: &mut [u8], id: u16) {
    buf[0..2].copy_from_slice(&id.to_be_bytes());
}

pub fn is_response(buf: &[u8]) -> bool {
    buf.len() >= HEADER_LEN && buf[2] & 0x80 != 0
}

pub fn is_truncated(buf: &[u8]) -> bool {
    buf.len() >= HEADER_LEN && buf[2] & 0x02 != 0
}

pub fn rcode(buf: &[u8]) -> u8 {
    buf[3] & 0x0F
}

fn u16_at(buf: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*buf.get(pos)?, *buf.get(pos + 1)?]))
}

fn u32_at(buf: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *buf.get(pos)?,
        *buf.get(pos + 1)?,
        *buf.get(pos + 2)?,
        *buf.get(pos + 3)?,
    ]))
}

/// Skip over a (possibly compressed) name starting at `pos`.
/// Returns the offset just past the name.
pub fn skip_name(buf: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *buf.get(pos)? as usize;
        if len == 0 {
            return Some(pos + 1);
        }
        if len & 0xC0 == 0xC0 {
            // compression pointer: 2 bytes, ends the name
            buf.get(pos + 1)?;
            return Some(pos + 2);
        }
        if len & 0xC0 != 0 {
            return None; // reserved label types
        }
        pos += len + 1;
        if pos > buf.len() {
            return None;
        }
    }
}

/// Read a name into a lowercase dotted string (no trailing dot), following
/// compression pointers. Returns (name, offset_past_name_at_original_position).
pub fn read_name(buf: &[u8], start: usize) -> Option<(String, usize)> {
    let mut name = String::with_capacity(32);
    let mut pos = start;
    let mut end: Option<usize> = None;
    let mut jumps = 0u32;
    loop {
        let len = *buf.get(pos)? as usize;
        if len == 0 {
            if end.is_none() {
                end = Some(pos + 1);
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | *buf.get(pos + 1)? as usize;
            if end.is_none() {
                end = Some(pos + 2);
            }
            jumps += 1;
            if jumps > 64 || ptr >= pos {
                return None; // loop guard: pointers must go backwards
            }
            pos = ptr;
            continue;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        let label = buf.get(pos + 1..pos + 1 + len)?;
        if !name.is_empty() {
            name.push('.');
        }
        for &b in label {
            name.push((b as char).to_ascii_lowercase());
        }
        if name.len() > 255 {
            return None;
        }
        pos += len + 1;
    }
    Some((name, end.unwrap()))
}

/// Parse the header + first question of a query or response.
pub fn parse_query(buf: &[u8]) -> Option<QueryMeta> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let qdcount = u16_at(buf, 4)?;
    if qdcount == 0 {
        return None;
    }
    let (qname, after_name) = read_name(buf, HEADER_LEN)?;
    let qtype = u16_at(buf, after_name)?;
    let qclass = u16_at(buf, after_name + 2)?;
    let question_end = after_name + 4;

    // find EDNS OPT in additional section to learn client's UDP size
    let mut udp_size = 512u16;
    if let Some(rrs) = walk_rrs(buf, question_end, qdcount as usize - 1) {
        for rr in &rrs {
            if rr.rtype == TYPE_OPT {
                udp_size = rr.class.max(512);
                break;
            }
        }
    }
    Some(QueryMeta {
        id: get_id(buf),
        qname,
        qtype,
        qclass,
        question_end,
        udp_size,
    })
}

#[derive(Debug)]
pub struct RrInfo {
    pub rtype: u16,
    pub class: u16, // for OPT this is the UDP payload size
    pub ttl: u32,
    pub ttl_offset: usize,
    pub rdata_start: usize,
    pub rdata_len: usize,
    pub section: u8, // 0 = answer, 1 = authority, 2 = additional
}

/// Walk all resource records after the question section.
/// `extra_questions` = remaining questions to skip beyond the first.
fn walk_rrs(buf: &[u8], mut pos: usize, extra_questions: usize) -> Option<Vec<RrInfo>> {
    for _ in 0..extra_questions {
        pos = skip_name(buf, pos)? + 4;
    }
    let ancount = u16_at(buf, 6)? as usize;
    let nscount = u16_at(buf, 8)? as usize;
    let arcount = u16_at(buf, 10)? as usize;
    let total = ancount + nscount + arcount;
    let mut out = Vec::with_capacity(total);
    for i in 0..total {
        if pos == buf.len() {
            break; // tolerate short messages
        }
        pos = skip_name(buf, pos)?;
        let rtype = u16_at(buf, pos)?;
        let class = u16_at(buf, pos + 2)?;
        let ttl = u32_at(buf, pos + 4)?;
        let rdlen = u16_at(buf, pos + 8)? as usize;
        let rdata_start = pos + 10;
        if rdata_start + rdlen > buf.len() {
            return None;
        }
        let section = if i < ancount {
            0
        } else if i < ancount + nscount {
            1
        } else {
            2
        };
        out.push(RrInfo {
            rtype,
            class,
            ttl,
            ttl_offset: pos + 4,
            rdata_start,
            rdata_len: rdlen,
            section,
        });
        pos = rdata_start + rdlen;
    }
    Some(out)
}

/// Metadata needed to cache a response.
pub struct CacheInfo {
    /// offsets of TTL fields to age when serving from cache (OPT excluded)
    pub ttl_offsets: Vec<u16>,
    /// effective freshness TTL in seconds
    pub ttl: u32,
    #[allow(dead_code)] // exercised in tests
    pub has_answers: bool,
}

/// Analyze a response for cacheability. Returns None if it should not be cached.
pub fn cache_info(buf: &[u8], question_end: usize, negative_ttl: u32) -> Option<CacheInfo> {
    if !is_response(buf) || is_truncated(buf) {
        return None;
    }
    let rc = rcode(buf);
    if rc != RCODE_NOERROR && rc != RCODE_NXDOMAIN {
        return None;
    }
    let qdcount = u16_at(buf, 4)? as usize;
    let rrs = walk_rrs(buf, question_end, qdcount.saturating_sub(1))?;
    let mut ttl_offsets = Vec::with_capacity(rrs.len());
    let mut min_answer_ttl: Option<u32> = None;
    let mut soa_ttl: Option<u32> = None;
    let mut has_answers = false;
    for rr in &rrs {
        if rr.rtype == TYPE_OPT {
            continue;
        }
        if rr.ttl_offset + 4 <= u16::MAX as usize {
            ttl_offsets.push(rr.ttl_offset as u16);
        }
        if rr.section == 0 {
            has_answers = true;
            min_answer_ttl = Some(min_answer_ttl.map_or(rr.ttl, |m| m.min(rr.ttl)));
        }
        if rr.section == 1 && rr.rtype == TYPE_SOA {
            // negative caching: SOA MINIMUM field would be exact; TTL is close enough
            soa_ttl = Some(rr.ttl);
        }
    }
    let ttl = if has_answers && rc == RCODE_NOERROR {
        min_answer_ttl.unwrap_or(negative_ttl)
    } else {
        // NXDOMAIN or NODATA
        soa_ttl.map_or(negative_ttl, |s| s.min(negative_ttl))
    };
    Some(CacheInfo {
        ttl_offsets,
        ttl,
        has_answers,
    })
}

/// Age TTLs by `elapsed` seconds (floor 1) in a cached response copy.
pub fn age_ttls(buf: &mut [u8], offsets: &[u16], elapsed: u32) {
    for &off in offsets {
        let off = off as usize;
        if off + 4 > buf.len() {
            continue;
        }
        let ttl = u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
        let new = ttl.saturating_sub(elapsed).max(1);
        buf[off..off + 4].copy_from_slice(&new.to_be_bytes());
    }
}

/// Clamp every RR TTL into [min_ttl, max_ttl] (0 = no bound) in place.
pub fn clamp_ttls(buf: &mut [u8], question_end: usize, min_ttl: u32, max_ttl: u32) {
    if min_ttl == 0 && max_ttl == 0 {
        return;
    }
    let qdcount = match u16_at(buf, 4) {
        Some(q) => q as usize,
        None => return,
    };
    let rrs = match walk_rrs(buf, question_end, qdcount.saturating_sub(1)) {
        Some(r) => r,
        None => return,
    };
    for rr in rrs {
        if rr.rtype == TYPE_OPT {
            continue;
        }
        let mut ttl = rr.ttl;
        if min_ttl > 0 && ttl < min_ttl {
            ttl = min_ttl;
        }
        if max_ttl > 0 && ttl > max_ttl {
            ttl = max_ttl;
        }
        if ttl != rr.ttl {
            buf[rr.ttl_offset..rr.ttl_offset + 4].copy_from_slice(&ttl.to_be_bytes());
        }
    }
}

/// Extract A/AAAA addresses from a response.
pub fn extract_ips(buf: &[u8], question_end: usize) -> Vec<std::net::IpAddr> {
    let mut out = Vec::new();
    let qdcount = match u16_at(buf, 4) {
        Some(q) => q as usize,
        None => return out,
    };
    if let Some(rrs) = walk_rrs(buf, question_end, qdcount.saturating_sub(1)) {
        for rr in rrs {
            if rr.section != 0 {
                continue;
            }
            let rd = &buf[rr.rdata_start..rr.rdata_start + rr.rdata_len];
            if rr.rtype == TYPE_A && rr.rdata_len == 4 {
                out.push(std::net::IpAddr::from([rd[0], rd[1], rd[2], rd[3]]));
            } else if rr.rtype == TYPE_AAAA && rr.rdata_len == 16 {
                let mut a = [0u8; 16];
                a.copy_from_slice(rd);
                out.push(std::net::IpAddr::from(a));
            }
        }
    }
    out
}

/// Minimum answer TTL of a response (for redirect synthesis), default `dflt`.
pub fn min_answer_ttl(buf: &[u8], question_end: usize, dflt: u32) -> u32 {
    let qdcount = match u16_at(buf, 4) {
        Some(q) => q as usize,
        None => return dflt,
    };
    walk_rrs(buf, question_end, qdcount.saturating_sub(1))
        .map(|rrs| {
            rrs.iter()
                .filter(|r| r.section == 0 && r.rtype != TYPE_OPT)
                .map(|r| r.ttl)
                .min()
                .unwrap_or(dflt)
        })
        .unwrap_or(dflt)
}

/// Build a reply that echoes the query's question, with the given rcode and no records.
pub fn build_reply(query: &[u8], question_end: usize, rcode: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(question_end);
    out.extend_from_slice(&query[..HEADER_LEN.min(query.len())]);
    if query.len() >= question_end {
        out.extend_from_slice(&query[HEADER_LEN..question_end]);
    }
    // QR=1, keep OPCODE + RD, clear TC/AA; set RA
    out[2] = 0x80 | (query[2] & 0x79);
    out[3] = 0x80 | rcode; // RA set
    out[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT=1
    out[6..8].copy_from_slice(&0u16.to_be_bytes());
    out[8..10].copy_from_slice(&0u16.to_be_bytes());
    out[10..12].copy_from_slice(&0u16.to_be_bytes());
    out
}

/// Build a truncated (TC) empty reply telling the client to retry over TCP.
pub fn build_truncated(query: &[u8], question_end: usize) -> Vec<u8> {
    let mut out = build_reply(query, question_end, RCODE_NOERROR);
    out[2] |= 0x02;
    out
}

/// Build an answer with the given IPs for an A/AAAA query (hosts / redirect).
pub fn build_ip_reply(
    query: &[u8],
    meta: &QueryMeta,
    ips: &[std::net::IpAddr],
    ttl: u32,
) -> Vec<u8> {
    let mut out = build_reply(query, meta.question_end, RCODE_NOERROR);
    let mut count = 0u16;
    for ip in ips {
        let (rtype, rdata): (u16, Vec<u8>) = match ip {
            std::net::IpAddr::V4(v4) if meta.qtype == TYPE_A => (TYPE_A, v4.octets().to_vec()),
            std::net::IpAddr::V6(v6) if meta.qtype == TYPE_AAAA => {
                (TYPE_AAAA, v6.octets().to_vec())
            }
            _ => continue,
        };
        out.extend_from_slice(&[0xC0, 0x0C]); // pointer to qname at offset 12
        out.extend_from_slice(&rtype.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes()); // IN
        out.extend_from_slice(&ttl.to_be_bytes());
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(&rdata);
        count += 1;
    }
    out[6..8].copy_from_slice(&count.to_be_bytes());
    out
}

/// Build a fresh query for (name, qtype) with EDNS udp=1232. Returns None for bad names.
pub fn build_query(name: &str, qtype: u16, qclass: u16) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(name.len() + 32);
    let id: u16 = fastrand::u16(..);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00]); // RD
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // ARCOUNT (OPT)
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        if label.len() > 63 {
            return None;
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&qclass.to_be_bytes());
    // OPT: root name, type 41, class = udp size 1232, ttl 0, rdlen 0
    out.push(0);
    out.extend_from_slice(&TYPE_OPT.to_be_bytes());
    out.extend_from_slice(&1232u16.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_roundtrip() {
        let q = build_query("Example.COM", TYPE_A, 1).unwrap();
        let meta = parse_query(&q).unwrap();
        assert_eq!(meta.qname, "example.com");
        assert_eq!(meta.qtype, TYPE_A);
        assert_eq!(meta.udp_size, 1232);
        assert_eq!(meta.question_end, HEADER_LEN + 13 + 4);
    }

    fn sample_response() -> (Vec<u8>, usize) {
        // response to example.com A with two A answers + one SOA-ish authority
        let mut q = build_query("example.com", TYPE_A, 1).unwrap();
        q.truncate(q.len() - 11); // strip OPT
        q[10..12].copy_from_slice(&0u16.to_be_bytes());
        let question_end = q.len();
        let mut r = q.clone();
        r[2] = 0x81; // QR|RD
        r[3] = 0x80; // RA
        r[6..8].copy_from_slice(&2u16.to_be_bytes()); // ANCOUNT=2
        for (ttl, ip) in [(300u32, [1, 2, 3, 4]), (60u32, [5, 6, 7, 8])] {
            r.extend_from_slice(&[0xC0, 0x0C]);
            r.extend_from_slice(&TYPE_A.to_be_bytes());
            r.extend_from_slice(&1u16.to_be_bytes());
            r.extend_from_slice(&ttl.to_be_bytes());
            r.extend_from_slice(&4u16.to_be_bytes());
            r.extend_from_slice(&ip);
        }
        (r, question_end)
    }

    #[test]
    fn cacheinfo_and_aging() {
        let (mut r, qend) = sample_response();
        let info = cache_info(&r, qend, 30).unwrap();
        assert_eq!(info.ttl, 60);
        assert!(info.has_answers);
        assert_eq!(info.ttl_offsets.len(), 2);
        let offs = info.ttl_offsets.clone();
        age_ttls(&mut r, &offs, 100);
        let info2 = cache_info(&r, qend, 30).unwrap();
        assert_eq!(info2.ttl, 1); // 60 - 100 floors at 1
        let ips = extract_ips(&r, qend);
        assert_eq!(ips.len(), 2);
    }

    #[test]
    fn clamping() {
        let (mut r, qend) = sample_response();
        clamp_ttls(&mut r, qend, 120, 200);
        let info = cache_info(&r, qend, 30).unwrap();
        assert_eq!(info.ttl, 120); // 60 raised to 120, 300 lowered to 200
    }

    #[test]
    fn ip_reply() {
        let q = build_query("example.com", TYPE_A, 1).unwrap();
        let meta = parse_query(&q).unwrap();
        let r = build_ip_reply(&q, &meta, &["9.9.9.9".parse().unwrap()], 300);
        assert!(is_response(&r));
        assert_eq!(rcode(&r), RCODE_NOERROR);
        let ips = extract_ips(&r, meta.question_end);
        assert_eq!(ips, vec!["9.9.9.9".parse::<std::net::IpAddr>().unwrap()]);
    }

    #[test]
    fn nx_reply() {
        let q = build_query("blocked.example", TYPE_A, 1).unwrap();
        let meta = parse_query(&q).unwrap();
        let r = build_reply(&q, meta.question_end, RCODE_NXDOMAIN);
        assert_eq!(rcode(&r), RCODE_NXDOMAIN);
        assert!(is_response(&r));
        assert_eq!(parse_query(&r).unwrap().qname, "blocked.example");
    }
}
