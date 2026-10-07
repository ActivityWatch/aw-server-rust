//! In-memory cache for query results over finished (past) timeperiods.
//!
//! Port of `aw_server/query_cache.py` from ActivityWatch/aw-server#174.
//!
//! Clients such as aw-webui split long views (Year, All time) into one request
//! per day, and every page load recomputes all of them. Past days rarely
//! change, so we keep their results, keyed by (query text, timeperiod).
//!
//! Correctness rests on invalidation, not on "the past never changes":
//!
//! - Queries only read data through the bucket list and period-bounded event
//!   reads, which return events whose extent overlaps the period.
//! - Every write goes through the endpoint handlers in `bucket.rs` /
//!   `import.rs`. After each write we record the time range it affected (the
//!   full extent of every inserted, replaced, merged or deleted event) and drop
//!   cached entries whose period overlaps it. Bucket create/delete/import
//!   clears the cache, since they can change what the bucket list resolves to.
//! - Race: a query that started before an overlapping write finished may have
//!   read old data. Each computation records the write generation it started
//!   at, and `put` refuses to store if any overlapping write happened since (or
//!   if the write log no longer reaches back that far).
//!
//! Only periods that ended at least `margin` ago are cached, so the current
//! hour and today are always computed fresh. The cache is in-memory only: a
//! restart starts empty, which also covers any write that bypassed the API
//! (e.g. editing the database file directly while the server was stopped).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};

use aw_models::Event;

/// A closed time range `(start, end)`, both in UTC.
pub type TimeRange = (DateTime<Utc>, DateTime<Utc>);

/// Full extent of an event, as the datastore's overlap check sees it.
pub fn event_range(event: &Event) -> TimeRange {
    (event.timestamp, event.calculate_endtime())
}

/// Inclusive on both ends: conservative (may invalidate a touching period).
fn overlaps(a: &TimeRange, b: &TimeRange) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// Merge overlapping/touching ranges. Past `max_ranges`, fall back to one
/// bounding range: over-invalidating after a big import is fine, scanning the
/// cache once per imported event is not.
pub fn coalesce(mut ranges: Vec<TimeRange>, max_ranges: usize) -> Vec<TimeRange> {
    ranges.sort();
    let mut merged: Vec<TimeRange> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => {
                if end > last.1 {
                    last.1 = end;
                }
            }
            _ => merged.push((start, end)),
        }
    }
    if merged.len() > max_ranges {
        let start = merged[0].0;
        let end = merged.iter().map(|r| r.1).max().unwrap();
        return vec![(start, end)];
    }
    merged
}

/// Exact cache identity: query text plus the requested period.
///
/// Whitespace inside the query can be significant (string literals), so the
/// text is used verbatim.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CacheKey {
    query: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

impl CacheKey {
    pub fn new(query: &str, period: TimeRange) -> Self {
        Self {
            query: query.to_string(),
            start: period.0,
            end: period.1,
        }
    }

    /// Fixed overhead of the key (timestamps only). Query text is not charged:
    /// the same ~38 KB webui query repeats for every day in long views and would
    /// exhaust the 128 MB budget before a single All-time load completes. This
    /// matches `aw_server/query_cache.py`, which counts only `len(json.dumps(result))`.
    fn weight(&self) -> usize {
        2 * std::mem::size_of::<DateTime<Utc>>()
    }
}

struct Entry {
    period: TimeRange,
    /// The result, already serialized to JSON: it is produced once and reused
    /// for both the size accounting and the response body.
    body: Arc<str>,
    size: usize,
    last_used: u64,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<CacheKey, Entry>,
    bytes: usize,
    /// Incremented once per write call, however many events it touched.
    generation: u64,
    /// Monotonic tick used for LRU ordering.
    clock: u64,
    /// `(generation, affected ranges)` of recent writes, oldest first.
    writes: VecDeque<(u64, Vec<TimeRange>)>,
    hits: u64,
    misses: u64,
}

