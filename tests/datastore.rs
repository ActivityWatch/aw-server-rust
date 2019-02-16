extern crate chrono;
extern crate aw_server;
extern crate serde_json;

#[cfg(test)]
mod datastore_tests {
    use chrono::Utc;
    use chrono::Duration;
    use serde_json::json;

    use aw_server::datastore::Datastore;
    use aw_server::models::Bucket;
    use aw_server::models::Event;

    #[test]
    fn test_datastore() {
        // Setup datastore
        let ds = Datastore::new_in_memory();
        //let conn = ds.setup("/tmp/test.db".to_string());

        // Create bucket
        let bucket = Bucket {
            bid: None,
            id: "testid".to_string(),
            _type: "testtype".to_string(),
            client: "testclient".to_string(),
            hostname: "testhost".to_string(),
            created: Some(Utc::now()),
            events: None
        };
        ds.create_bucket(&bucket).unwrap();

        // Fetch bucket
        let bucket_fetched = ds.get_bucket(&bucket.id).unwrap();
        assert_eq!(bucket_fetched.id, bucket.id);
        assert_eq!(bucket_fetched._type, bucket._type);
        assert_eq!(bucket_fetched.client, bucket.client);
        assert_eq!(bucket_fetched.hostname, bucket.hostname);
        assert_eq!(bucket_fetched.created, bucket.created);

        // Fetch all buckets
        let fetched_buckets = ds.get_buckets().unwrap();
        assert_eq!(fetched_buckets.len(), 1);

        // Insert event
        let e1 = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(0),
            data: json!({"key": "value"})
        };
        let mut e2 = e1.clone();
        e2.timestamp = e2.timestamp + Duration::nanoseconds(1);
        let mut e_replace = e2.clone();
        e_replace.data = json!({"key": "value2"});
        e_replace.duration = Duration::seconds(2);

        let mut event_list = Vec::new();
        event_list.push(e1.clone());
        event_list.push(e2.clone());

        ds.insert_events(&bucket.id, &event_list).unwrap();

        ds.replace_last_event(&bucket.id, &e_replace).unwrap();

        // Get all events
        let fetched_events_all = ds.get_events(&bucket.id, None, None, None).unwrap();
        let expected_fetched_events = vec![&e_replace, &e1];
        assert_eq!(fetched_events_all.len(), 2);
        for i in 0..fetched_events_all.len() {
            let expected = &expected_fetched_events[i];
            let new = &fetched_events_all[i];
            assert_eq!(new.timestamp,expected.timestamp);
            assert_eq!(new.duration,expected.duration);
            assert_eq!(new.data,expected.data);
        }

        println!("Get events with limit filter");
        let fetched_events_limit = ds.get_events(&bucket.id, None, None, Some(1)).unwrap();
        assert_eq!(fetched_events_limit.len(), 1);
        assert_eq!(fetched_events_limit[0].timestamp,e_replace.timestamp);
        assert_eq!(fetched_events_limit[0].duration,e_replace.duration);
        assert_eq!(fetched_events_limit[0].data,e_replace.data);

        println!("Get events with starttime filter");
        let fetched_events_start = ds.get_events(&bucket.id, Some(e2.timestamp.clone()), None, None).unwrap();
        assert_eq!(fetched_events_start.len(), 1);
        assert_eq!(fetched_events_start[0].timestamp,e_replace.timestamp);
        assert_eq!(fetched_events_start[0].duration,e_replace.duration);
        assert_eq!(fetched_events_start[0].data,e_replace.data);

        println!("Get events with endtime filter");
        let fetched_events_start = ds.get_events(&bucket.id, None, Some(e1.timestamp.clone()), None).unwrap();
        assert_eq!(fetched_events_start.len(), 1);
        assert_eq!(fetched_events_start[0].timestamp,e1.timestamp);
        assert_eq!(fetched_events_start[0].duration,e1.duration);
        assert_eq!(fetched_events_start[0].data,e1.data);

        // Get eventcount
        let event_count = ds.get_event_count(&bucket.id, None, None).unwrap();
        assert_eq!(event_count, 2);

        // Delete bucket
        match ds.delete_bucket(&bucket.id) {
            Ok(_) => println!("bucket successfully deleted"),
            Err(e) => panic!(e)
        }
        match ds.get_bucket(&bucket.id) {
            Ok(_) => panic!("Expected datastore to delete bucket but bucket seems to still be available"),
            Err(_e) => ()
        }
    }
}
