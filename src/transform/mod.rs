use std::collections::HashMap;
use models::Event;
use serde_json::value::Value;

// TODO: Compare with aw-cores version to make sure it works correctly
pub fn heartbeat(last_event: &Event, heartbeat: &Event, pulsetime: f64) -> Option<Event> {
    // Verify that data is the same
    if heartbeat.data != last_event.data {
        return None
    }

    let last_event_endtime = last_event.calculate_endtime();
    let heartbeat_endtime = heartbeat.calculate_endtime();

    // Verify that timestamps intersect (including pulsetime)
    let pulsetime_ns : i64 = (pulsetime*1000000000.0).round() as i64;
    let last_endtime_allowed = last_event_endtime + chrono::Duration::nanoseconds(pulsetime_ns);
    if last_event.timestamp > heartbeat.timestamp {
        return None
    }
    if heartbeat.timestamp > last_endtime_allowed {
        return None
    }

    let mut starttime = &last_event.timestamp;
    if heartbeat.timestamp < last_event.timestamp {
        starttime = &heartbeat.timestamp;
    }

    let mut endtime = &heartbeat_endtime;
    if last_event_endtime > heartbeat_endtime {
        endtime = &last_event_endtime;
    }

    let duration = endtime.signed_duration_since(*starttime);
    if duration.num_nanoseconds().unwrap() < 0 {
        debug!("Merging heartbeats would result in a negative duration, refusing to merge!");
        return None
    }

    // Success, return successful heartbeat last_event
    return Some(Event {
        id: None,
        timestamp: starttime.clone(),
        duration: duration,
        data: last_event.data.clone()
    })
}

pub fn sort_by_timestamp(mut events: Vec<Event>) -> Vec<Event> {
    events.sort_by(|e1, e2| e1.timestamp.cmp(&e2.timestamp));
    return events;
}

/* Highest first */
pub fn sort_by_duration(mut events: Vec<Event>) -> Vec<Event> {
    events.sort_by(|e1, e2| e2.duration.cmp(&e1.duration));
    return events;
}

pub fn flood(events: Vec<Event>, pulsetime: chrono::Duration) -> Vec<Event> {
    let mut events_sorted = sort_by_timestamp (events);
    let mut e1_iter = events_sorted.drain(..).peekable();
    let mut new_events = Vec::new();
    let mut drop_next = false;
    while let Some(mut e1) = e1_iter.next() {
        if drop_next {
            drop_next = false;
            continue;
        }
        let e2 = match e1_iter.peek() {
            Some(e) => e,
            None => {
                new_events.push(e1.clone());
                break;
            }
        };

        let gap = e2.timestamp - e1.timestamp;

        if gap < pulsetime {
            if e1.data == e2.data {
                let e2_end = e2.timestamp + e2.duration;
                // Extend e1 to the end of e2
                e1.duration = e2_end - e1.timestamp;
                // Drop next event since they are merged and flooded into e1
                drop_next = true;
            } else {
                // Extend e1 to the start of e2
                e1.duration = e2.timestamp - e1.timestamp;
            }
        }
        new_events.push(e1.clone());
    }
    return new_events;
}

pub fn merge_events_by_keys(events: Vec<Event>, keys: Vec<String>) -> Vec<Event> {
    if keys.len() == 0 {
        return vec![];
    }
    let mut merged_events_map : HashMap<String, Event> = HashMap::new();
    'event: for event in events {
        let mut key_values = Vec::new();
        'key: for key in &keys {
            match event.data.get(&key) {
                Some(v) => key_values.push(v.to_string()),
                None => continue 'event
            }
        }
        let summed_key = key_values.join(".");
        if merged_events_map.contains_key(&summed_key) {
            let merged_event = merged_events_map.get_mut(&summed_key).unwrap();
            merged_event.duration = merged_event.duration + event.duration;
        } else {
            let mut data = HashMap::new();
            for key in &keys {
                data.insert(key.clone(), event.data.get(key).unwrap());
            }
            let merged_event = Event {
                id: None,
                timestamp: event.timestamp.clone(),
                duration: event.duration.clone(),
                data: event.data.clone(),
            };
            merged_events_map.insert(summed_key, merged_event);
        }
    }
    let mut merged_events_list = Vec::new();
    for (_key, event) in merged_events_map.drain() {
        merged_events_list.push(event);
    }
    return merged_events_list;
}

pub fn chunk_events_by_key(events: Vec<Event>, key: &str) -> Vec<Event> {
    let mut chunked_events : Vec<Event> = Vec::new();
    for event in events {
        if chunked_events.len() == 0 && event.data.get(key).is_some() {
            // TODO: Add sub-chunks
            chunked_events.push(event.clone());
        } else {
            let val = match event.data.get(key) {
                None => continue,
                Some(v) => v
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
    return chunked_events;
}

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

pub fn filter_period_intersect(events: &Vec<Event>, filter_events: &Vec<Event>) -> Vec<Event> {
    let mut filtered_events = Vec::new();
    for filter in filter_events {
        let filter_endtime = filter.calculate_endtime();
        for event in events {
            if event.timestamp > filter_endtime {
                continue
            }
            let event_endtime = event.calculate_endtime();
            if event_endtime < filter.timestamp {
                continue
            }
            let mut e = event.clone();
            e.timestamp = std::cmp::max(e.timestamp, filter.timestamp);
            let endtime = std::cmp::min(event_endtime, filter_endtime);
            e.duration = endtime - e.timestamp;
            filtered_events.push(e);
        }
    }
    return filtered_events;
}

pub fn split_url_event(event: &mut Event) {
    use rocket::http::uri::Absolute;
    use serde_json::Value;
    let uri_str = match event.data.get("url") {
        None => return,
        Some(val) => match val {
            Value::String(s) => s.clone(),
            _ => return
        }
    };
    let uri = match Absolute::parse(&uri_str) {
        Ok(uri) => uri,
        Err(_) => return
    };
    let data = match event.data {
        Value::Object(ref mut o) => o,
        _ => panic!("event data is not a object!")
    };
    // Protocol
    let protocol = uri.scheme().to_string();
    data.insert("protocol".to_string(), Value::String(protocol));
    // Domain
    let domain = match uri.authority() {
        Some(authority) => {
            authority.host().trim_start_matches("www.").to_string()
        },
        None => "".to_string(),
    };
    data.insert("domain".to_string(), Value::String(domain));
    // Path
    let path = match uri.origin() {
        Some(origin) => origin.path().to_string(),
        None => "".to_string()
    };
    data.insert("path".to_string(), Value::String(path));
    // Params
    // TODO: What's the difference between params and query?
    let params = match uri.origin() {
        Some(origin) => match origin.query() {
            Some(query) => query.to_string(),
            None => "".to_string()
        },
        None => "".to_string()
    };
    data.insert("params".to_string(), Value::String(params));

    // TODO: aw-server-python also has options and identifier
}
