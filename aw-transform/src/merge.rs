use std::collections::HashMap;

use aw_models::Event;

/// Merge events with the same values at the specified keys
///
/// Doesn't care about if events are neighbouring or not, this transform merges
/// all events with the same key.
/// Each merged event keeps the timestamp and the whole data of the first event
/// with those key values, and the durations are summed. Events missing any of
/// the keys are dropped, and an empty key list returns no events. The merged
/// events come out in the order their first event appeared in the input, so
/// the result is deterministic. That is not necessarily timestamp order: sort
/// by timestamp before passing the result to union_no_overlap, which expects
/// chronological input.
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
    // Index into `merged` by key, so the output keeps first-seen order
    // instead of HashMap iteration order, which differs between runs.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut merged: Vec<Event> = Vec::new();
    'event: for mut event in events {
        let mut key_values = Vec::new();
        for key in &keys {
            match event.data.get(key) {
                Some(v) => key_values.push(v.to_string()),
                None => continue 'event,
            }
        }
        // The category component of the merge key is what keeps events with
        // the same grouping values but different categories from collapsing
        // into one row that keeps only the first event's category and
        // misattributes the summed duration.
        //
        // - A `$category` merge already groups by the final category, so no
        //   extra component is needed: a manual and an automatic event that
        //   landed in the same category must sum into one row.
        // - Otherwise the component is the event's effective category: its
        //   manual override if it has one, else its `$category`. After
        //   `categorize` this equals `$category` (a valid manual override
        //   always wins there), so a manual and an automatic event that agree
        //   on the final category still fold into one row. Before
        //   classification `query_bucket` events carry no `$category` (stored
        //   copies are stripped), so manual events stay apart from automatic
        //   ones and from each other.
        let split = if keys.iter().any(|key| key == "$category") {
            None
        } else {
            crate::classify::manual_category(&event)
                .map(|path| serde_json::json!(path))
                .or_else(|| event.data.get("$category").cloned())
        };
        let summed_key = serde_json::to_string(&(key_values, split)).unwrap();
        match index.entry(summed_key) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                merged[*entry.get()].duration += event.duration;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                event.id = None;
                entry.insert(merged.len());
                merged.push(event);
            }
        }
    }
    merged
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

    #[test]
    fn merge_keeps_first_seen_order() {
        // HashMap iteration order would vary between runs; the output must
        // follow the order in which each group first appears.
        let mk = |secs: &str, app: &str| Event {
            id: None,
            timestamp: DateTime::from_str(secs).unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"app": json!(app)},
        };
        let apps = ["m", "c", "x", "a", "q", "b", "z", "d", "k", "e"];
        let mut events = Vec::new();
        for (i, app) in apps.iter().enumerate() {
            events.push(mk(&format!("2000-01-01T00:00:{:02}Z", i), app));
        }
        // Repeats merge into the first occurrence without reordering
        events.push(mk("2000-01-01T00:00:30Z", "a"));
        events.push(mk("2000-01-01T00:00:31Z", "m"));
        let res = merge_events_by_keys(events, vec!["app".to_string()]);
        let order: Vec<&str> = res
            .iter()
            .map(|e| e.data.get("app").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(order, apps);
        assert_eq!(res[0].duration, Duration::seconds(2));
        assert_eq!(res[3].duration, Duration::seconds(2));
    }
}
