extern crate chrono;
extern crate aw_server;
extern crate serde_json;

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use aw_server::datastore;
    use aw_server::models::bucket::Bucket;
    use aw_server::models::event::Event;
    use aw_server::models::duration::Duration;

    #[test]
    fn test_datastore() {
        // Setup datastore
        let conn = datastore::setup_memory();
        //let conn = datastore::setup("/tmp/test.db".to_string());

        // Create bucket
        let bucket = Bucket {
            id: "testid".to_string(),
            _type: "testtype".to_string(),
            client: "testclient".to_string(),
            hostname: "testhost".to_string(),
            created: Some(Utc::now()),
        };
        datastore::create_bucket(&conn, &bucket).unwrap();

        // Fetch bucket
        let bucket_fetched = datastore::get_bucket(&conn, &bucket.id).unwrap();
        assert_eq!(bucket_fetched.id, bucket.id);
        assert_eq!(bucket_fetched._type, bucket._type);
        assert_eq!(bucket_fetched.client, bucket.client);
        assert_eq!(bucket_fetched.hostname, bucket.hostname);
        assert_eq!(bucket_fetched.created, bucket.created);

        // Fetch all buckets
        let fetched_buckets = datastore::get_buckets(&conn).unwrap();
        assert_eq!(fetched_buckets.len(), 1);

        // Insert event
        let e1 = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::from_seconds(1.0),
            data: json!({"key": "value"})
        };
        let mut e2 = e1.clone();
        e2.timestamp = Utc::now();
        let mut e_replace = e2.clone();
        e_replace.data = json!({"key": "value2"});
        e_replace.duration = Duration::from_seconds(2.0);

        let mut event_list = Vec::new();
        event_list.push(e1.clone());
        event_list.push(e2);

        datastore::insert_events(&conn, &bucket.id, &event_list).unwrap();

        datastore::replace_last_event(&conn, &bucket.id, &e_replace).unwrap();

        // Get events
        let fetched_events = datastore::get_events(&conn, &bucket.id, None, None, None).unwrap();
        let expected_fetched_events = vec![e_replace, e1];
        assert_eq!(fetched_events.len(), 2);
        for i in 0..fetched_events.len() {
            let expected = &expected_fetched_events[i];
            let new = &fetched_events[i];
            assert_eq!(new.timestamp,expected.timestamp);
            assert_eq!(new.duration,expected.duration);
            assert_eq!(new.data,expected.data);
        }

        // Get eventcount
        let event_count = datastore::get_events_count(&conn, &bucket.id, None, None).unwrap();
        assert_eq!(event_count, 2);
    }
}
