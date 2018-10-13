extern crate chrono;
extern crate serde_json;

extern crate aw_server;

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use chrono::Duration;
    use serde_json::json;

    use aw_server::models::event::Event;
    use aw_server::transform;

    #[test]
    fn test_heartbeat_pulsetime() {
        let now = Utc::now();
        let event1 = Event {
            id: None,
            timestamp: now,
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let heartbeat1 = Event {
            id: None,
            timestamp: now + Duration::seconds(2),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };

        // Merge result
        let res_merge = transform::heartbeat(&event1, &heartbeat1, 2.0).unwrap();
        assert!(res_merge.duration == Duration::seconds(3));

        // No merge result
        let res_no_merge = transform::heartbeat(&event1, &heartbeat1, 0.0);
        assert!(res_no_merge.is_none());

        // TODO: needs more tests!
    }

    #[test]
    fn test_heartbeat_data() {
        let now = Utc::now();
        let event = Event {
            id: None,
            timestamp: now.clone(),
            duration: Duration::seconds(0),
            data: json!({"test": 1})
        };
        let heartbeat_same_data = Event {
            id: None,
            timestamp: now.clone(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };

        // Data is same, should merge
        let res_merge = transform::heartbeat(&event, &heartbeat_same_data, 1.0);
        assert!(res_merge.is_some());

        let heartbeat_different_data = Event {
            id: None,
            timestamp: now.clone(),
            duration: Duration::seconds(1),
            data: json!({"test": 2})
        };
        // Data is different, should not merge
        let res_merge = transform::heartbeat(&event, &heartbeat_different_data, 1.0);
        assert!(res_merge.is_none());
    }
}
