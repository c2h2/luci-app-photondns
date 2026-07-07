//! Sharded LRU response cache with serve-stale and prefetch support.

use crate::dns;
use lru::LruCache;
use parking_lot::Mutex;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SHARDS: usize = 16;

pub type CacheKey = String; // "qname\0qtype\0qclass"

pub fn make_key(qname: &str, qtype: u16, qclass: u16) -> CacheKey {
    format!("{}\u{0}{}\u{0}{}", qname, qtype, qclass)
}

pub struct CacheEntry {
    /// full wire response (some upstream-assigned ID)
    pub data: Vec<u8>,
    /// offsets of TTL fields to age
    pub ttl_offsets: Box<[u16]>,
    /// wire length of the question section (bytes 12..12+len)
    pub question_len: u16,
    pub stored_at: Instant,
    /// seconds of freshness from stored_at
    pub ttl: u32,
    /// seconds past expiry the entry may still be served stale
    pub stale_ttl: u32,
    pub hits: AtomicU32,
    pub refreshing: AtomicBool,
    /// unix seconds when stored (for dump/restore)
    pub stored_unix: u64,
}

pub enum Freshness {
    Fresh { remaining: u32 },
    Stale,
}

impl CacheEntry {
    pub fn elapsed_secs(&self, now: Instant) -> u32 {
        now.duration_since(self.stored_at).as_secs() as u32
    }

    pub fn freshness(&self, now: Instant) -> Option<Freshness> {
        let elapsed = self.elapsed_secs(now);
        if elapsed < self.ttl {
            Some(Freshness::Fresh {
                remaining: self.ttl - elapsed,
            })
        } else if elapsed < self.ttl.saturating_add(self.stale_ttl) {
            Some(Freshness::Stale)
        } else {
            None
        }
    }

    /// Materialize a response for `query`: copy stored bytes, restore the
    /// client's ID and exact question spelling (0x20 case), age TTLs.
    /// Stale entries are stamped with `stale_client_ttl` instead of the aged
    /// floor of 1 so clients can cache them briefly (RFC 8767).
    pub fn make_response(
        &self,
        query: &[u8],
        meta: &dns::QueryMeta,
        now: Instant,
        stale_client_ttl: u32,
    ) -> Vec<u8> {
        let mut out = self.data.clone();
        dns::set_id(&mut out, meta.id);
        let qlen = meta.question_end - dns::HEADER_LEN;
        if qlen == self.question_len as usize && out.len() >= meta.question_end {
            out[dns::HEADER_LEN..meta.question_end]
                .copy_from_slice(&query[dns::HEADER_LEN..meta.question_end]);
        }
        let elapsed = self.elapsed_secs(now);
        if elapsed >= self.ttl {
            dns::set_ttls(&mut out, &self.ttl_offsets, stale_client_ttl.max(1));
        } else {
            dns::age_ttls(&mut out, &self.ttl_offsets, elapsed);
        }
        out
    }
}

struct Shard {
    map: LruCache<CacheKey, Arc<CacheEntry>>,
}

pub struct DnsCache {
    shards: Vec<Mutex<Shard>>,
    pub capacity: usize,
    pub inserts: AtomicU64,
    pub evictions: AtomicU64,
}

