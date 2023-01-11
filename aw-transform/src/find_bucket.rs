use aw_models::Bucket;

/// Finds the first bucket which starts with the specified string, and optionally matches a
/// hostname.
pub fn find_bucket<'a>(
    bucket_filter: &str,
    hostname_filter: &Option<String>,
    buckets: impl IntoIterator<Item = &'a Bucket>,
) -> Option<String> {
    for bucket in buckets {
        if bucket.id.starts_with(bucket_filter) {
            if let Some(hostname) = hostname_filter {
                if hostname == &bucket.hostname {
                    return Some(bucket.id.to_string());
                }
            } else {
                return Some(bucket.id.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::find_bucket;
    use aw_models::Bucket;
    use aw_models::BucketMetadata;

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
