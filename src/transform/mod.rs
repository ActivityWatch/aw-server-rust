use chrono::Duration;
use super::models::event::Event;

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
    if heartbeat.timestamp > last_endtime_allowed || heartbeat_endtime < last_event.timestamp {
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
