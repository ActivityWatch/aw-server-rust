#[macro_use]
extern crate log;
extern crate chrono;
#[macro_use]
extern crate aw_datastore;
extern crate serde_json;

extern crate appdirs;

#[cfg(test)]
mod datastore_tests {
    use chrono::Duration;
    use chrono::Utc;
    use serde_json::json;

    use aw_datastore::Datastore;

    use aw_models::Bucket;
    use aw_models::BucketMetadata;
    use aw_models::Event;

    fn test_bucket() -> Bucket {
        Bucket {
            bid: None,
            id: "testid".to_string(),
            _type: "testtype".to_string(),
            client: "testclient".to_string(),
            hostname: "testhost".to_string(),
            created: None,
            data: json_map! {},
            metadata: BucketMetadata::default(),
            events: None,
            last_updated: None,
        }
    }

    #[cfg(not(target_os = "android"))]
    use appdirs;
    #[cfg(not(target_os = "android"))]
    use std::fs;
    use std::path::PathBuf;
    pub fn get_cache_dir() -> Result<PathBuf, ()> {
        #[cfg(not(target_os = "android"))]
        {
            let mut dir = appdirs::user_cache_dir(Some("activitywatch"), None)?;
            dir.push("aw-server-rust");
            fs::create_dir_all(dir.clone()).expect("Unable to create cache dir");
            return Ok(dir);
        }

        #[cfg(target_os = "android")]
        {
            panic!("not implemented on Android");
        }
    }

    fn create_test_bucket(ds: &Datastore) -> Bucket {
        let bucket = test_bucket();
        ds.create_bucket(&bucket).unwrap();
        bucket
    }

    #[test]
    fn test_bucket_create_delete() {
        // Setup datastore
        let ds = Datastore::new_in_memory(false);
        let bucket = create_test_bucket(&ds);

        // Fetch bucket
        let bucket_fetched = ds.get_bucket(&bucket.id).unwrap();
        assert_eq!(bucket_fetched.id, bucket.id);
        assert_eq!(bucket_fetched._type, bucket._type);
        assert_eq!(bucket_fetched.client, bucket.client);
        assert_eq!(bucket_fetched.hostname, bucket.hostname);
        assert_eq!(bucket_fetched.metadata.end, None);

        match bucket_fetched.created {
            None => panic!("Expected 'None' in bucket to be replaced with current time"),
            Some(created) => {
                let now = Utc::now();
                assert!(created <= now);
                assert!(created > now - Duration::seconds(10));
            }
        };

        // Fetch all buckets
        let fetched_buckets = ds.get_buckets().unwrap();
        assert!(fetched_buckets.contains_key(&bucket.id));
        assert_eq!(fetched_buckets[&bucket.id].id, bucket.id);
        assert_eq!(fetched_buckets[&bucket.id]._type, bucket._type);
        assert_eq!(fetched_buckets[&bucket.id].client, bucket.client);
        assert_eq!(fetched_buckets[&bucket.id].hostname, bucket.hostname);
        assert_eq!(
            fetched_buckets[&bucket.id].metadata.start,
            bucket.metadata.start
        );
        assert_eq!(
            fetched_buckets[&bucket.id].metadata.end,
            bucket.metadata.end
        );

        match fetched_buckets[&bucket.id].created {
            None => panic!("Expected 'None' in bucket to be replaced with current time"),
            Some(created) => {
                let now = Utc::now();
                assert!(created <= now);
                assert!(created > now - Duration::seconds(10));
            }
        };

        // Delete bucket
        match ds.delete_bucket(&bucket.id) {
            Ok(_) => info!("bucket successfully deleted"),
            Err(e) => panic!(e),
        }
        match ds.get_bucket(&bucket.id) {
            Ok(_) => {
                panic!("Expected datastore to delete bucket but bucket seems to still be available")
            }
            Err(_e) => (),
        }
    }

