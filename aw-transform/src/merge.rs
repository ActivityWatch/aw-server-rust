use std::collections::HashMap;

use aw_models::Event;

/// Merge events with the same values at the specified keys
///
/// Doesn't care about if events are neighbouring or not, this transform merges
/// all events with the same key.
/// The timestamp will be the timestamp of the first event with a specific key value
///
/// # Example 1
/// A simple example only using one key
///
/// ```ignore
/// keys: ["a"]
/// input:
///   { duration: 1.0, data: { "a": 1 } }
///   { duration: 1.0, data: { "a": 1 } }
///   { duration: 1.0, data: { "a": 2 } }
///   { duration: 1.0, data: { "b": 1 } }
///   { duration: 1.0, data: { "a": 1 } }
/// output:
///   { duration: 3.0, data: { "a": 1 } }
///   { duration: 1.0, data: { "a": 2 } }
///   { duration: 1.0, data: { "b": 1 } }
/// ```
///
/// # Example 2
/// A more complex example only using two keys
/// ```ignore
/// keys: ["a", "b"]
/// input:
///   { duration: 1.0, data: { "a": 1, "b": 1 } }
///   { duration: 1.0, data: { "a": 2, "b": 2 } }
///   { duration: 1.0, data: { "a": 1, "b": 1 } }
///   { duration: 1.0, data: { "a": 1, "b": 2 } }
/// output:
///   { duration: 2.0, data: { "a": 1, "b": 1 } }
///   { duration: 1.0, data: { "a": 2, "b": 2 } }
///   { duration: 1.0, data: { "a": 1, "b": 2 } }
/// ```
pub fn merge_events_by_keys(events: Vec<Event>, keys: Vec<String>) -> Vec<Event> {
    if keys.is_empty() {
        return vec![];
    }
    let mut merged_events_map: HashMap<String, Event> = HashMap::new();
    'event: for mut event in events {
        let mut key_values = Vec::new();
        for key in &keys {
            match event.data.get(key) {
                Some(v) => key_values.push(v.to_string()),
                None => continue 'event,
            }
        }
        let summed_key = key_values.join(".");
        match merged_events_map.entry(summed_key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().duration += event.duration;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                event.id = None;
                entry.insert(event);
            }
        }
    }
    merged_events_map.into_values().collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::DateTime;
    use chrono::Duration;
    use serde_json::json;

    use aw_models::Event;

    use crate::sort_by_timestamp;

    use super::merge_events_by_keys;

    #[test]
    fn merge_preserves_first_payload_and_clears_id() {
        let first = Event {
            id: Some(42),
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(2),
            data: json_map! {"app": json!("browser"), "title": json!("page"), "extra": json!({"nested": [1, 2]})},
        };
        let mut second = first.clone();
        second.timestamp += Duration::seconds(10);
        second.data.insert("extra".into(), json!("different"));
        let mut missing = first.clone();
        missing.data.remove("title");
        let result = merge_events_by_keys(
            vec![first.clone(), second, missing],
            vec!["app".into(), "title".into()],
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, None);
        assert_eq!(result[0].timestamp, first.timestamp);
        assert_eq!(result[0].data, first.data);
        assert_eq!(result[0].duration, Duration::seconds(4));
    }

    #[test]
    fn test_merge_events_by_key() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let e2 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(3),
            data: json_map! {"test2": json!(3)},
        };
        let e3 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:02Z").unwrap(),
            duration: Duration::seconds(7),
            data: json_map! {"test": json!(6)},
        };
        let e4 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:03Z").unwrap(),
            duration: Duration::seconds(9),
            data: json_map! {"test": json!(1)},
        };
        let in_events = vec![e1, e2, e3, e4];
        let res1 = merge_events_by_keys(in_events, vec!["test".to_string()]);
        // Needed, otherwise the order is undeterministic
        let res2 = sort_by_timestamp(res1);
        let expected = vec![
            Event {
                id: None,
                timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
                duration: Duration::seconds(10),
                data: json_map! {"test": json!(1)},
            },
            Event {
                id: None,
                timestamp: DateTime::from_str("2000-01-01T00:00:02Z").unwrap(),
                duration: Duration::seconds(7),
                data: json_map! {"test": json!(6)},
            },
        ];
        assert_eq!(&res2, &expected);
    }
}
