#[macro_use]
extern crate log;
extern crate aw_sync;

#[cfg(test)]
mod sync_tests {
    use std::collections::HashMap;
    use std::path::Path;

    use chrono::{DateTime, Duration, Utc};

    use aw_datastore::{Datastore, DatastoreError};
    use aw_models::{Bucket, Event};
    use aw_sync::{create_datastore, AccessMethod, SyncSpec};

    struct TestState {
        ds_src: Datastore,
        ds_dest: Datastore,
    }

    fn init_teststate() -> TestState {
        TestState {
            ds_src: Datastore::new_in_memory(false),
            ds_dest: Datastore::new_in_memory(false),
        }
    }

    fn create_bucket(ds: &Datastore, n: i32) -> String {
        // Create a bucket
        let bucket_id = format!("bucket-{n}");
        let bucket_jsonstr = format!(
            r#"{{
            "id": "{bucket_id}",
            "type": "test",
            "hostname": "device-{n}",
            "client": "test"
        }}"#
        );
        let bucket: Bucket = serde_json::from_str(&bucket_jsonstr).unwrap();
        match ds.create_bucket(&bucket) {
            Ok(()) => (),
            Err(e) => match e {
                DatastoreError::BucketAlreadyExists(_) => {
                    debug!("bucket already exists, skipping");
                }
                e => panic!("woops! {e:?}"),
            },
        };
        bucket_id
    }

    fn create_event(data_str: &str) -> Event {
        // A workaround needed because otherwise events might get same timestamp if
        // call is repeated quickly on platforms with a low-precision clock.
        std::thread::sleep(std::time::Duration::from_millis(5));

        let timestamp: DateTime<Utc> = Utc::now();
        let event_jsonstr = format!(
            r#"{{
            "timestamp": "{}",
            "duration": 0,
            "data": {{"test": {} }}
        }}"#,
            timestamp.to_rfc3339(),
            data_str
        );
        serde_json::from_str(&event_jsonstr).unwrap()
    }

    fn create_events(ds: &Datastore, bucket_id: &str, n: i64) {
        let events: Vec<Event> = (0..n)
            .map(|i| create_event(format!("{i}").as_str()))
            .collect::<Vec<Event>>();

        ds.insert_events(bucket_id, &events[..]).unwrap();
        ds.force_commit().unwrap();
    }

    fn get_all_buckets(datastores: Vec<&Datastore>) -> Vec<(&Datastore, Bucket)> {
        let mut all_buckets: Vec<(&Datastore, Bucket)> = Vec::new();
        for ds in datastores {
            let buckets = ds.get_buckets().unwrap();
            for bucket in buckets.values() {
                all_buckets.push((ds, bucket.clone()));
            }
        }
        all_buckets
    }

    fn get_all_buckets_map(datastores: Vec<&Datastore>) -> HashMap<String, (&Datastore, Bucket)> {
        let all_buckets = get_all_buckets(datastores);
        all_buckets
            .iter()
            .cloned()
            .map(|(ds, b)| (b.id.clone(), (ds, b)))
            .collect()
    }

    #[test]
    fn test_buckets_created() {
        // TODO: Split up this test
        let state = init_teststate();
        create_bucket(&state.ds_src, 0);

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        );

        let buckets_src: HashMap<String, Bucket> = state.ds_src.get_buckets().unwrap();
        let buckets_dest: HashMap<String, Bucket> = state.ds_dest.get_buckets().unwrap();
        assert!(buckets_src.len() == buckets_dest.len());
    }

    fn check_synced_buckets_equal_to_src(all_buckets_map: &HashMap<String, (&Datastore, Bucket)>) {
        for (ds, bucket) in all_buckets_map.values() {
            if bucket.id.contains("-synced") {
                let bucket_src_id = bucket.id.split("-synced-").next().unwrap();
                let (ds_src, bucket_src) = all_buckets_map.get(bucket_src_id).unwrap();
                let events_synced = ds.get_events(bucket.id.as_str(), None, None, None).unwrap();
                let events_src = ds_src
                    .get_events(bucket_src.id.as_str(), None, None, None)
                    .unwrap();
                println!("{events_synced:?}");
                println!("{events_src:?}");
                assert!(events_synced == events_src);
            }
        }
    }

    #[test]
    fn test_one_updated_event() {
        // This tests the syncing of one single event that is then updated by a heartbeat after the
        // first sync pass.
        let state = init_teststate();

        let bucket_id = create_bucket(&state.ds_src, 0);
        state
            .ds_src
            .heartbeat(bucket_id.as_str(), create_event("1"), 1.0)
            .unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        );

        let all_datastores: Vec<&Datastore> = [&state.ds_src, &state.ds_dest].to_vec();
        let all_buckets_map = get_all_buckets_map(all_datastores);

        // Check that all synced buckets are identical to source bucket
        check_synced_buckets_equal_to_src(&all_buckets_map);

        // Add some more events
        state
            .ds_src
            .heartbeat(bucket_id.as_str(), create_event("1"), 1.0)
            .unwrap();
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        );

        // Check again that new events were indeed synced
        check_synced_buckets_equal_to_src(&all_buckets_map);
    }

    #[test]
    fn test_events() {
        let state = init_teststate();

        let bucket_id = create_bucket(&state.ds_src, 0);
        create_events(&state.ds_src, bucket_id.as_str(), 10);

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        );

        let all_datastores: Vec<&Datastore> = [&state.ds_src, &state.ds_dest].to_vec();
        let all_buckets_map = get_all_buckets_map(all_datastores);

        // Check that all synced buckets are identical to source bucket
        check_synced_buckets_equal_to_src(&all_buckets_map);

        // Add some more events
        create_events(&state.ds_src, bucket_id.as_str(), 10);
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        );

        // Check again that new events were indeed synced
        check_synced_buckets_equal_to_src(&all_buckets_map);
    }

    // TODO: Find a way to reuse this (previously used in an integration test)
    fn setup_test(sync_directory: &Path) -> std::io::Result<Vec<Datastore>> {
        let mut datastores: Vec<Datastore> = Vec::new();
        for n in 0..2 {
            let dspath = sync_directory.join(format!("test-remote-{n}.db"));
            let ds_ = create_datastore(&dspath);
            let ds = &ds_ as &dyn AccessMethod;

            // Create a bucket
            // NOTE: Created with duplicate name to make sure it still works under such conditions
            let bucket_jsonstr = format!(
                r#"{{
                    "id": "bucket",
                    "type": "test",
                    "hostname": "device-{n}",
                    "client": "test"
                }}"#
            );
            let bucket: Bucket = serde_json::from_str(&bucket_jsonstr)?;
            match ds.create_bucket(&bucket) {
                Ok(()) => (),
                Err(e) => match e {
                    DatastoreError::BucketAlreadyExists(_) => {
                        debug!("bucket already exists, skipping");
                    }
                    e => panic!("woops! {e:?}"),
                },
            };

            // Insert some testing events into the bucket
            let events: Vec<Event> = (0..3)
                .map(|i| {
                    let timestamp: DateTime<Utc> = Utc::now() + Duration::milliseconds(i * 10);
                    let event_jsonstr = format!(
                        r#"{{
                            "timestamp": "{}",
                            "duration": 0,
                            "data": {{"test": {} }}
                        }}"#,
                        timestamp.to_rfc3339(),
                        i
                    );
                    serde_json::from_str(&event_jsonstr).unwrap()
                })
                .collect::<Vec<Event>>();

            ds.insert_events(bucket.id.as_str(), events).unwrap();
            //let new_eventcount = ds.get_event_count(bucket.id.as_str(), None, None).unwrap();
            //info!("Eventcount: {:?} ({} new)", new_eventcount, events.len());
            datastores.push(ds_);
        }
        Ok(datastores)
    }
}
