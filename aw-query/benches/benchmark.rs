use criterion::{criterion_group, criterion_main};

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

#[cfg(test)]
mod query_benchmarks {
    use chrono::Duration;
    use criterion::Criterion;
    use serde_json::json;
    use serde_json::Map;
    use serde_json::Value;

    use aw_datastore::Datastore;
    use aw_models::Bucket;
    use aw_models::BucketMetadata;
    use aw_models::Event;
    use aw_models::TimeInterval;

    static BUCKETNAME: &str = "testbucket";
    static TIME_INTERVAL: &str = "1980-01-01T00:00:00Z/2080-01-02T00:00:00Z";

    fn setup_datastore() -> Datastore {
        Datastore::new_in_memory(false)
    }

    fn create_bucket(ds: &Datastore, bucketname: String) {
        let bucket = Bucket {
            bid: None,
            id: bucketname,
            _type: "testtype".to_string(),
            client: "testclient".to_string(),
            hostname: "testhost".to_string(),
            created: Some(chrono::Utc::now()),
            data: json_map! {},
            metadata: BucketMetadata::default(),
            events: None,
            last_updated: None,
        };
        ds.create_bucket(&bucket).unwrap();
    }

    fn insert_events(ds: &Datastore, bucketname: &str, num_events: i64) {
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
        ds.insert_events(bucketname, &event_list).unwrap();
    }

    pub fn bench_assign(c: &mut Criterion) {
        let ds = setup_datastore();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();
        c.bench_function("bench assign", |b| {
            b.iter(|| {
                let code = String::from("return a=1;");
                match aw_query::query(&code, &interval, &ds).unwrap() {
                    aw_query::DataType::None() => (),
                    ref data => panic!("Wrong datatype, {data:?}"),
                };
            })
        });
    }

    pub fn bench_many_events(c: &mut Criterion) {
        let ds = setup_datastore();
        create_bucket(&ds, BUCKETNAME.to_string());
        insert_events(&ds, BUCKETNAME, 5000);

        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();
        c.bench_function("bench many events", |b| {
            b.iter(|| {
                let code = String::from(
                    "
                events = query_bucket(\"testbucket\");
                return events;
                ",
                );
                aw_query::query(&code, &interval, &ds).unwrap();
            })
        });
    }
}

criterion_group!(
    benches,
    query_benchmarks::bench_assign,
    query_benchmarks::bench_many_events
);
criterion_main!(benches);
