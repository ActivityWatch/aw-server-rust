use super::models::event::Event;

pub fn heartbeat(event: &Event, heartbeat: &Event, pulsetime: f64) -> Option<Event> {
    // Verify that data is the same
    if heartbeat.data != event.data {
        return None
    }

    // Verify that timestamps intersect with pulsetime
    let pulsetime_ns : i64 = (pulsetime*1000000000.0).round() as i64;
    let last_timestamp_allowed = event.timestamp + chrono::Duration::nanoseconds(pulsetime_ns);
    let heartbeat_duration_ns : i64 = (heartbeat.duration.num_nanos()) as i64;
    let heartbeat_end_timestamp = heartbeat.timestamp + chrono::Duration::nanoseconds(heartbeat_duration_ns);
    if heartbeat_end_timestamp > last_timestamp_allowed {
        return None
    }

    // Success, return successful heartbeat event
    return Some(Event {
        id: None,
        timestamp: event.timestamp.clone(),
        duration: event.duration.clone(),
        data: event.data.clone()
    })
}
