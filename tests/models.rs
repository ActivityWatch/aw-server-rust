extern crate chrono;
extern crate serde_json;

extern crate aw_server;

#[cfg(test)]
mod tests {
    use serde_json::json;
    use chrono::Utc;

    use aw_server::models::bucket::Bucket;
    use aw_server::models::duration::Duration;
    use aw_server::models::event::Event;

    #[test]
    fn test_bucket() {
        let b = Bucket {
            id: "id".to_string(),
            _type: "type".to_string(),
            client: "client".to_string(),
            hostname: "hostname".to_string(),
            created: None
        };
        println!("bucket: {:?}", b);
    }

    #[test]
    fn test_event() {
        let e = Event {
            id: None,
            timestamp: Utc::now(),
            duration: Duration::from_seconds(1.0),
            data: json!({"test": 1})
        };
        println!("event: {:?}", e);
    }

    #[test]
    fn test_duration() {
        let d_s = Duration::from_seconds(1.2345);
        assert_eq!(d_s.num_seconds(), 1.2345);
        assert_eq!(d_s.num_nanos(), 1234500000);
        println!("seconds: {:?}", d_s);
        let d_ns = Duration::from_nanos(2345678900);
        assert_eq!(d_ns.num_seconds(), 2.3456789);
        assert_eq!(d_ns.num_nanos(), 2345678900);
        println!("seconds: {:?}", d_ns);
    }
}
