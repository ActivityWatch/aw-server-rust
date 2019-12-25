use serde_json::value::Value;

use aw_models::Event;

pub fn filter_keyvals(events: Vec<Event>, key: &str, vals: &Vec<Value>) -> Vec<Event> {
    let mut filtered_events = Vec::new();
    for event in events {
        match event.data.get(key) {
            Some(v) => match v {
                Value::Null => {
                    for val in vals {
                        match val {
                            Value::Null => filtered_events.push(event.clone()),
                            _ => break
                        }
                    }
                },
                Value::Bool(b1) => {
                    for val in vals {
                        match val {
                            Value::Bool(ref b2) => if b1 == b2 { filtered_events.push(event.clone()) },
                            _ => break
                        }
                    }
                },
                Value::Number(n1) => {
                    for val in vals {
                        match val {
                            Value::Number(ref n2) => if n1 == n2 { filtered_events.push(event.clone()) },
                            _ => break
                        }
                    }
                },
                Value::String(s1) => {
                    for val in vals {
                        match val {
                            Value::String(ref s2) => if s1 == s2 { filtered_events.push(event.clone()) },
                            _ => break
                        }
                    }
                },
                Value::Array(_) => {
                    // TODO: cannot match objects
                    break
                },
                Value::Object(_) => {
                    // TODO: cannot match objects
                    break
                }
            }
            None => break
        }
    }
    return filtered_events;
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::DateTime;
    use chrono::Duration;
    use serde_json::json;

    use aw_models::Event;

    use super::filter_keyvals;

    #[test]
    fn test_filter_keyvals () {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:00Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map!{"test": json!(1)}
        };
        let mut e2 = e1.clone();
        e2.data = json_map!{"test": json!(2)};
        let mut e3 = e1.clone();
        e3.data = json_map!{"test2": json!(2)};
        let res = filter_keyvals(vec![e1.clone(), e2, e3], "test", &vec![json!(1)]);
        assert_eq!(vec![e1], res);
    }
}
