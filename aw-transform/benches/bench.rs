#![feature(test)]
extern crate aw_models;
extern crate aw_transform;
extern crate serde_json;
extern crate test;

use chrono::Duration;
use serde_json::json;
use serde_json::Map;
use serde_json::Value;
use test::Bencher;

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

#[bench]
fn bench_filter_period_intersect(b: &mut Bencher) {
    let events2 = create_events(1000);
    b.iter(|| {
        let events1 = create_events(1000);
        filter_period_intersect(&events1, &events2);
    });
}