/// Bounded cache of query results for finished past periods.
///
/// Thread-safe: all state sits behind a `Mutex`, so the cache can live in
/// `ServerState` and be shared by every request.
pub struct QueryCache {
    max_entries: usize,
    max_bytes: usize,
    margin: Duration,
    write_log_size: usize,
    inner: Mutex<Inner>,
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryCache {
    pub fn new() -> Self {
        Self::with_limits(10_000, 128 * 1024 * 1024, Duration::minutes(10), 10_000)
    }

    pub fn with_limits(
        max_entries: usize,
        max_bytes: usize,
        margin: Duration,
        write_log_size: usize,
    ) -> Self {
        Self {
            max_entries,
            max_bytes,
            margin,
            write_log_size: write_log_size.max(1),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Whether a period ended long enough ago to be cached. The current hour
    /// and today are always computed fresh.
    pub fn cacheable(&self, period: TimeRange) -> bool {
        self.cacheable_at(period, Utc::now())
    }

    pub fn cacheable_at(&self, period: TimeRange, now: DateTime<Utc>) -> bool {
        period.1 <= now - self.margin
    }

    pub fn generation(&self) -> u64 {
        self.inner.lock().unwrap().generation
    }

    /// Return the cached result, serialized. It is shared, not copied.
    pub fn get(&self, key: &CacheKey) -> Option<Arc<str>> {
        let mut inner = self.inner.lock().unwrap();
        inner.clock += 1;
        let clock = inner.clock;
        let body = match inner.entries.get_mut(key) {
            Some(entry) => {
                entry.last_used = clock;
                Some(Arc::clone(&entry.body))
            }
            None => None,
        };
        if body.is_some() {
            inner.hits += 1;
        } else {
            inner.misses += 1;
        }
        body
    }

    /// Store `serialized` unless a write overlapping `period` happened after
    /// `started_generation`. Returns whether the entry was stored.
    pub fn put(
        &self,
        key: CacheKey,
        period: TimeRange,
        serialized: Arc<str>,
        started_generation: u64,
    ) -> bool {
        let size = serialized.len().saturating_add(key.weight());
        if size > self.max_bytes {
            return false;
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.generation != started_generation {
            let oldest_logged = inner
                .writes
                .front()
                .map(|(gen, _)| *gen)
                .unwrap_or(inner.generation + 1);
            if oldest_logged > started_generation + 1 {
                // write log doesn't reach back far enough to tell
                return false;
            }
            for (gen, affected) in inner.writes.iter() {
                if *gen > started_generation && affected.iter().any(|r| overlaps(r, &period)) {
                    return false;
                }
            }
        }
        inner.clock += 1;
        let clock = inner.clock;
        if let Some(old) = inner.entries.remove(&key) {
            inner.bytes -= old.size;
        }
        inner.bytes += size;
        inner.entries.insert(
            key,
            Entry {
                period,
                body: serialized,
                size,
                last_used: clock,
            },
        );
        inner.evict(self.max_entries, self.max_bytes);
        true
    }

    /// Record writes affecting `ranges` and drop overlapping entries. Call
    /// after the write.
    pub fn invalidate(&self, ranges: Vec<TimeRange>) {
        let ranges = coalesce(ranges, 64);
        if ranges.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        let generation = inner.generation;
        if inner.writes.len() == self.write_log_size {
            inner.writes.pop_front();
        }
        inner.writes.push_back((generation, ranges.clone()));
        let stale: Vec<CacheKey> = inner
            .entries
            .iter()
            .filter(|(_, entry)| ranges.iter().any(|r| overlaps(r, &entry.period)))
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            if let Some(entry) = inner.entries.remove(&key) {
                inner.bytes -= entry.size;
            }
        }
    }

    /// Drop everything (bucket list changed). Also blocks in-flight stores.
    pub fn clear(&self) {
        self.invalidate(vec![(DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC)]);
        let mut inner = self.inner.lock().unwrap();
        inner.entries.clear();
        inner.bytes = 0;
    }

    pub fn stats(&self) -> QueryCacheStats {
        let inner = self.inner.lock().unwrap();
        QueryCacheStats {
            entries: inner.entries.len(),
            bytes: inner.bytes,
            hits: inner.hits,
            misses: inner.misses,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryCacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub hits: u64,
    pub misses: u64,
}

impl Inner {
    fn evict(&mut self, max_entries: usize, max_bytes: usize) {
        while (self.entries.len() > max_entries || self.bytes > max_bytes)
            && !self.entries.is_empty()
        {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            match victim {
                Some(key) => {
                    if let Some(entry) = self.entries.remove(&key) {
                        self.bytes -= entry.size;
                    }
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap()
    }

    fn period(day: u32) -> TimeRange {
        (dt(day, 0, 0), dt(day + 1, 0, 0))
    }

    fn result(value: f64) -> Arc<str> {
        Arc::from(
            serde_json::to_string(&aw_query::DataType::Number(value))
                .unwrap()
                .as_str(),
        )
    }

    #[test]
    fn only_periods_past_the_margin_are_cacheable() {
        let cache = QueryCache::new();
        let now = dt(10, 12, 0);
        // ended 2h ago -> cacheable
        assert!(cache.cacheable_at((dt(10, 8, 0), dt(10, 10, 0)), now));
        // current hour -> never cached
        assert!(!cache.cacheable_at((dt(10, 11, 0), dt(10, 12, 30)), now));
        // ended 5 min ago -> inside the margin, not cached
        assert!(!cache.cacheable_at((dt(10, 10, 0), dt(10, 11, 55)), now));
    }

    #[test]
    fn put_then_get_roundtrips() {
        let cache = QueryCache::new();
        let key = CacheKey::new("RETURN = 1;", period(1));
        let gen = cache.generation();
        assert!(cache.put(key.clone(), period(1), result(1.0), gen));
        let hit = cache.get(&key).expect("cached");
        assert_eq!(&*hit, "1.0");
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn overlapping_write_invalidates_entry() {
        let cache = QueryCache::new();
        let key = CacheKey::new("RETURN = 1;", period(1));
        let gen = cache.generation();
        cache.put(key.clone(), period(1), result(1.0), gen);
        // a write well past the cached day does not overlap it
        cache.invalidate(vec![(dt(5, 0, 0), dt(5, 1, 0))]);
        assert!(cache.get(&key).is_some());
        // write that overlaps day 1 drops it
        cache.invalidate(vec![(dt(1, 10, 0), dt(1, 12, 0))]);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn store_is_refused_when_an_overlapping_write_raced_it() {
        let cache = QueryCache::new();
        let started = cache.generation();
        cache.invalidate(vec![(dt(1, 10, 0), dt(1, 12, 0))]);
        let key = CacheKey::new("RETURN = 1;", period(1));
        assert!(!cache.put(key, period(1), result(1.0), started));
    }

    #[test]
    fn store_succeeds_when_the_racing_write_does_not_overlap() {
        let cache = QueryCache::new();
        let started = cache.generation();
        cache.invalidate(vec![(dt(5, 0, 0), dt(5, 1, 0))]);
        let key = CacheKey::new("RETURN = 1;", period(1));
        assert!(cache.put(key, period(1), result(1.0), started));
    }

    #[test]
    fn explicit_clear_drops_everything() {
        let cache = QueryCache::new();
        let key = CacheKey::new("RETURN = 1;", period(1));
        let gen = cache.generation();
        cache.put(key.clone(), period(1), result(1.0), gen);
        cache.clear();
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().bytes, 0);
    }

    #[test]
    fn entry_limit_evicts_least_recently_used() {
        let cache = QueryCache::with_limits(2, 128 * 1024 * 1024, Duration::minutes(10), 100);
        let key_a = CacheKey::new("a", period(1));
        let key_b = CacheKey::new("b", period(1));
        let key_c = CacheKey::new("c", period(1));
        let gen = cache.generation();
        cache.put(key_a.clone(), period(1), result(1.0), gen);
        cache.put(key_b.clone(), period(1), result(2.0), gen);
        // touch a so b becomes the LRU victim
        assert!(cache.get(&key_a).is_some());
        cache.put(key_c.clone(), period(1), result(3.0), gen);
        assert_eq!(cache.stats().entries, 2);
        assert!(cache.get(&key_b).is_none());
        assert!(cache.get(&key_a).is_some());
        assert!(cache.get(&key_c).is_some());
    }

    #[test]
    fn byte_limit_is_enforced_by_result_size_not_query_text() {
        // Room for entries of a couple of bytes plus the ~32-byte key timestamp overhead.
        let cache = QueryCache::with_limits(100, 64, Duration::minutes(10), 100);
        let gen = cache.generation();
        cache.put(CacheKey::new("a", period(1)), period(1), result(1.0), gen);
        cache.put(CacheKey::new("b", period(1)), period(1), result(2.0), gen);
        assert!(cache.stats().bytes <= 64);

        // A large query text does NOT count against the budget; only result size does.
        // This is the fix for the All-time view getting 0% cache hits: the 38 KB webui
        // query was charging ~130 MB for 3,417 days, exceeding the 128 MB max_bytes.
        let large_query = CacheKey::new(&"x".repeat(4096), period(3));
        assert!(cache.put(large_query, period(3), result(1.0), gen));

        // But a large *result* that exceeds max_bytes on its own is still rejected.
        let large_result: Arc<str> = Arc::from("x".repeat(65).as_str());
        let big = CacheKey::new("q", period(4));
        assert!(!cache.put(big, period(4), large_result, gen));
    }

    #[test]
    fn all_time_view_fits_budget_with_large_query_text() {
        // Regression for #784: 3,417 daily entries × 38 KB query > 128 MB key budget → 0% hits.
        // After the fix, key weight charges only timestamps (~32 B), so 3,417 small results fit.
        let cache = QueryCache::new(); // 128 MB budget
        let base = dt(1, 0, 0);
        let query = "x".repeat(38 * 1024); // ~38 KB webui fullDesktopQuery
        let gen = cache.generation();
        let mut stored = 0usize;
        for i in 0..3417u64 {
            let start = base + Duration::days(i as i64);
            let end = start + Duration::days(1);
            let key = CacheKey::new(&query, (start, end));
            // result is tiny: a JSON number, ~5 bytes
            if cache.put(key, (start, end), result(1.0), gen) {
                stored += 1;
            }
        }
        assert_eq!(
            stored, 3417,
            "All 3,417 daily entries must fit the 128 MB budget"
        );
        assert_eq!(cache.stats().entries, 3417);
    }

    #[test]
    fn coalesce_merges_touching_and_bounds_the_result() {
        let merged = coalesce(
            vec![
                (dt(1, 0, 0), dt(1, 1, 0)),
                (dt(1, 1, 0), dt(1, 2, 0)),
                (dt(1, 4, 0), dt(1, 5, 0)),
            ],
            64,
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0], (dt(1, 0, 0), dt(1, 2, 0)));

        let folded = coalesce(
            vec![
                (dt(1, 0, 0), dt(1, 1, 0)),
                (dt(2, 0, 0), dt(2, 1, 0)),
                (dt(3, 0, 0), dt(3, 1, 0)),
            ],
            2,
        );
        assert_eq!(folded, vec![(dt(1, 0, 0), dt(3, 1, 0))]);
    }
}
