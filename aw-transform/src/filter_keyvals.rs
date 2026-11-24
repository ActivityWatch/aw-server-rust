use std::convert::Infallible;

use fancy_regex::Regex;
use serde_json::value::Value;

use aw_models::Event;

/// Drops events not matching the specified key and value(s)
///
/// # Example
/// ```ignore
///  key: a
///  vals: [1,2]
///  input:  [a:1][a:2][a:3][b:4]
///  output: [a:1][a:2]
/// ```
pub fn filter_keyvals(mut events: Vec<Event>, key: &str, vals: &[Value]) -> Vec<Event> {
    let mut filtered_events = Vec::new();
    for event in events.drain(..) {
        if let Some(v) = event.data.get(key) {
            for val in vals {
                if val == v {
                    filtered_events.push(event);
                    break;
                }
            }
        }
    }
    filtered_events
}

/// Drops events not matching the regex on the value for a specified key
/// Will only match if the value is a string
///
/// # Example
/// ```ignore
/// key: a
/// regex: "[A-Z]+"
/// input:  [a:"HELLO"][a:"hello"][a:3][b:"HELLO"]
/// output: [a:"HELLO"]
/// ```
pub fn filter_keyvals_regex(events: Vec<Event>, key: &str, regex: &Regex) -> Vec<Event> {
    match filter_keyvals_regex_checked(events, key, regex, || Ok::<(), Infallible>(())) {
        Ok(events) => events,
        Err(never) => match never {},
    }
}

/// Like [`filter_keyvals_regex`], but calls `check` before every event and
/// once more at the end, and stops with the first error it returns. A single
/// regex match cannot be interrupted, so a deadline enforced through `check`
/// can be overshot by up to one match.
pub fn filter_keyvals_regex_checked<E>(
    mut events: Vec<Event>,
    key: &str,
    regex: &Regex,
    mut check: impl FnMut() -> Result<(), E>,
) -> Result<Vec<Event>, E> {
    let mut filtered_events = Vec::new();

    for event in events.drain(..) {
        check()?;
        if let Some(value) = event.data.get(key).and_then(|v| v.as_str()) {
            match regex.is_match(value) {
                Ok(true) => filtered_events.push(event),
                Ok(false) => (),
                Err(err) => warn!("Failed to run regex: {}", err),
            };
        }
    }
    check()?;
    Ok(filtered_events)
}

/// Drops events matching the specified key and value(s). Opposite of filter_keyvals.
///
/// # Example
/// ```ignore
///  key: a
///  vals: [1,2]
///  input:  [a:1][a:2][a:3][b:4]
///  output: [a:3][b:4]
/// ```
pub fn exclude_keyvals(mut events: Vec<Event>, key: &str, vals: &[Value]) -> Vec<Event> {
    let mut filtered_events = Vec::new();
    'events: for event in events.drain(..) {
        if let Some(v) = event.data.get(key) {
            for val in vals {
                if val == v {
                    continue 'events;
                }
            }
        }
        filtered_events.push(event);
    }
    filtered_events
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::{DateTime, Duration};
    use fancy_regex::RegexBuilder;
    use serde_json::json;

    use aw_models::Event;

    use super::{
        exclude_keyvals, filter_keyvals, filter_keyvals_regex, filter_keyvals_regex_checked,
    };

    #[test]
    fn test_filter_keyvals() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let mut e2 = e1.clone();
        e2.data = json_map! {"test": json!(1), "test2": json!(1)};
        let mut e3 = e1.clone();
        e3.data = json_map! {"test2": json!(2)};
        let res = filter_keyvals(vec![e1.clone(), e2.clone(), e3], "test", &[json!(1)]);
        assert_eq!(vec![e1, e2], res);
    }

    #[test]
    fn test_filter_keyvals_regex() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"key1": json!("value1")},
        };
        let mut e2 = e1.clone();
        e2.data = json_map! {"key1": json!("value2")};
        let mut e3 = e1.clone();
        e3.data = json_map! {"key2": json!("value3")};

        let events = vec![e1.clone(), e2.clone(), e3.clone()];

        let regex_value = RegexBuilder::new("value").build().unwrap();
        let regex_value1 = RegexBuilder::new("value1").build().unwrap();

        let res = filter_keyvals_regex(events.clone(), "key1", &regex_value);
        assert_eq!(vec![e1.clone(), e2], res);
        let res = filter_keyvals_regex(events.clone(), "key1", &regex_value1);
        assert_eq!(vec![e1], res);
        let res = filter_keyvals_regex(events.clone(), "key2", &regex_value);
        assert_eq!(vec![e3], res);
        let res = filter_keyvals_regex(events.clone(), "key2", &regex_value1);
        assert_eq!(0, res.len());
        let res = filter_keyvals_regex(events, "key3", &regex_value);
        assert_eq!(0, res.len());
    }

    #[test]
    fn test_filter_keyvals_regex_non_string_data_value() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"key1": json!(100)},
        };
        let events = vec![e1.clone()];
        let regex_value = RegexBuilder::new("value").build().unwrap();
        let res = filter_keyvals_regex(events.clone(), "key1", &regex_value);
        assert!(res.is_empty());
    }

    #[test]
    fn test_exclude_keyvals() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let mut e2 = e1.clone();
        e2.data = json_map! {"test": json!(1), "test2": json!(2)};
        let mut e3 = e1.clone();
        e3.data = json_map! {"test": json!(2)};
        let res = exclude_keyvals(vec![e1.clone(), e2.clone(), e3], "test", &[json!(2)]);
        assert_eq!(vec![e1, e2], res);
    }

    #[test]
    fn test_filter_keyvals_regex_checked_propagates_every_cancellation_point() {
        let event = |value: &str| Event {
            data: json_map! {"test": json!(value)},
            ..Event::default()
        };
        let events = vec![event("aa"), event("b"), event("aaa")];
        let regex = RegexBuilder::new("^a+$").build().unwrap();
        let mut total = 0;
        let kept = filter_keyvals_regex_checked(events.clone(), "test", &regex, || {
            total += 1;
            Ok::<(), usize>(())
        })
        .unwrap();
        assert_eq!(kept, filter_keyvals_regex(events.clone(), "test", &regex));
        // One check per event plus one at the end.
        assert_eq!(total, 4);
        for fail_at in 1..=total {
            let mut calls = 0;
            let result = filter_keyvals_regex_checked(events.clone(), "test", &regex, || {
                calls += 1;
                if calls == fail_at {
                    Err(calls)
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.err(), Some(fail_at));
        }
    }
}