    #[test]
    fn test_events_get_filters() {
        // Setup datastore
        let ds = Datastore::new_in_memory(false);
        let bucket = create_test_bucket(&ds);

        // Insert event
        let e1 = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(0),
            data: json_map! {"key": json!("value")},
        };
        let mut e2 = e1.clone();
        e2.timestamp = e2.timestamp + Duration::nanoseconds(1);

        let event_list = [e1.clone(), e2.clone()];

        ds.insert_events(&bucket.id, &event_list).unwrap();

        // Get all events
        let fetched_events_all = ds.get_events(&bucket.id, None, None, None).unwrap();
        let expected_fetched_events = vec![&e2, &e1];
        assert_eq!(fetched_events_all.len(), 2);
        for i in 0..fetched_events_all.len() {
            let expected = &expected_fetched_events[i];
            let new = &fetched_events_all[i];
            assert_eq!(new.timestamp, expected.timestamp);
            assert_eq!(new.duration, expected.duration);
            assert_eq!(new.data, expected.data);
        }

        info!("Get events with limit filter");
        let fetched_events_limit = ds.get_events(&bucket.id, None, None, Some(1)).unwrap();
        assert_eq!(fetched_events_limit.len(), 1);
        assert_eq!(fetched_events_limit[0].timestamp, e2.timestamp);
        assert_eq!(fetched_events_limit[0].duration, e2.duration);
        assert_eq!(fetched_events_limit[0].data, e2.data);

        info!("Get events with starttime filter");
        let fetched_events_start = ds
            .get_events(&bucket.id, Some(e2.timestamp.clone()), None, None)
            .unwrap();
        assert_eq!(fetched_events_start.len(), 1);
        assert_eq!(fetched_events_start[0].timestamp, e2.timestamp);
        assert_eq!(fetched_events_start[0].duration, e2.duration);
        assert_eq!(fetched_events_start[0].data, e2.data);

        info!("Get events with endtime filter");
        let fetched_events_start = ds
            .get_events(&bucket.id, None, Some(e1.timestamp.clone()), None)
            .unwrap();
        assert_eq!(fetched_events_start.len(), 1);
        assert_eq!(fetched_events_start[0].timestamp, e1.timestamp);
        assert_eq!(fetched_events_start[0].duration, e1.duration);
        assert_eq!(fetched_events_start[0].data, e1.data);

