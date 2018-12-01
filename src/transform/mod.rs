use std::collections::HashMap;
use models::Event;

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
        println!("Merging heartbeats would result in a negative duration, refusing to merge!");
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

pub fn flood(mut events: Vec<Event>, pulsetime: chrono::Duration) -> Vec<Event> {
    let mut events_sorted = sort_by_timestamp (events);
    let mut e1_iter = events_sorted.drain(..).peekable();
    let mut new_events = Vec::new();
    let mut drop_next = false;
    while let Some(mut e1) = e1_iter.next() {
        if (drop_next) {
            drop_next = false;
            continue;
        }
        let e2 = match e1_iter.peek() {
            Some(e) => e,
            None => break
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
        'key: for key in keys.iter() {
            match event.data.get(key) {
                Some(v) => key_values.push(v.to_string()),
                None => continue 'event
            }
        }
        let summed_key = key_values.join(".");
        if merged_events_map.contains_key(&summed_key) {
            let merged_event = merged_events_map.get_mut(&summed_key).unwrap();
            merged_event.duration = merged_event.duration + event.duration;
        } else {
            // TODO: only copy values in select keys, not all keys in event
            /*
            let data = HashMap::new();
            for (k, v) in e1.data.iter() {
                data.insert(k, v);
            }
            */
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
