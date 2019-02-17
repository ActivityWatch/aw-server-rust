#[macro_use]
extern crate log;
extern crate chrono;
extern crate serde_json;

extern crate aw_server;

#[cfg(test)]
mod transform_tests {
    use std::str::FromStr;

    use chrono::Utc;
    use chrono::DateTime;
    use chrono::Duration;
    use serde_json::json;

    use aw_server::models::Event;
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

    #[test]
    fn test_sort_by_timestamp() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let res = transform::sort_by_timestamp(vec![e2.clone(), e1.clone()]);
        assert_eq!(res, vec![e1, e2]);
    }

    #[test]
    fn test_sort_by_duration() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(2),
            data: json!({"test": 1})
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let res = transform::sort_by_duration(vec![e2.clone(), e1.clone()]);
        assert_eq!(res, vec![e1, e2]);
    }

    #[test]
    fn test_flood() {
        // Test merging of events with the same data
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let e_expected = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(4),
            data: json!({"test": 1})
        };
        let res = transform::flood(vec![e1.clone(), e2.clone()], Duration::seconds(5));
        assert_eq!(1, res.len());
        assert_eq!(&res[0], &e_expected);

        // Test flood gap between two different events which should meet in the middle
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 2})
        };
        let e1_expected = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(2),
            data: json!({"test": 1})
        };
        let e2_expected = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:02Z").unwrap(),
            duration: Duration::seconds(2),
            data: json!({"test": 2})
        };
        let res = transform::flood(vec![e1.clone(), e2.clone()], Duration::seconds(5));
        assert_eq!(2, res.len());
        assert_eq!(&res[0], &e1_expected);
        assert_eq!(&res[1], &e2_expected);
    }

    #[test]
    fn test_merge_events_by_key() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(3),
            data: json!({"test2": 3})
        };
        let e3 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:02Z").unwrap(),
            duration: Duration::seconds(7),
            data: json!({"test": 6})
        };
        let e4 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(9),
            data: json!({"test": 1})
        };
        let in_events = vec![e1.clone(), e2.clone(), e3.clone(), e4.clone()];
        let res1 = transform::merge_events_by_keys (in_events, vec!["test".to_string()]);
        // Needed, otherwise the order is undeterministic
        let res2 = transform::sort_by_timestamp (res1);
        let expected = vec![
            Event {
                id: None,
                timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
                duration: Duration::seconds(10),
                data: json!({"test": 1})
            },
            Event {
                id: None,
                timestamp: DateTime::from_str("2000-01-01T00:00:02Z").unwrap(),
                duration: Duration::seconds(7),
                data: json!({"test": 6})
            }
        ];
        assert_eq!(&res2, &expected);
    }

    #[test]
    fn test_filter_keyvals () {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let mut e2 = e1.clone();
        e2.data = json!({"test": 2});
        let mut e3 = e1.clone();
        e3.data = json!({"test2": 2});
        let res = transform::filter_keyvals(vec![e1.clone(), e2, e3], "test", &vec![json!(1)]);
        assert_eq!(vec![e1], res);
    }

    #[test]
    fn test_filter_period_intersect() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let mut e2 = e1.clone();
        e2.timestamp = DateTime::from_str("2000-01-01T00:00:02Z").unwrap();
        let mut e3 = e1.clone();
        e3.timestamp = DateTime::from_str("2000-01-01T00:00:03Z").unwrap();
        let mut e4 = e1.clone();
        e4.timestamp = DateTime::from_str("2000-01-01T00:00:04Z").unwrap();
        let mut e5 = e1.clone();
        e5.timestamp = DateTime::from_str("2000-01-01T00:00:05Z").unwrap();

        let filter_event = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:02.5Z").unwrap(),
            duration: Duration::seconds(2),
            data: json!({"test": 1})
        };

        let filtered_events = transform::filter_period_intersect(&vec![e1, e2, e3, e4, e5], &vec![filter_event]);
        assert_eq!(filtered_events.len(), 3);
        assert_eq!(filtered_events[0].duration, Duration::milliseconds(500));
        assert_eq!(filtered_events[1].duration, Duration::milliseconds(1000));
        assert_eq!(filtered_events[2].duration, Duration::milliseconds(500));

        let dt : DateTime<Utc> = DateTime::from_str("2000-01-01T00:00:02.500Z").unwrap();
        assert_eq!(filtered_events[0].timestamp, dt);
        let dt : DateTime<Utc> = DateTime::from_str("2000-01-01T00:00:03.000Z").unwrap();
        assert_eq!(filtered_events[1].timestamp, dt);
        let dt : DateTime<Utc> = DateTime::from_str("2000-01-01T00:00:04.000Z").unwrap();
        assert_eq!(filtered_events[2].timestamp, dt);
    }

    #[test]
    fn test_chunk_events_by_key() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"test": 1})
        };
        let mut e2 = e1.clone();
        e2.data = json!({"test2": 1});
        let e3 = e1.clone();
        let mut e4 = e1.clone();
        e4.data = json!({"test": 2});

        let res = transform::chunk_events_by_key(vec![e1, e2, e3, e4], "test");
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].duration, Duration::seconds(2));
        assert_eq!(res[1].duration, Duration::seconds(1));
    }

    #[test]
    fn test_split_url_events() {
        let mut e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json!({"url": "http://www.google.com/path?query=1"})
        };
        transform::split_url_event(&mut e1);
        assert_eq!(e1.data, json!({
            "url": "http://www.google.com/path?query=1",
            "protocol": "http",
            "domain": "google.com",
            "path": "/path",
            "params": "query=1"
        }));
    }
}
