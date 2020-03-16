use aw_models::Event;

pub fn sort_by_timestamp(mut events: Vec<Event>) -> Vec<Event> {
    events.sort_by(|e1, e2| e1.timestamp.cmp(&e2.timestamp));
    events
}

/* Highest first */
pub fn sort_by_duration(mut events: Vec<Event>) -> Vec<Event> {
    events.sort_by(|e1, e2| e2.duration.cmp(&e1.duration));
    events
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::DateTime;
    use chrono::Duration;
    use serde_json::json;

    use aw_models::Event;

    use super::{sort_by_duration, sort_by_timestamp};

    #[test]
    fn test_sort_by_timestamp() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let res = sort_by_timestamp(vec![e2.clone(), e1.clone()]);
        assert_eq!(res, vec![e1, e2]);
    }

    #[test]
    fn test_sort_by_duration() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(2),
            data: json_map! {"test": json!(1)},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let res = sort_by_duration(vec![e2.clone(), e1.clone()]);
        assert_eq!(res, vec![e1, e2]);
    }
}
