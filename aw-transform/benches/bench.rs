use chrono::Duration;
use criterion::{criterion_group, criterion_main, Criterion};
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

criterion_group!(benches, bench_filter_period_intersect);
criterion_main!(benches);
