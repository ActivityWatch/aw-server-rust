use aw_models::Event;

pub fn heartbeat(last_event: &Event, heartbeat: &Event, pulsetime: f64) -> Option<Event> {
    // Verify that data is the same
    if heartbeat.data != last_event.data {
        return None;
    }

    let last_event_endtime = last_event.calculate_endtime();
    let heartbeat_endtime = heartbeat.calculate_endtime();

    // Verify that timestamps intersect (including pulsetime)
    let pulsetime_ns: i64 = (pulsetime * 1_000_000_000.0).round() as i64;
    let last_endtime_allowed = last_event_endtime + chrono::Duration::nanoseconds(pulsetime_ns);
    if last_event.timestamp > heartbeat.timestamp {
        return None;
    }
    if heartbeat.timestamp > last_endtime_allowed {
        return None;
    }

    let starttime = if heartbeat.timestamp < last_event.timestamp {
        &heartbeat.timestamp
    } else {
        &last_event.timestamp
    };

    let endtime = if last_event_endtime > heartbeat_endtime {
        &last_event_endtime
    } else {
        &heartbeat_endtime
    };

    let duration = endtime.signed_duration_since(*starttime);
    if duration.num_nanoseconds().unwrap() < 0 {
        debug!("Merging heartbeats would result in a negative duration, refusing to merge!");
        return None;
    }

    // Success, return successful heartbeat last_event
    Some(Event {
        id: None,
        timestamp: *starttime,
        duration,
        data: last_event.data.clone(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::Utc;
    use serde_json::json;

    use aw_models::Event;

    use super::heartbeat;

    #[test]
    fn test_heartbeat_pulsetime() {
        let now = Utc::now();
        let event1 = Event {
            id: None,
            timestamp: now,
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };
        let heartbeat1 = Event {
            id: None,
            timestamp: now + Duration::seconds(2),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };

        // Merge result
        let res_merge = heartbeat(&event1, &heartbeat1, 2.0).unwrap();
        assert_eq!(res_merge.timestamp, now);
        assert_eq!(res_merge.duration, Duration::seconds(3));
        assert_eq!(res_merge.data, event1.data);

        // No merge result
        let res_no_merge = heartbeat(&event1, &heartbeat1, 0.0);
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
            data: json_map! {"test": json!(1)},
        };
        let heartbeat_same_data = Event {
            id: None,
            timestamp: now.clone(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(1)},
        };

        // Data is same, should merge
        let res_merge = heartbeat(&event, &heartbeat_same_data, 1.0);
        assert!(res_merge.is_some());

        let heartbeat_different_data = Event {
            id: None,
            timestamp: now.clone(),
            duration: Duration::seconds(1),
            data: json_map! {"test": json!(2)},
        };
        // Data is different, should not merge
        let res_merge = heartbeat(&event, &heartbeat_different_data, 1.0);
        assert!(res_merge.is_none());
    }
}