        // Get eventcount
        let event_count = ds.get_event_count(&bucket.id, None, None).unwrap();
        assert_eq!(event_count, 2);
    }

    #[test]
    fn test_events_delete() {
        // Setup datastore
        let ds = Datastore::new_in_memory(false);
        let bucket = create_test_bucket(&ds);

        // Insert event
        let e1 = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(0),
            data: json_map! {"key": json!("value")},
        };
        let mut e2 = e1.clone();
        e2.timestamp = e2.timestamp + Duration::seconds(1);

        let event_list = [e1.clone(), e2.clone()];

        ds.insert_events(&bucket.id, &event_list).unwrap();

        // Get all events
        let fetched_events_all = ds.get_events(&bucket.id, None, None, None).unwrap();
        let expected_fetched_events = vec![&e2, &e1];
        assert_eq!(fetched_events_all.len(), 2);
        for i in 0..fetched_events_all.len() {
            let expected = &expected_fetched_events[i];
            let new = &fetched_events_all[i];
            assert_eq!(new.timestamp, expected.timestamp);
            assert_eq!(new.duration, expected.duration);
            assert_eq!(new.data, expected.data);
        }
        let e1 = &fetched_events_all[0];
        let e2 = &fetched_events_all[1];

        // Delete one event
        ds.delete_events_by_id(&bucket.id, vec![e1.id.unwrap()])
            .unwrap();

        // Get all events
        let fetched_events_all = ds.get_events(&bucket.id, None, None, None).unwrap();
        let expected_fetched_events = vec![e2];
        assert_eq!(fetched_events_all.len(), 1);
        for i in 0..fetched_events_all.len() {
            let expected = &expected_fetched_events[i];
            let new = &fetched_events_all[i];
            assert_eq!(new.id, expected.id);
            assert_eq!(new.timestamp, expected.timestamp);
            assert_eq!(new.duration, expected.duration);
            assert_eq!(new.data, expected.data);
        }
    }

    #[test]
    fn test_bucket_metadata_start_end() {
        // Setup datastore
        let ds = Datastore::new_in_memory(false);
        let bucket = create_test_bucket(&ds);

        // Insert event
        let e1 = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(0),
            data: json_map! {"key": json!("value")},
        };
        let mut e2 = e1.clone();
        e2.timestamp = e2.timestamp + Duration::nanoseconds(1);

        let event_list = [e1.clone(), e2.clone()];

        ds.insert_events(&bucket.id, &event_list).unwrap();

        // Validate correct start and end in bucket
        let bucket_fetched = ds.get_bucket(&bucket.id).unwrap();
        assert_eq!(bucket_fetched.metadata.start, Some(e1.timestamp));
        assert_eq!(bucket_fetched.metadata.end, Some(e2.calculate_endtime()));
    }

    #[test]
    fn test_event_heartbeat() {
        // Setup datastore
        let ds = Datastore::new_in_memory(false);
        let bucket = create_test_bucket(&ds);

        // Insert event
        let e1 = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(0),
            data: json_map! {"key": json!("value")},
        };
        let mut e2 = e1.clone();
        e2.timestamp = e2.timestamp + Duration::seconds(1);

        let mut e_diff_data = e2.clone();
        e_diff_data.timestamp = e_diff_data.timestamp + Duration::seconds(1);
        e_diff_data.data = json_map! {"key": json!("other value")};

        // First event
        ds.heartbeat(&bucket.id, e1.clone(), 10.0).unwrap();
        let fetched_events = ds.get_events(&bucket.id, None, None, None).unwrap();
        assert_eq!(fetched_events.len(), 1);
        assert_eq!(fetched_events[0].timestamp, e1.timestamp);
        assert_eq!(fetched_events[0].duration, e1.duration);
        assert_eq!(fetched_events[0].data, e1.data);
        let e1 = &fetched_events[0];

        // Heartbeat match
        ds.heartbeat(&bucket.id, e2.clone(), 10.0).unwrap();
        let fetched_events = ds.get_events(&bucket.id, None, None, None).unwrap();
        assert_eq!(fetched_events.len(), 1);
        assert_eq!(fetched_events[0].timestamp, e1.timestamp);
        assert_eq!(fetched_events[0].duration, Duration::seconds(1));
        assert_eq!(fetched_events[0].data, e1.data);
        assert_eq!(fetched_events[0].id, e1.id);
        let e2 = &fetched_events[0];

        // Heartbeat diff
        ds.heartbeat(&bucket.id, e_diff_data.clone(), 10.0).unwrap();
        let fetched_events = ds.get_events(&bucket.id, None, None, None).unwrap();
        assert_eq!(fetched_events.len(), 2);
        assert_eq!(fetched_events[0].timestamp, e_diff_data.timestamp);
        assert_eq!(fetched_events[0].duration, e_diff_data.duration);
        assert_eq!(fetched_events[0].data, e_diff_data.data);
        assert_ne!(fetched_events[0].id, e2.id);
    }

    #[test]
    fn test_event_replace() {
        // Setup datastore
        let ds = Datastore::new_in_memory(false);
        let bucket = create_test_bucket(&ds);

        // Insert event
        let e = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(0),
            data: json_map! {"key": json!("value")},
        };
        let mut e1 = e.clone();
        e1.data = json_map! {"key": json!("value1")};
        let mut e2 = e.clone();
        e2.data = json_map! {"key": json!("value2")};
        let mut e3 = e.clone();
        e3.data = json_map! {"key": json!("value3")};

        let events_init = &[e1.clone(), e2.clone(), e3.clone()];
        let events_ret = ds.insert_events(&bucket.id, events_init).unwrap();
        // Validate return from insert
        assert_eq!(events_ret[0].id, Some(1));
        assert_eq!(events_ret[1].id, Some(2));
        assert_eq!(events_ret[2].id, Some(3));
        assert_eq!(events_ret.len(), 3);
        assert_eq!(events_ret[0], events_init[0]);
        assert_eq!(events_ret[1], events_init[1]);
        assert_eq!(events_ret[2], events_init[2]);

        let events_init = events_ret;

        // Insert e2 with identical data and id (which means a replace)
        {
            let events_ret = ds
                .insert_events(&bucket.id, &[events_init[1].clone()])
                .unwrap();
            assert_eq!(events_ret.len(), 1);
            assert_eq!(events_ret[0], events_init[1]);
            let fetched_events = ds.get_events(&bucket.id, None, None, None).unwrap();
            assert_eq!(fetched_events, events_init);
        }

        // Make new event with same id but different data
        {
            let mut e2 = events_init[1].clone();
            e2.data = json_map! {"key": json!("value2_modified")};
            let events_ret = ds.insert_events(&bucket.id, &[e2.clone()]).unwrap();
            assert_eq!(events_ret.len(), 1);
            assert_eq!(events_ret[0], e2);
            let fetched_events = ds.get_events(&bucket.id, None, None, None).unwrap();
            assert_eq!(fetched_events.len(), 3);
            assert_eq!(fetched_events[1], e2);
            assert_eq!(fetched_events[0].id, Some(1));
            assert_eq!(fetched_events[1].id, Some(2));
            assert_eq!(fetched_events[2].id, Some(3));
        }
    }

    #[test]
    fn test_datastore_reload() {
        // Create tmp datastore path
        let mut db_path = get_cache_dir().unwrap();
        db_path.push("datastore-unittest.db");
        let db_path_str = db_path.to_str().unwrap().to_string();

        if db_path.exists() {
            std::fs::remove_file(db_path.clone())
                .expect("Failed to remove datastore-unittest.db file");
        }

        let empty_bucket = test_bucket();
        let mut populated_bucket = empty_bucket.clone();
        populated_bucket.id = "testid2".to_string();
        let e1 = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(0),
            data: json_map! {"key": json!("value")},
        };
        {
            // Initialize database and create buckets
            let ds = Datastore::new(db_path_str.clone(), false);
            ds.create_bucket(&empty_bucket).unwrap();
            ds.create_bucket(&populated_bucket).unwrap();
            // Insert event
            ds.insert_events(&populated_bucket.id, &[e1.clone()])
                .unwrap();

            // Check that all cached bucket data is correct
            let buckets = ds.get_buckets().unwrap();
            assert_eq!(buckets[&empty_bucket.id].metadata.start, None);
            assert_eq!(buckets[&empty_bucket.id].metadata.end, None);
            assert_eq!(
                buckets[&populated_bucket.id].metadata.start,
                Some(e1.calculate_endtime())
            );
            assert_eq!(
                buckets[&populated_bucket.id].metadata.end,
                Some(e1.timestamp)
            );
        }
        {
            // Load database again
            let ds = Datastore::new(db_path_str.clone(), false);
            // Check that all bucket data is correct after reload
            let buckets = ds.get_buckets().unwrap();
            assert_eq!(buckets[&empty_bucket.id].metadata.start, None);
            assert_eq!(buckets[&empty_bucket.id].metadata.end, None);
            assert_eq!(
                buckets[&populated_bucket.id].metadata.start,
                Some(e1.timestamp)
            );
            assert_eq!(
                buckets[&populated_bucket.id].metadata.end,
                Some(e1.calculate_endtime())
            );
        }
    }
}
