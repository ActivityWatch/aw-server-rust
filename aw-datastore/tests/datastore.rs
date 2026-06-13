#[macro_use]
extern crate log;
extern crate chrono;
#[macro_use]
extern crate aw_datastore;
extern crate serde_json;

extern crate dirs;

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
    use std::fs;
    use std::path::PathBuf;
    pub fn get_cache_dir() -> Result<PathBuf, ()> {
        #[cfg(not(target_os = "android"))]
        {
            let dir = dirs::cache_dir()
                .ok_or(())?
                .join("activitywatch")
                .join("aw-server-rust");
            fs::create_dir_all(&dir).expect("Unable to create cache dir");
            Ok(dir)
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
            Err(e) => panic!("{e:?}"),
        }
        match ds.get_bucket(&bucket.id) {
            Ok(_) => {
                panic!("Expected datastore to delete bucket but bucket seems to still be available")
            }
            Err(_e) => (),
        }
    }

    #[test]
    fn test_events_get_single() {
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
        e2.timestamp += Duration::nanoseconds(1);

        let event_list = [e1, e2];
        ds.insert_events(&bucket.id, &event_list).unwrap();

        let events = ds.get_events(&bucket.id, None, None, None).unwrap();
        let first_event = events.first().unwrap();
        let first_event_id = first_event.id.unwrap();

        let fetched_event = ds.get_event(&bucket.id, first_event_id).unwrap();
        // TODO: Check entire events to ensure integrity
        assert_eq!(fetched_event.id.unwrap(), first_event_id);
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
        e2.timestamp += Duration::nanoseconds(1);

        let event_list = [e1.clone(), e2.clone()];

        ds.insert_events(&bucket.id, &event_list).unwrap();

        // Get all events
        let fetched_events_all = ds.get_events(&bucket.id, None, None, None).unwrap();
        let expected_fetched_events = [&e2, &e1];
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
            .get_events(&bucket.id, Some(e2.timestamp), None, None)
            .unwrap();
        assert_eq!(fetched_events_start.len(), 1);
        assert_eq!(fetched_events_start[0].timestamp, e2.timestamp);
        assert_eq!(fetched_events_start[0].duration, e2.duration);
        assert_eq!(fetched_events_start[0].data, e2.data);

        info!("Get events with endtime filter");
        let fetched_events_start = ds
            .get_events(&bucket.id, None, Some(e1.timestamp), None)
            .unwrap();
        assert_eq!(fetched_events_start.len(), 1);
        assert_eq!(fetched_events_start[0].timestamp, e1.timestamp);
        assert_eq!(fetched_events_start[0].duration, e1.duration);
        assert_eq!(fetched_events_start[0].data, e1.data);

        // Get eventcount
        let event_count = ds.get_event_count(&bucket.id, None, None).unwrap();
        assert_eq!(event_count, 2);
    }

    /// Tests that events that cover a timeperiod get included when that timeperiod is queried.
    #[test]
    fn test_get_events_filters_cover() {
        // TODO: Also test event-cutoff, although perhaps that happens in the transforms/queries?

        // Setup datastore
        let ds = Datastore::new_in_memory(false);
        let bucket = create_test_bucket(&ds);

        let now = Utc::now();

        // Insert event
        let e1 = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(100),
            data: json_map! {"key": json!("value")},
        };

        let event_list = [e1];
        ds.insert_events(&bucket.id, &event_list).unwrap();

        info!("Get event that covers queried timeperiod");
        let query_start = now + Duration::seconds(1);
        let query_end = query_start + Duration::seconds(1);
        let fetched_events_limit = ds
            .get_events(&bucket.id, Some(query_start), Some(query_end), Some(1))
            .unwrap();
        assert_eq!(fetched_events_limit.len(), 1);

        // Get eventcount
        let event_count = ds
            .get_event_count(&bucket.id, Some(query_start), Some(query_end))
            .unwrap();
        assert_eq!(event_count, 1);
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
        e2.timestamp += Duration::seconds(1);

        let event_list = [e1.clone(), e2.clone()];

        ds.insert_events(&bucket.id, &event_list).unwrap();

        // Get all events
        let fetched_events_all = ds.get_events(&bucket.id, None, None, None).unwrap();
        let expected_fetched_events = [&e2, &e1];
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
        let expected_fetched_events = [e2];
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
        e2.timestamp += Duration::nanoseconds(1);

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
        e2.timestamp += Duration::seconds(1);

        let mut e_diff_data = e2.clone();
        e_diff_data.timestamp += Duration::seconds(1);
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
        ds.heartbeat(&bucket.id, e2, 10.0).unwrap();
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
    fn test_event_heartbeat_consecutive_merges() {
        // Regression test for https://github.com/ActivityWatch/aw-server-rust/issues/559
        // After insert_heartbeat merges two events, the merged event stored in cache
        // must retain the DB event ID so subsequent heartbeats can find it for
        // replace_last_event. Without the ID, every other heartbeat would fail.
        let ds = Datastore::new_in_memory(false);
        let bucket = create_test_bucket(&ds);

        let now = Utc::now();
        let pulsetime = 10.0;

        // Send 5 consecutive heartbeats with the same data, 1 second apart.
        // They should all merge into a single event with increasing duration.
        for i in 0..5 {
            let e = Event {
                id: None,
                timestamp: now + Duration::seconds(i),
                duration: Duration::seconds(0),
                data: json_map! {"key": json!("value")},
            };
            ds.heartbeat(&bucket.id, e, pulsetime).unwrap();
        }

        let events = ds.get_events(&bucket.id, None, None, None).unwrap();
        assert_eq!(
            events.len(),
            1,
            "all heartbeats should merge into one event"
        );
        assert_eq!(events[0].timestamp, now);
        assert_eq!(events[0].duration, Duration::seconds(4));
        assert!(events[0].id.is_some(), "event should have a DB id");
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
        let mut e3 = e;
        e3.data = json_map! {"key": json!("value3")};

        let events_init = &[e1, e2, e3];
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
    fn test_migration_v4_to_v5() {
        let mut db_path = get_cache_dir().unwrap();
        db_path.push("datastore-unittest-migration-v4.db");
        let db_path_str = db_path.to_str().unwrap().to_string();

        if db_path.exists() {
            std::fs::remove_file(&db_path)
                .expect("Failed to remove datastore-unittest-migration-v4.db file");
        }

        // Construct a database with the v4 schema (single-column indexes)
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE buckets (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT UNIQUE NOT NULL,
                    type TEXT NOT NULL,
                    client TEXT NOT NULL,
                    hostname TEXT NOT NULL,
                    created TEXT NOT NULL,
                    data TEXT NOT NULL DEFAULT '{}'
                );
                CREATE INDEX bucket_id_index ON buckets(id);
                CREATE TABLE events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    bucketrow INTEGER NOT NULL,
                    starttime INTEGER NOT NULL,
                    endtime INTEGER NOT NULL,
                    data TEXT NOT NULL,
                    FOREIGN KEY (bucketrow) REFERENCES buckets(id)
                );
                CREATE INDEX events_bucketrow_index ON events(bucketrow);
                CREATE INDEX events_starttime_index ON events(starttime);
                CREATE INDEX events_endtime_index ON events(endtime);
                CREATE TABLE key_value (
                    key TEXT PRIMARY KEY,
                    value TEXT,
                    last_modified NUMBER NOT NULL
                );
                INSERT INTO buckets (name, type, client, hostname, created, data)
                    VALUES ('testid', 'testtype', 'testclient', 'testhost',
                            '2024-01-01T00:00:00+00:00', '{}');
                INSERT INTO events (bucketrow, starttime, endtime, data)
                    VALUES (1, 1000000000, 2000000000, '{"key": "value"}');
                PRAGMA user_version = 4;
            "#,
            )
            .unwrap();
        }

        // Opening the datastore migrates to the newest version
        {
            let ds = Datastore::new(db_path_str, false);
            let events = ds.get_events("testid", None, None, None).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].data, json_map! {"key": json!("value")});
            ds.close();
        }

        // Verify version bump and index replacement
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            let version: i32 = conn
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            // Database should be upgraded to v6 (compression) automatically
            assert_eq!(version, 6);
            let old_indexes: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name IN
                     ('events_bucketrow_index', 'events_starttime_index', 'events_endtime_index')",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(old_indexes, 0, "single-column indexes should be dropped");
            let new_index: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'index'
                     AND name = 'events_bucketrow_starttime_index'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(new_index, 1, "composite index should exist");
        }

        std::fs::remove_file(&db_path)
            .expect("Failed to remove datastore-unittest-migration-v4.db file");
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
            ds.insert_events(&populated_bucket.id, std::slice::from_ref(&e1))
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
            ds.force_commit().unwrap();
        }
        {
            // Load database again
            let ds = Datastore::new(db_path_str, false);
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

    /// Test that an encrypted datastore can be created, written to, and reopened with the same key
    /// with data intact.
    #[test]
    #[cfg(any(feature = "encryption", feature = "encryption-vendored"))]
    fn test_encrypted_datastore_roundtrip() {
        use std::fs;
        let dir = get_cache_dir().unwrap();
        let db_path = dir.join("test-encrypted.db").to_str().unwrap().to_string();
        // Clean up from previous runs
        let _ = fs::remove_file(&db_path);

        let key = "s3cr3t-p@ssw0rd".to_string();

        // Create and populate encrypted datastore
        {
            let ds = Datastore::new_encrypted(db_path.clone(), key.clone(), false);
            let bucket = create_test_bucket(&ds);
            let e = Event {
                id: None,
                timestamp: Utc::now(),
                duration: Duration::seconds(1),
                data: json_map! { "app": "test-encrypted" },
            };
            let inserted = ds.insert_events(&bucket.id, &[e]).unwrap();
            assert_eq!(inserted.len(), 1);
            ds.force_commit().unwrap();
            ds.close();
        }

        // Reopen with correct key — data must survive the roundtrip
        {
            let ds = Datastore::new_encrypted(db_path.clone(), key.clone(), false);
            let events = ds
                .get_events("testid", None, None, None)
                .expect("should read events from encrypted DB after reopen");
            assert_eq!(events.len(), 1, "expected 1 event after encrypted reopen");
            assert_eq!(events[0].data["app"], "test-encrypted");
            ds.close();
        }

        let _ = fs::remove_file(&db_path);
    }

    /// With the compression feature, a database that grows past the training
    /// threshold should get a dictionary on the next open, recompress its rows,
    /// and still return every event's data intact.
    #[test]
    #[cfg(feature = "compression-zstd")]
    fn test_compression_dictionary_roundtrip() {
        use rusqlite::Connection;

        let mut db_path = get_cache_dir().unwrap();
        db_path.push("datastore-unittest-compression.db");
        let db_path_str = db_path.to_str().unwrap().to_string();
        if db_path.exists() {
            std::fs::remove_file(&db_path).unwrap();
        }

        let bucket = test_bucket();
        // Realistic, highly-repetitive event data, well above MIN_EVENTS_TO_TRAIN.
        let n = 4000;
        let make_event = |i: usize| Event {
            id: None,
            timestamp: Utc::now() + Duration::milliseconds(i as i64),
            duration: Duration::seconds(1),
            data: json_map! {
                "app": json!(["Firefox", "Terminal", "Code", "Slack"][i % 4]),
                "title": json!(format!("Working on window {}", i % 100))
            },
        };

        // Session 1: no dictionary exists yet (DB started empty), rows stored raw.
        {
            let ds = Datastore::new(db_path_str.clone(), false);
            ds.create_bucket(&bucket).unwrap();
            let events: Vec<Event> = (0..n).map(make_event).collect();
            ds.insert_events(&bucket.id, &events).unwrap();
            ds.force_commit().unwrap();
            ds.close();
        }

        // No dictionary should have been trained during session 1.
        {
            let conn = Connection::open(&db_path).unwrap();
            let dict_rows: i64 = conn
                .query_row("SELECT count(*) FROM compression_dict", [], |r| r.get(0))
                .unwrap();
            assert_eq!(dict_rows, 0, "no dictionary expected before reopen");
        }

        // Session 2: reopening trains a dictionary and recompresses existing rows.
        {
            let ds = Datastore::new(db_path_str.clone(), false);
            let events = ds.get_events(&bucket.id, None, None, None).unwrap();
            assert_eq!(events.len(), n);
            // Data must survive the train + recompress roundtrip exactly.
            for e in &events {
                let i = e.data["title"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("Working on window ")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                assert!(i < 100);
                assert!(e.data["app"].is_string());
            }
            // Inserting more after the dictionary exists must also roundtrip.
            ds.insert_events(&bucket.id, &[make_event(99999)]).unwrap();
            ds.force_commit().unwrap();
            ds.close();
        }

        // A dictionary now exists and at least some rows are stored compressed.
        {
            let conn = Connection::open(&db_path).unwrap();
            let dict_rows: i64 = conn
                .query_row("SELECT count(*) FROM compression_dict", [], |r| r.get(0))
                .unwrap();
            assert_eq!(dict_rows, 1, "dictionary should exist after reopen");
            // 0x28 0xB5 0x2F 0xFD is the zstd magic number (Little Endian).
            let compressed: i64 = conn
                .query_row(
                    "SELECT count(*) FROM events WHERE hex(substr(data, 1, 4)) = '28B52FFD'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(compressed > 0, "expected some rows to be stored compressed");
        }

        // Best-effort cleanup: on Windows the worker may still hold the file
        // handle briefly after close(), which would make a hard unwrap flaky.
        let _ = std::fs::remove_file(&db_path);
    }

    /// Upgrading a populated pre-v6 database should train and apply compression
    /// on the very first open (not only after a second restart). Regression test
    /// for the db_version being captured before migrations ran.
    #[test]
    #[cfg(feature = "compression-zstd")]
    fn test_migration_v5_to_v6_trains_on_first_open() {
        let mut db_path = get_cache_dir().unwrap();
        db_path.push("datastore-unittest-migration-v6.db");
        let db_path_str = db_path.to_str().unwrap().to_string();
        if db_path.exists() {
            std::fs::remove_file(&db_path).unwrap();
        }

        // Build a v5 database with enough repetitive events to train a dictionary.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE buckets (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT UNIQUE NOT NULL, type TEXT NOT NULL, client TEXT NOT NULL,
                    hostname TEXT NOT NULL, created TEXT NOT NULL,
                    data TEXT NOT NULL DEFAULT '{}'
                );
                CREATE INDEX bucket_id_index ON buckets(id);
                CREATE TABLE events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    bucketrow INTEGER NOT NULL, starttime INTEGER NOT NULL,
                    endtime INTEGER NOT NULL, data TEXT NOT NULL,
                    FOREIGN KEY (bucketrow) REFERENCES buckets(id)
                );
                CREATE INDEX events_bucketrow_starttime_endtime_index
                    ON events(bucketrow, starttime DESC, endtime);
                CREATE TABLE key_value (key TEXT PRIMARY KEY, value TEXT, last_modified NUMBER NOT NULL);
                INSERT INTO buckets (name, type, client, hostname, created, data)
                    VALUES ('testid', 'testtype', 'testclient', 'testhost',
                            '2024-01-01T00:00:00+00:00', '{}');
                WITH RECURSIVE seq(x) AS (
                    SELECT 1 UNION ALL SELECT x + 1 FROM seq WHERE x < 3000
                )
                INSERT INTO events (bucketrow, starttime, endtime, data)
                    SELECT 1, x * 1000000000, x * 1000000000 + 1000000000,
                           '{"app":"App' || (x % 4) || '","title":"Window title ' || (x % 50) || '"}'
                    FROM seq;
                PRAGMA user_version = 5;
            "#,
            )
            .unwrap();
        }

        // First open migrates to v6 AND sets up compression in the same session.
        {
            let ds = Datastore::new(db_path_str, false);
            let events = ds.get_events("testid", None, None, None).unwrap();
            assert_eq!(events.len(), 3000);
            // newest-first ordering: last inserted was x=3000 -> App0
            assert_eq!(events[0].data["app"], json!("App0"));
            ds.close();
        }

        // A dictionary was trained and some rows compressed on that first open.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            let version: i32 = conn
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version, 6);
            let dict_rows: i64 = conn
                .query_row("SELECT count(*) FROM compression_dict", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                dict_rows, 1,
                "dictionary should be trained on the first open after upgrade"
            );
            let compressed: i64 = conn
                .query_row(
                    "SELECT count(*) FROM events WHERE hex(substr(data, 1, 4)) = '28B52FFD'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                compressed > 0,
                "expected rows to be compressed after first open"
            );
        }

        // Best-effort cleanup: on Windows the worker may still hold the file
        // handle briefly after close(), which would make a hard unwrap flaky.
        let _ = std::fs::remove_file(&db_path);
    }

    /// A row that looks compressed (zstd magic) but can't be decompressed — e.g.
    /// the dictionary is missing or the feature is disabled — must be skipped
    /// with a warning, never panic the worker. Regression test for the previous
    /// unwrap() on the decompression-failure path.
    #[test]
    fn test_unreadable_compressed_row_does_not_panic() {
        let mut db_path = get_cache_dir().unwrap();
        db_path.push("datastore-unittest-badrow.db");
        let db_path_str = db_path.to_str().unwrap().to_string();
        if db_path.exists() {
            std::fs::remove_file(&db_path).unwrap();
        }

        // Create a v6 database with one valid event.
        {
            let ds = Datastore::new(db_path_str.clone(), false);
            ds.create_bucket(&test_bucket()).unwrap();
            ds.insert_events(
                "testid",
                &[Event {
                    id: None,
                    timestamp: Utc::now(),
                    duration: Duration::seconds(1),
                    data: json_map! {"app": json!("ok")},
                }],
            )
            .unwrap();
            ds.force_commit().unwrap();
            ds.close();
        }

        // Inject a row whose data starts with the zstd magic number but is not a
        // valid frame, and for which no dictionary exists.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            let bid: i64 = conn
                .query_row("SELECT id FROM buckets WHERE name = 'testid'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            conn.execute(
                "INSERT INTO events (bucketrow, starttime, duration, data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    bid,
                    5_000_000_000i64,
                    1_000_000_000i64,
                    vec![0x28u8, 0xB5, 0x2F, 0xFD, 0xDE, 0xAD]
                ],
            )
            .unwrap();
        }

        // Reading must not panic: the bad row is skipped, the good row returned.
        {
            let ds = Datastore::new(db_path_str, false);
            let events = ds.get_events("testid", None, None, None).unwrap();
            assert_eq!(
                events.len(),
                1,
                "the unreadable row should be skipped, the valid one kept"
            );
            assert_eq!(events[0].data["app"], json!("ok"));
            ds.close();
        }

        // Best-effort cleanup: on Windows the worker may still hold the file
        // handle briefly after close(), which would make a hard unwrap flaky.
        let _ = std::fs::remove_file(&db_path);
    }
}
