use super::models::event::Event;
use super::models::duration::Duration;

// TODO: Compare with aw-cores version to make sure it works correctly
pub fn heartbeat(event: &Event, heartbeat: &Event, pulsetime: f64) -> Option<Event> {
    // Verify that data is the same
    if heartbeat.data != event.data {
        return None
    }

    let event_endtime = event.calculate_endtime();
    let heartbeat_endtime = heartbeat.calculate_endtime();

    // Verify that timestamps intersect (including pulsetime)
    let pulsetime_ns : i64 = (pulsetime*1000000000.0).round() as i64;
    let last_endtime_allowed = event_endtime + chrono::Duration::nanoseconds(pulsetime_ns);
    if heartbeat.timestamp > last_endtime_allowed || heartbeat_endtime < event.timestamp {
        return None
    }

    let mut starttime = &event.timestamp;
    if heartbeat.timestamp < event.timestamp {
        starttime = &heartbeat.timestamp;
    }

    let mut endtime = &heartbeat_endtime;
    if event_endtime > heartbeat_endtime {
        endtime = &event_endtime;
    }

    let duration = Duration::from_nanos(endtime.signed_duration_since(*starttime).num_nanoseconds().unwrap() as u64);
    println!("{:?}", duration);

    // Success, return successful heartbeat event
    return Some(Event {
        id: None,
        timestamp: starttime.clone(),
        duration: duration,
        data: event.data.clone()
    })
}
