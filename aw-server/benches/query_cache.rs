//! Profiling evidence for the finished-period query cache.
//!
//! Measures the work the cache actually avoids: evaluating a query for one
//! finished day of real events against the datastore, versus serving the same
//! request from the cache. This is the workload aw-webui issues once per day
//! for the Year and All time views.
//!
//! Run with: `cargo bench -p aw-server --bench query_cache`

use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::{json, Map, Value};

use aw_datastore::Datastore;
use aw_models::{Bucket, BucketMetadata, Event, TimeInterval};
use aw_server::endpoints::query_cache::{CacheKey, QueryCache};

const EVENTS: i64 = 5_000;

fn setup() -> (Datastore, TimeInterval) {
    let datastore = Datastore::new_in_memory(false);
    let bucket = Bucket {
        bid: None,
        id: "bench".into(),
        _type: "test".into(),
        client: "test".into(),
        hostname: "test".into(),
        created: Some(Utc::now()),
        data: Map::new(),
        metadata: BucketMetadata::default(),
        events: None,
        last_updated: None,
    };
    datastore.create_bucket(&bucket).unwrap();

    let day_start = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
    let data: Map<String, Value> = [("app".to_string(), json!("bench"))].into_iter().collect();
    let events: Vec<Event> = (0..EVENTS)
        .map(|i| Event {
            id: None,
            timestamp: day_start + Duration::seconds(i * 10),
            duration: Duration::seconds(10),
            data: data.clone(),
        })
        .collect();
    datastore.insert_events("bench", &events).unwrap();

    (
        datastore,
        TimeInterval::new(day_start, day_start + Duration::days(1)),
    )
}

fn bench_query_cache(c: &mut Criterion) {
    let (datastore, interval) = setup();
    let code = "events = query_bucket(\"bench\");\nRETURN = events;";
    let period = (interval.start().to_owned(), interval.end().to_owned());

    // Warm the baseline result the cache would store.
    let result = aw_query::query(code, &interval, &datastore).unwrap();
    let serialized: Arc<str> = Arc::from(serde_json::to_string(&result).unwrap().as_str());

    let cache = QueryCache::new();
    let key = CacheKey::new(code, period);
    cache.put(
        key.clone(),
        period,
        Arc::clone(&serialized),
        cache.generation(),
    );

    let mut group = c.benchmark_group("query_cache");
    group.bench_function("cold_query", |b| {
        b.iter(|| aw_query::query(code, &interval, &datastore).unwrap())
    });
    group.bench_function("cache_hit", |b| b.iter(|| cache.get(&key)));
    // Miss path: evaluate, serialize once (the same string is stored and
    // returned), and store.
    group.bench_function("miss_evaluate_and_store", |b| {
        b.iter(|| {
            let result = aw_query::query(code, &interval, &datastore).unwrap();
            let serialized: Arc<str> = Arc::from(serde_json::to_string(&result).unwrap().as_str());
            cache.put(
                CacheKey::new(code, period),
                period,
                serialized,
                cache.generation(),
            );
        })
    });
    group.finish();
}

criterion_group!(benches, bench_query_cache);
criterion_main!(benches);
