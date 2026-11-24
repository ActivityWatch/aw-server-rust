use aw_models::Bucket;

/// Finds the bucket which starts with the specified string, and optionally matches a
/// hostname. An exact match wins; otherwise the lexicographically smallest prefix match is
/// returned, so the result does not depend on the iteration order of `buckets`.
pub fn find_bucket<'a>(
    bucket_filter: &str,
    hostname_filter: &Option<String>,
    buckets: impl IntoIterator<Item = &'a Bucket>,
) -> Option<String> {
    let mut prefix_match = None;
    for bucket in buckets {
        if bucket.id.starts_with(bucket_filter) {
            let hostname_matches = match hostname_filter {
                Some(hostname) => hostname == &bucket.hostname,
                None => true,
            };

            if hostname_matches {
                if bucket.id == bucket_filter {
                    // Exact match, return immediately
                    return Some(bucket.id.to_string());
                }
                // Keep the lexicographically smallest prefix match, not the first one seen:
                // callers pass HashMap values, whose order is random per process, and the
                // window and afk lookups must resolve to the same host.
                let smaller = match &prefix_match {
                    Some(m) => bucket.id < *m,
                    None => true,
                };
                if smaller {
                    prefix_match = Some(bucket.id.to_string());
                }
            }
        }
    }
    prefix_match
}

#[cfg(test)]
mod tests {
    use super::find_bucket;
    use aw_models::Bucket;
    use aw_models::BucketMetadata;

    #[test]
    fn prefix_match_does_not_depend_on_iteration_order() {
        // get_buckets() returns a HashMap, so the iteration order is random per process.
        // With two hosts, the window and afk lookups must resolve to the same host,
        // whatever order the buckets come in.
        let bucket = |id: &str| Bucket {
            bid: None,
            id: id.to_string(),
            _type: "type".to_string(),
            hostname: "h".to_string(),
            client: "testclient".to_string(),
            created: None,
            data: json_map! {},
            metadata: BucketMetadata::default(),
            events: None,
            last_updated: None,
        };
        let a = bucket("aw-watcher-window_hostA");
        let b = bucket("aw-watcher-window_hostB");
        let forward = find_bucket("aw-watcher-window_", &None, vec![&a, &b]);
        let backward = find_bucket("aw-watcher-window_", &None, vec![&b, &a]);
        assert_eq!(forward, backward);
        assert_eq!(forward.as_deref(), Some("aw-watcher-window_hostA"));
    }

    #[test]
    fn test_find_bucket() {
        let expected_bucketname = "aw-datastore-test_test-host".to_string();
        let expected_hostname = "testhost".to_string();
        let b1 = Bucket {
            bid: None,
            id: "no match".to_string(),
            _type: "type".to_string(),
            hostname: expected_hostname,
            client: "testclient".to_string(),
            created: None,
            data: json_map! {},
            metadata: BucketMetadata::default(),
            events: None,
            last_updated: None,
        };
        let mut b2 = b1.clone();
        b2.id = expected_bucketname.clone();
        let b3 = b1.clone();

        let buckets = vec![b1.clone(), b2, b3.clone()];

        // Check that it correctly finds bucket
        let res = find_bucket("aw-datastore-test", &Some("testhost".to_string()), &buckets);
        assert_eq!(res, Some(expected_bucketname));

        // Check that it doesn't find a bucket for an unavailable hostname
        let res = find_bucket(
            "aw-datastore-test",
            &Some("unavailablehost".to_string()),
            &buckets,
        );
        assert_eq!(res, None);

        // Check that it doesn't find a bucket for any hostname
        let buckets = vec![b1, b3];
        let res = find_bucket("aw-datastore-test", &None, &buckets);
        assert_eq!(res, None);
    }
}
