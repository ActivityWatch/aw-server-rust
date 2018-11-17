extern crate chrono;
extern crate serde_json;

extern crate aw_server;

#[cfg(test)]
mod models_tests {
    use serde_json::json;
    use chrono::Utc;
    use chrono::Duration;

    use aw_server::models::bucket::Bucket;
    use aw_server::models::event::Event;

    #[test]
    fn test_bucket() {
        let b = Bucket {
            bid: None,
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
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        println!("event: {:?}", e);
    }
}
