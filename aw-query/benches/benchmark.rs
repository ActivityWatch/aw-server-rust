#![feature(test)]
extern crate test;

extern crate aw_datastore;
extern crate aw_models;
extern crate aw_query;

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
    use test::Bencher;

    use chrono::Duration;
    use serde_json::json;
    use serde_json::Map;
    use serde_json::Value;

    use aw_datastore::Datastore;
    use aw_models::Bucket;
    use aw_models::BucketMetadata;
    use aw_models::Event;
    use aw_models::TimeInterval;

    static TIME_INTERVAL: &str = "1980-01-01T00:00:00Z/2080-01-02T00:00:00Z";
    static BUCKET_ID: &str = "testid";

    fn setup_datastore_empty() -> Datastore {
        return Datastore::new_in_memory(false);
    }

    fn setup_datastore_with_bucket() -> Datastore {
        let ds = setup_datastore_empty();
        // Create bucket
        let bucket = Bucket {
            bid: None,
            id: BUCKET_ID.to_string(),
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
        return ds;
    }

    fn setup_datastore_populated() -> Datastore {
        let ds = setup_datastore_with_bucket();

        let mut possible_data = Vec::<Map<String, Value>>::new();
        for i in 0..20 {
            possible_data.push(json_map! {"number": i});
        }
        //
        let mut event_list = Vec::new();
        for i in 0..3000 {
            let e = Event {
                id: None,
                timestamp: chrono::Utc::now() + Duration::seconds(i),
                duration: Duration::seconds(1),
                data: possible_data[i as usize % 20].clone(),
            };
            event_list.push(e);
        }
        ds.insert_events(&BUCKET_ID, &event_list).unwrap();

        return ds;
    }

    #[bench]
    fn bench_assign(b: &mut Bencher) {
        let ds = setup_datastore_empty();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();
        b.iter(|| {
            let code = String::from("return a=1;");
            match aw_query::query(&code, &interval, &ds).unwrap() {
                aw_query::DataType::None() => (),
                ref data => panic!("Wrong datatype, {:?}", data),
            };
        });
    }

    #[bench]
    fn bench_many_events(b: &mut Bencher) {
        let ds = setup_datastore_populated();
        let interval = TimeInterval::new_from_string(TIME_INTERVAL).unwrap();
        b.iter(|| {
            let code = String::from(
                "
                events = query_bucket(\"testid\");
                return events;
            ",
            );
            aw_query::query(&code, &interval, &ds).unwrap();
        });
    }
}
