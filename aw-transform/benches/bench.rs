use chrono::Duration;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use serde_json::json;
use serde_json::Map;
use serde_json::Value;

use aw_models::Event;
use aw_transform::*;

// TODO: Move me to an appropriate place
#[macro_export]
macro_rules! json_map {
    { $( $key:literal : $value:expr),* } => {{
        use serde_json::Value;
        use serde_json::map::Map;
        #[allow(unused_mut)]
        let mut map : Map<String, Value> = Map::new();
        $(
          map.insert( $key.to_string(), json!($value) );
        )*
        map
    }};
}

fn create_events(num_events: i64) -> Vec<Event> {
    let mut possible_data = Vec::<Map<String, Value>>::new();
    for i in 0..20 {
        possible_data.push(json_map! {"number": i});
    }
    let mut event_list = Vec::new();
    for i in 0..num_events {
        let e = Event {
            id: None,
            timestamp: chrono::Utc::now() + Duration::seconds(i),
            duration: Duration::seconds(10),
            data: possible_data[i as usize % 20].clone(),
        };
        event_list.push(e);
    }
    event_list
}

/// Events separated by a gap shorter than the `pulsetime` used by `bench_flood`,
/// with runs of equal data. `create_events` overlaps every event with its
/// neighbours, so it never reaches flood's gap-filling or same-data merging.
fn create_sparse_events(num_events: i64) -> Vec<Event> {
    let now = chrono::Utc::now();
    (0..num_events)
        .map(|i| {
            let number = (i / 3) % 2;
            Event {
                id: None,
                timestamp: now + Duration::seconds(i * 15),
                duration: Duration::seconds(10),
                data: json_map! {"number": number},
            }
        })
        .collect()
}

/// Events with runs of adjacent events sharing the same `number`, so
/// `chunk_events_by_key` has durations to aggregate. `create_events` cycles its
/// keys, so neighbouring events never compare equal.
fn create_chunkable_events(num_events: i64) -> Vec<Event> {
    let now = chrono::Utc::now();
    (0..num_events)
        .map(|i| {
            let number = (i / 5) % 10;
            Event {
                id: None,
                timestamp: now + Duration::seconds(i * 10),
                duration: Duration::seconds(10),
                data: json_map! {"number": number},
            }
        })
        .collect()
}

/// Deterministic xorshift shuffle, so the sort benchmark gets genuinely
/// non-monotone input without pulling in a random-number dependency.
fn shuffle<T>(v: &mut [T], mut seed: u64) {
    for i in (1..v.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.swap(i, (seed % (i as u64 + 1)) as usize);
    }
}

fn bench_filter_period_intersect(c: &mut Criterion) {
    let events2 = create_events(1000);
    c.bench_function("1000 events", |b| {
        b.iter(|| {
            let events1 = create_events(1000);
            filter_period_intersect(events1, events2.clone());
        })
    });
}

fn bench_union_no_overlap(c: &mut Criterion) {
    let mut group = c.benchmark_group("union_no_overlap");
    for n in [1_000, 8_000, 32_000] {
        let now = chrono::Utc::now();
        let first: Vec<_> = (0..n)
            .map(|i| {
                Event::new(
                    now + Duration::seconds(i * 4 + 1),
                    Duration::seconds(1),
                    json_map! {"app": "foreground"},
                )
            })
            .collect();
        let second: Vec<_> = (0..n)
            .map(|i| {
                Event::new(
                    now + Duration::seconds(i * 4),
                    Duration::seconds(3),
                    json_map! {"app": "background"},
                )
            })
            .collect();
        group.bench_function(BenchmarkId::new("overlapping", n), |b| {
            b.iter_batched(
                || (first.clone(), second.clone()),
                |(a, b)| union_no_overlap(a, b),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_merge_events_by_keys(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_events_by_keys");
    for distinct in [100, 50_000] {
        let mut events = create_events(50_000);
        for (i, event) in events.iter_mut().enumerate() {
            event.data = json_map! {
                "app": "browser",
                "title": format!("Page {}", i % distinct),
                "url": "https://example.com/a/long/path"
            };
        }
        group.bench_with_input(
            BenchmarkId::new("distinct", distinct),
            &events,
            |b, events| {
                b.iter_batched(
                    || (events.clone(), vec!["app".into(), "title".into()]),
                    |(events, keys)| merge_events_by_keys(events, keys),
                    BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

const SIZES: [i64; 3] = [1_000, 10_000, 100_000];

fn bench_flood(c: &mut Criterion) {
    let mut group = c.benchmark_group("flood");
    for n in SIZES {
        let events = create_sparse_events(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &events, |b, events| {
            b.iter_batched(
                || events.clone(),
                |events| flood(events, Duration::seconds(5)),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_sort_by_timestamp(c: &mut Criterion) {
    let mut group = c.benchmark_group("sort_by_timestamp");
    for n in SIZES {
        // A reversed input is one descending run, which the adaptive sort
        // handles in near-linear time; it is kept as a best case.
        let mut reversed = create_events(n);
        reversed.reverse();
        group.bench_with_input(BenchmarkId::new("reversed", n), &reversed, |b, events| {
            b.iter_batched(|| events.clone(), sort_by_timestamp, BatchSize::LargeInput);
        });

        // Shuffled input mixes ascending and descending runs, so the sort
        // cannot take its cheap nearly-sorted path.
        let mut shuffled = create_events(n);
        shuffle(&mut shuffled, 0x9E37_79B9_7F4A_7C15);
        group.bench_with_input(BenchmarkId::new("shuffled", n), &shuffled, |b, events| {
            b.iter_batched(|| events.clone(), sort_by_timestamp, BatchSize::LargeInput);
        });
    }
    group.finish();
}

fn bench_filter_keyvals(c: &mut Criterion) {
    let mut group = c.benchmark_group("filter_keyvals");
    let vals = [json!(1), json!(2), json!(3)];
    for n in SIZES {
        let events = create_events(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &events, |b, events| {
            b.iter_batched(
                || events.clone(),
                |events| filter_keyvals(events, "number", &vals),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_chunk_events_by_key(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunk_events_by_key");
    for n in SIZES {
        let events = create_chunkable_events(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &events, |b, events| {
            b.iter_batched(
                || events.clone(),
                |events| chunk_events_by_key(events, "number"),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_filter_period_intersect,
    bench_union_no_overlap,
    bench_merge_events_by_keys,
    bench_flood,
    bench_sort_by_timestamp,
    bench_filter_keyvals,
    bench_chunk_events_by_key
);
criterion_main!(benches);