impl DnsCache {
    pub fn new(capacity: usize) -> Self {
        let per_shard = (capacity + SHARDS - 1) / SHARDS;
        let shards = (0..SHARDS)
            .map(|_| {
                Mutex::new(Shard {
                    map: LruCache::new(NonZeroUsize::new(per_shard.max(1)).unwrap()),
                })
            })
            .collect();
        Self {
            shards,
            capacity,
            inserts: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    fn shard(&self, key: &CacheKey) -> &Mutex<Shard> {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        &self.shards[(h.finish() as usize) % SHARDS]
    }

    /// Get an entry; expired-beyond-stale entries are removed.
    pub fn get(&self, key: &CacheKey, now: Instant) -> Option<(Arc<CacheEntry>, Freshness)> {
        let mut shard = self.shard(key).lock();
        let entry = shard.map.get(key)?.clone();
        match entry.freshness(now) {
            Some(f) => {
                entry.hits.fetch_add(1, Ordering::Relaxed);
                Some((entry, f))
            }
            None => {
                shard.map.pop(key);
                None
            }
        }
    }

    pub fn remove(&self, key: &CacheKey) {
        self.shard(key).lock().map.pop(key);
    }

    pub fn insert(&self, key: CacheKey, entry: CacheEntry) {
        let mut shard = self.shard(&key).lock();
        if shard.map.len() == shard.map.cap().get() && !shard.map.contains(&key) {
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
        shard.map.put(key, Arc::new(entry));
        self.inserts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn flush(&self) {
        for s in &self.shards {
            s.lock().map.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.lock().map.len()).sum()
    }

    /// Persist non-expired entries. Format:
    /// magic "PHOTONDC" u8 version, u64 count, then per entry:
    /// u16 key_len, key, u64 stored_unix, u32 ttl, u32 stale_ttl,
    /// u16 question_len, u16 n_offsets, offsets..., u32 data_len, data.
    pub fn dump(&self, path: &str) -> std::io::Result<usize> {
        use std::io::Write;
        let now = Instant::now();
        let mut entries: Vec<(CacheKey, Arc<CacheEntry>)> = Vec::new();
        for s in &self.shards {
            let shard = s.lock();
            for (k, v) in shard.map.iter() {
                if v.freshness(now).is_some() {
                    entries.push((k.clone(), v.clone()));
                }
            }
        }
        let tmp = format!("{}.tmp", path);
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        f.write_all(b"PHOTONDC")?;
        f.write_all(&[1u8])?;
        f.write_all(&(entries.len() as u64).to_be_bytes())?;
        for (k, e) in &entries {
            f.write_all(&(k.len() as u16).to_be_bytes())?;
            f.write_all(k.as_bytes())?;
            f.write_all(&e.stored_unix.to_be_bytes())?;
            f.write_all(&e.ttl.to_be_bytes())?;
            f.write_all(&e.stale_ttl.to_be_bytes())?;
            f.write_all(&e.question_len.to_be_bytes())?;
            f.write_all(&(e.ttl_offsets.len() as u16).to_be_bytes())?;
            for off in e.ttl_offsets.iter() {
                f.write_all(&off.to_be_bytes())?;
            }
            f.write_all(&(e.data.len() as u32).to_be_bytes())?;
            f.write_all(&e.data)?;
        }
        f.into_inner()?.sync_all().ok();
        std::fs::rename(&tmp, path)?;
        Ok(entries.len())
    }

    pub fn load(&self, path: &str) -> std::io::Result<usize> {
        let data = std::fs::read(path)?;
        let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "bad cache dump");
        if data.len() < 17 || &data[0..8] != b"PHOTONDC" || data[8] != 1 {
            return Err(bad());
        }
        let unix_now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let now = Instant::now();
        let count = u64::from_be_bytes(data[9..17].try_into().unwrap());
        let mut pos = 17usize;
        let mut loaded = 0usize;
        for _ in 0..count {
            let need = |n: usize, pos: usize| -> std::io::Result<()> {
                if pos + n > data.len() {
                    Err(bad())
                } else {
                    Ok(())
                }
            };
            need(2, pos)?;
            let klen = u16::from_be_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            need(klen + 20, pos)?;
            let key = String::from_utf8_lossy(&data[pos..pos + klen]).into_owned();
            pos += klen;
            let stored_unix = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
            pos += 8;
            let ttl = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let stale_ttl = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let question_len = u16::from_be_bytes(data[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let noff = u16::from_be_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            need(noff * 2 + 4, pos)?;
            let mut offsets = Vec::with_capacity(noff);
            for _ in 0..noff {
                offsets.push(u16::from_be_bytes(data[pos..pos + 2].try_into().unwrap()));
                pos += 2;
            }
            let dlen = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            need(dlen, pos)?;
            let bytes = data[pos..pos + dlen].to_vec();
            pos += dlen;

            let age = unix_now.saturating_sub(stored_unix) as u32;
            if age >= ttl.saturating_add(stale_ttl) {
                continue; // fully expired
            }
            let entry = CacheEntry {
                data: bytes,
                ttl_offsets: offsets.into_boxed_slice(),
                question_len,
                stored_at: now.checked_sub(Duration::from_secs(age as u64)).unwrap_or(now),
                ttl,
                stale_ttl,
                hits: AtomicU32::new(0),
                refreshing: AtomicBool::new(false),
                stored_unix,
            };
            self.insert(key, entry);
            loaded += 1;
        }
        Ok(loaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns;

    fn entry_for(name: &str, ttl: u32, stale: u32) -> (CacheKey, CacheEntry) {
        let q = dns::build_query(name, dns::TYPE_A, 1).unwrap();
        let meta = dns::parse_query(&q).unwrap();
        let resp = dns::build_ip_reply(&q, &meta, &["1.2.3.4".parse().unwrap()], ttl);
        let info = dns::cache_info(&resp, meta.question_end, 30).unwrap();
        let key = make_key(&meta.qname, meta.qtype, meta.qclass);
        (
            key,
            CacheEntry {
                data: resp,
                ttl_offsets: info.ttl_offsets.into_boxed_slice(),
                question_len: (meta.question_end - dns::HEADER_LEN) as u16,
                stored_at: Instant::now(),
                ttl: info.ttl,
                stale_ttl: stale,
                hits: AtomicU32::new(0),
                refreshing: AtomicBool::new(false),
                stored_unix: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            },
        )
    }

    #[test]
    fn insert_get_flush() {
        let cache = DnsCache::new(100);
        let (key, entry) = entry_for("a.example.com", 300, 3600);
        cache.insert(key.clone(), entry);
        assert_eq!(cache.len(), 1);
        let (e, f) = cache.get(&key, Instant::now()).unwrap();
        assert!(matches!(f, Freshness::Fresh { .. }));
        assert_eq!(e.hits.load(Ordering::Relaxed), 1);
        cache.flush();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn stale_and_expiry() {
        let cache = DnsCache::new(100);
        let (key, mut entry) = entry_for("b.example.com", 1, 5);
        entry.stored_at = Instant::now() - Duration::from_secs(3);
        cache.insert(key.clone(), entry);
        let (_, f) = cache.get(&key, Instant::now()).unwrap();
        assert!(matches!(f, Freshness::Stale));
        // beyond stale window -> gone
        let (key2, mut e2) = entry_for("c.example.com", 1, 2);
        e2.stored_at = Instant::now() - Duration::from_secs(10);
        cache.insert(key2.clone(), e2);
        assert!(cache.get(&key2, Instant::now()).is_none());
    }

    #[test]
    fn response_materialization_cases_ids() {
        let cache = DnsCache::new(10);
        let (key, entry) = entry_for("mixed.example.com", 300, 0);
        cache.insert(key.clone(), entry);
        // client asks with different case + different id
        let mut q2 = dns::build_query("MiXeD.eXample.COM", dns::TYPE_A, 1).unwrap();
        dns::set_id(&mut q2, 0xBEEF);
        let meta2 = dns::parse_query(&q2).unwrap();
        let (e, _) = cache.get(&key, Instant::now()).unwrap();
        let resp = e.make_response(&q2, &meta2, Instant::now(), 30);
        assert_eq!(dns::get_id(&resp), 0xBEEF);
        // echoed question preserves client's exact bytes
        assert_eq!(
            &resp[dns::HEADER_LEN..meta2.question_end],
            &q2[dns::HEADER_LEN..meta2.question_end]
        );
    }

    #[test]
    fn stale_response_stamped_with_client_ttl() {
        let cache = DnsCache::new(10);
        let (key, mut entry) = entry_for("stale.example.com", 60, 3600);
        entry.stored_at = Instant::now() - Duration::from_secs(120);
        cache.insert(key.clone(), entry);
        let q = dns::build_query("stale.example.com", dns::TYPE_A, 1).unwrap();
        let meta = dns::parse_query(&q).unwrap();
        let (e, f) = cache.get(&key, Instant::now()).unwrap();
        assert!(matches!(f, Freshness::Stale));
        let resp = e.make_response(&q, &meta, Instant::now(), 30);
        let info = dns::cache_info(&resp, meta.question_end, 30).unwrap();
        assert_eq!(info.ttl, 30);
        // fresh entries still age normally
        let (key2, entry2) = entry_for("fresh.example.com", 300, 3600);
        cache.insert(key2.clone(), entry2);
        let q2 = dns::build_query("fresh.example.com", dns::TYPE_A, 1).unwrap();
        let meta2 = dns::parse_query(&q2).unwrap();
        let (e2, _) = cache.get(&key2, Instant::now()).unwrap();
        let resp2 = e2.make_response(&q2, &meta2, Instant::now(), 30);
        let info2 = dns::cache_info(&resp2, meta2.question_end, 30).unwrap();
        assert!(info2.ttl > 290);
    }

    #[test]
    fn dump_and_load() {
        let dir = std::env::temp_dir().join("photondns-test-dump");
        let path = dir.to_str().unwrap().to_string();
        let cache = DnsCache::new(100);
        for i in 0..5 {
            let (k, e) = entry_for(&format!("host{}.example.com", i), 600, 600);
            cache.insert(k, e);
        }
        let n = cache.dump(&path).unwrap();
        assert_eq!(n, 5);
        let cache2 = DnsCache::new(100);
        let loaded = cache2.load(&path).unwrap();
        assert_eq!(loaded, 5);
        assert_eq!(cache2.len(), 5);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn lru_eviction() {
        let cache = DnsCache::new(16); // 1 per shard
        for i in 0..64 {
            let (k, e) = entry_for(&format!("h{}.example.com", i), 300, 0);
            cache.insert(k, e);
        }
        assert!(cache.len() <= 16);
        assert!(cache.evictions.load(Ordering::Relaxed) > 0);
    }
}
