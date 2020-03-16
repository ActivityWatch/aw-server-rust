use aw_models::Event;

pub fn chunk_events_by_key(events: Vec<Event>, key: &str) -> Vec<Event> {
    let mut chunked_events: Vec<Event> = Vec::new();
    for event in events {
        if chunked_events.is_empty() && event.data.get(key).is_some() {
            // TODO: Add sub-chunks
            chunked_events.push(event.clone());
        } else {
            let val = match event.data.get(key) {
                None => continue,
                Some(v) => v,
            };
            let mut last_event = chunked_events.pop().unwrap();
            let last_val = last_event.data.get(key).unwrap().clone();
            if &last_val == val {
                // TODO: Add sub-chunks
                last_event.duration = last_event.duration + event.duration;
            }
            chunked_events.push(last_event);
            if &last_val != val {
                // TODO: Add sub-chunks
                chunked_events.push(event.clone());
            }
        }
    }
    chunked_events
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::DateTime;
    use chrono::Duration;
    use serde_json::json;

    use aw_models::Event;

    use super::chunk_events_by_key;

    #[test]
    fn test_chunk_events_by_key() {
        let e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let mut e2 = e1.clone();
        e2.data = json_map! {"test2": json!(1)};
        let e3 = e1.clone();
        let mut e4 = e1.clone();
        e4.data = json_map! {"test": json!(2)};

        let res = chunk_events_by_key(vec![e1, e2, e3, e4], "test");
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].duration, Duration::seconds(2));
        assert_eq!(res[1].duration, Duration::seconds(1));
    }
}
