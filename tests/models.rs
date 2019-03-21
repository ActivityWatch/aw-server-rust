#[macro_use]
extern crate log;
extern crate chrono;
extern crate serde_json;

extern crate aw_server;

#[cfg(test)]
mod models_tests {
    use std::str::FromStr;

    use serde_json::json;
    use chrono::DateTime;
    use chrono::Utc;
    use chrono::Duration;

    use aw_server::models::Bucket;
    use aw_server::models::BucketMetadata;
    use aw_server::models::Event;
    use aw_server::models::TimeInterval;

    #[test]
    fn test_bucket() {
        let b = Bucket {
            bid: None,
            id: "id".to_string(),
            _type: "type".to_string(),
            client: "client".to_string(),
            hostname: "hostname".to_string(),
            created: None,
            data: json!("{}"),
            metadata: BucketMetadata::default(),
            events: None
        };
        debug!("bucket: {:?}", b);
    }

    #[test]
    fn test_event() {
        let e = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        debug!("event: {:?}", e);
    }

    #[test]
    fn test_timeinterval() {
        let start = DateTime::from_str("2000-01-01T00:00:00Z").unwrap();
        let end = DateTime::from_str("2000-01-02T00:00:00Z").unwrap();
        let period_str = "2000-01-01T00:00:00+00:00/2000-01-02T00:00:00+00:00";
        let duration = end - start;
        let tp = TimeInterval::new(start, end);
        assert_eq!(tp.start(), &start);
        assert_eq!(tp.end(), &end);
        assert_eq!(tp.duration(), duration);
        assert_eq!(tp.to_string(), period_str);

        let tp = TimeInterval::new_from_string(period_str).unwrap();
        assert_eq!(tp.start(), &start);
        assert_eq!(tp.end(), &end);
        assert_eq!(tp.duration(), duration);
        assert_eq!(tp.to_string(), period_str);
    }
}
