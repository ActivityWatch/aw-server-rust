pub fn find_bucket<'a>(
    bucket_filter: &str,
    bucketnames: impl IntoIterator<Item = &'a String>,
) -> Option<String> {
    for bucketname in bucketnames {
        if bucketname.starts_with(bucket_filter) {
            return Some(bucketname.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::find_bucket;

    #[test]
    fn test_find_bucket() {
        let expected_bucketname = "aw-datastore-test_test-host".to_string();
        let bucketnames = vec![
            "no match".to_string(),
            expected_bucketname.clone(),
            "no match 2".to_string(),
        ];
        let res = find_bucket("aw-datastore-test", &bucketnames);
        assert_eq!(res, Some(expected_bucketname));

        let bucketnames = vec!["no match".to_string(), "no match 2".to_string()];
        let res = find_bucket("aw-datastore-test", &bucketnames);
        assert_eq!(res, None);
    }
}
