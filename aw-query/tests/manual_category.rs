use aw_datastore::Datastore;
use aw_models::{Bucket, Event, TimeInterval};
use aw_query::DataType;
use chrono::{Duration, Utc};
use serde_json::json;

#[test]
fn sidecar_precedence_preaggregation_and_clear_restore_rules() {
    let ds = Datastore::new_in_memory(false);
    let bucket: Bucket = serde_json::from_value(
        json!({"id":"test","type":"test","client":"test","hostname":"test"}),
    )
    .unwrap();
    ds.create_bucket(&bucket).unwrap();
    let start = "2026-01-01T10:00:00Z"
        .parse::<chrono::DateTime<Utc>>()
        .unwrap();
    let mut event = Event::default();
    event.timestamp = start;
    event.duration = Duration::seconds(10);
    event.data.insert("app".into(), json!("browser"));
    // A client cannot spoof a sidecar annotation by posting a reserved data key.
    event
        .data
        .insert("$manual_category".into(), json!(["Spoofed"]));
    let mut second = event.clone();
    second.timestamp += Duration::seconds(10);
    second.duration = Duration::seconds(20);
    let rows = ds.insert_events("test", &[event, second]).unwrap();
    ds.set_event_category("test", rows[0].id.unwrap(), vec!["Work".into()])
        .unwrap();
    let interval =
        TimeInterval::new_from_string("2026-01-01T10:00:00Z/2026-01-01T11:00:00Z").unwrap();
    let code = r#"events = merge_events_by_keys(sort_by_timestamp(query_bucket("test")), ["app"]); return categorize(events, [[["Automatic"], {"type":"regex","regex":"browser"}]]);"#;
    let result = aw_query::query(&code, &interval, &ds).unwrap();
    let events: Vec<Event> = result.try_into().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data["$category"], json!(["Work"]));
    assert_eq!(events[1].data["$category"], json!(["Automatic"]));
    assert_eq!(events[0].duration, Duration::seconds(10));
    assert_eq!(events[1].duration, Duration::seconds(20));
    ds.set_event_category("test", rows[0].id.unwrap(), vec!["Uncategorized".into()])
        .unwrap();
    let events: Vec<Event> = aw_query::query(&code, &interval, &ds)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(events[0].data["$category"], json!(["Uncategorized"]));
    ds.delete_event_category("test", rows[0].id.unwrap())
        .unwrap();
    let events: Vec<Event> = aw_query::query(&code, &interval, &ds)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data["$category"], json!(["Automatic"]));
    assert_eq!(events[0].duration, Duration::seconds(30));
    let sum = aw_query::query(
        "return sum_durations(query_bucket(\"test\"));",
        &interval,
        &ds,
    )
    .unwrap();
    assert!(matches!(sum, DataType::Number(n) if n == 30.0));
}
