extern crate chrono;
extern crate aw_server;

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use aw_server::datastore;
    use aw_server::models::Bucket;

    #[test]
    fn test_bucket() {
        // Setup datastore
        let conn = datastore::setup_memory();

        // Create bucket
        let bucket = Bucket {
            id: "testid".to_string(),
            _type: "testtype".to_string(),
            client: "testclient".to_string(),
            hostname: "testhost".to_string(),
            created: Some(Utc::now()),
        };
        datastore::create_bucket(&conn, &bucket);

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

        // Finished
    }
}
