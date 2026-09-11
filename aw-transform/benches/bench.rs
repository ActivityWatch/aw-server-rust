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

criterion_group!(
    benches,
    bench_filter_period_intersect,
    bench_union_no_overlap
);
criterion_main!(benches);
