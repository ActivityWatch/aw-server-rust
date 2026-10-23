use aw_datastore::{Datastore, DatastoreError};
use aw_models::Bucket;
use serde_json::json;

fn bucket(id: &str, hostname: &str, device: &str) -> Bucket {
    serde_json::from_value(json!({
        "id": id, "type": "test", "client": "test", "hostname": hostname,
        "device_id": device
    }))
    .unwrap()
}

#[test]
fn hostname_free_lookup_reuses_existing_history() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket(
        "window_host_with_underscores",
        "host_with_underscores",
        "local",
    ))
    .unwrap();
    assert_eq!(
        ds.get_bucket("window").unwrap().id,
        "window_host_with_underscores"
    );
    assert!(matches!(
        ds.create_bucket(&bucket("window", "host_with_underscores", "local")),
        Err(DatastoreError::BucketAlreadyExists(_))
    ));
    assert_eq!(ds.get_buckets().unwrap().len(), 1);
}

#[test]
fn suffixed_lookup_reuses_hostname_free_row() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window", "host_with_underscores", "local"))
        .unwrap();
    assert_eq!(
        ds.get_bucket("window_host_with_underscores").unwrap().id,
        "window"
    );
    assert!(matches!(
        ds.create_bucket(&bucket(
            "window_host_with_underscores",
            "host_with_underscores",
            "local"
        )),
        Err(DatastoreError::BucketAlreadyExists(_))
    ));
    assert!(matches!(
        ds.get_bucket("window_other_host"),
        Err(DatastoreError::NoSuchBucket(_))
    ));
}

#[test]
fn ambiguous_alias_does_not_choose_a_device() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window_alpha", "alpha", "device-a"))
        .unwrap();
    ds.create_bucket(&bucket("window_beta", "beta", "device-b"))
        .unwrap();
    // Neither unique-looking spelling is authority to select a device.
    assert!(ds.get_bucket("window").is_err());
    assert_eq!(ds.get_bucket("window_alpha").unwrap().device_id, "device-a");
    assert_eq!(ds.get_buckets().unwrap().len(), 2);
}

#[test]
fn aliases_share_event_history_privacy_and_heartbeat_state() {
    use aw_models::Event;
    use chrono::{Duration, Utc};
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window_host", "host", "local"))
        .unwrap();
    let first = Event {
        timestamp: Utc::now(),
        data: serde_json::from_value(json!({"app": "editor"})).unwrap(),
        ..Event::default()
    };
    let inserted = ds.heartbeat("window_host", first.clone(), 10.0).unwrap();
    let mut next = first;
    next.timestamp += Duration::seconds(1);
    assert_eq!(ds.heartbeat("window", next, 10.0).unwrap().id, inserted.id);
    assert_eq!(ds.get_event_count("window", None, None).unwrap(), 1);
    assert_eq!(ds.get_events("window", None, None, None).unwrap().len(), 1);
    assert_eq!(
        ds.get_event("window", inserted.id.unwrap()).unwrap().id,
        inserted.id
    );
    let (mut export, _) = ds
        .export_to_file(Some("window"), tempfile::tempfile().unwrap())
        .unwrap();
    use std::io::{Read, Seek, SeekFrom};
    export.seek(SeekFrom::Start(0)).unwrap();
    let mut json = String::new();
    export.read_to_string(&mut json).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value["buckets"]["window_host"]["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    ds.set_key_value(
        "settings.privacy_filters",
        &json!([{
            "enabled": true, "bucket_prefix": "window_host", "field": "app",
            "pattern": "secret", "action": "drop", "replacement": null
        }])
        .to_string(),
    )
    .unwrap();
    let secret: Event = serde_json::from_value(
        json!({"timestamp": "2026-01-01T00:00:00Z", "duration": 0, "data": {"app": "secret"}}),
    )
    .unwrap();
    assert!(ds.insert_events("window", &[secret]).unwrap().is_empty());
    ds.delete_events_by_id("window", vec![inserted.id.unwrap()])
        .unwrap();
    assert_eq!(ds.get_event_count("window_host", None, None).unwrap(), 0);
    ds.delete_bucket("window").unwrap();
    assert!(ds.get_buckets().unwrap().is_empty());
}

#[test]
fn creating_a_peer_exact_name_cannot_redirect_an_active_alias() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window_host", "host", "local"))
        .unwrap();
    assert_eq!(ds.get_bucket("window").unwrap().id, "window_host");
    assert!(ds
        .create_bucket(&bucket("window", "other", "peer"))
        .is_err());
    assert_eq!(ds.get_bucket("window").unwrap().id, "window_host");
    assert_eq!(ds.get_buckets().unwrap().len(), 1);
}

#[test]
fn renaming_a_peer_cannot_redirect_an_active_alias() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window_host", "host", "local"))
        .unwrap();
    ds.create_bucket(&bucket("unrelated", "other", "peer"))
        .unwrap();
    assert!(ds.rename_bucket("unrelated", "window").is_err());
    assert_eq!(ds.get_bucket("window").unwrap().id, "window_host");
}
