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

#[test]
fn local_create_cannot_acknowledge_an_alias_that_global_writes_redirect() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window", "other", "peer"))
        .unwrap();
    ds.create_bucket(&bucket("window_host", "host", "local"))
        .unwrap();
    assert!(matches!(
        ds.create_bucket(&bucket("window", "host", "local")),
        Err(DatastoreError::AmbiguousBucket(_, _))
    ));
}

#[test]
fn reader_refreshes_aliases_after_hostname_repair() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("aliases.sqlite")
        .to_str()
        .unwrap()
        .to_owned();
    let writer = Datastore::new(path.clone(), false);
    writer
        .create_bucket(&bucket("window", "unknown", "local"))
        .unwrap();
    let reader = Datastore::open_reader(path).unwrap();
    assert!(reader.get_bucket("window_host").is_err());
    assert_eq!(writer.migrate_hostname("host").unwrap(), 1);
    assert_eq!(reader.get_bucket("window_host").unwrap().id, "window");
}

#[test]
fn scoped_hostname_repair_preserves_peer_metadata_and_reader_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("hostname-scope.sqlite")
        .to_str()
        .unwrap()
        .to_owned();
    let writer = Datastore::new(path.clone(), false);
    for (device, id) in [("local", "window"), ("peer", "peer-window")] {
        writer
            .create_bucket(&bucket(id, "unknown", device))
            .unwrap();
    }
    writer.force_commit().unwrap();
    let reader = Datastore::open_reader(path).unwrap();
    assert!(reader
        .for_device("local")
        .get_bucket("window_host")
        .is_err());
    assert_eq!(
        writer.for_device("local").migrate_hostname("host").unwrap(),
        1
    );
    assert_eq!(
        reader
            .for_device("local")
            .get_bucket("window_host")
            .unwrap()
            .id,
        "window"
    );
    assert_eq!(
        reader
            .for_device("peer")
            .get_bucket("peer-window")
            .unwrap()
            .hostname,
        "unknown"
    );
    assert!(reader
        .for_device("peer")
        .get_bucket("peer-window_host")
        .is_err());
    // The unscoped startup migration still repairs every remaining unknown row.
    assert_eq!(writer.migrate_hostname("peer-host").unwrap(), 1);
    assert_eq!(
        reader
            .for_device("local")
            .get_bucket("window")
            .unwrap()
            .hostname,
        "host"
    );
    assert_eq!(
        reader
            .for_device("peer")
            .get_bucket("peer-window_peer-host")
            .unwrap()
            .id,
        "peer-window"
    );
}

#[test]
fn device_lookup_is_exact_and_never_falls_back_to_another_device() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window_host", "host", "local"))
        .unwrap();
    ds.create_bucket(&bucket("window_other", "other", "peer"))
        .unwrap();
    assert_eq!(
        ds.get_bucket_for_device("local", "window").unwrap().id,
        "window_host"
    );
    assert_eq!(
        ds.get_bucket_for_device("peer", "window").unwrap().id,
        "window_other"
    );
    assert!(matches!(
        ds.get_bucket_for_device("missing", "window_host"),
        Err(DatastoreError::NoSuchBucket(_))
    ));
    assert!(matches!(
        ds.get_bucket_for_device("local", "window_other"),
        Err(DatastoreError::NoSuchBucket(_))
    ));
}

#[test]
fn scoped_handle_routes_events_and_export_without_peer_fallback() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window_host", "host", "local"))
        .unwrap();
    ds.create_bucket(&bucket("window_other", "other", "peer"))
        .unwrap();
    let local = ds.for_device("local");
    let peer = ds.for_device("peer");
    assert_eq!(local.get_bucket("window").unwrap().id, "window_host");
    assert_eq!(peer.get_bucket("window").unwrap().id, "window_other");
    assert!(matches!(
        peer.get_bucket("window_host"),
        Err(DatastoreError::NoSuchBucket(_))
    ));
    let event = aw_models::Event::default();
    local.insert_events("window", &[event]).unwrap();
    assert_eq!(local.get_event_count("window", None, None).unwrap(), 1);
    assert_eq!(peer.get_event_count("window", None, None).unwrap(), 0);
    assert_eq!(peer.get_buckets().unwrap().len(), 1);
    use std::io::{Read, Seek, SeekFrom};
    let (mut file, _) = local
        .export_to_file(None, tempfile::tempfile().unwrap())
        .unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    let export: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(export["buckets"].as_object().unwrap().len(), 1);
    assert!(export["buckets"]["window_host"].is_object());
    peer.delete_bucket("window").unwrap();
    assert_eq!(local.get_event_count("window", None, None).unwrap(), 1);
    assert_eq!(ds.get_buckets().unwrap().len(), 1);
}

#[test]
fn scoped_creation_uses_selected_ownership_and_restores_unscoped_handle() {
    let ds = Datastore::new_in_memory(false);
    let peer = ds.for_device("peer");
    peer.create_bucket(&bucket("peer_bucket", "host", "forged"))
        .unwrap();
    assert_eq!(peer.get_bucket("peer_bucket").unwrap().device_id, "peer");
    ds.create_bucket(&bucket("local_bucket", "host", "local"))
        .unwrap();
    assert!(matches!(
        peer.get_bucket("local_bucket"),
        Err(DatastoreError::NoSuchBucket(_))
    ));
    assert_eq!(ds.get_buckets().unwrap().len(), 2);
    assert_eq!(ds.get_bucket("local_bucket").unwrap().device_id, "local");
    assert_eq!(peer.get_buckets().unwrap().len(), 1);
}

#[test]
fn empty_device_scope_cannot_write_into_local_ownership() {
    let ds = Datastore::new_in_memory(false);
    assert!(ds
        .for_device("")
        .create_bucket(&bucket("unexpected", "host", "local"))
        .is_err());
    assert!(ds.get_buckets().unwrap().is_empty());
}

#[test]
fn legacy_name_migration_refreshes_peer_destinations_before_mutation() {
    use aw_datastore::DatastoreInstance;
    use rusqlite::Connection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("migration-refresh.sqlite");
    let conn = Connection::open(&path).unwrap();
    let mut ds = DatastoreInstance::new(&conn, true).unwrap();
    let old = "aw-watcher-android-test_phone";
    let new = "aw-watcher-android_phone";
    ds.create_bucket(&conn, bucket(old, "phone", "local"))
        .unwrap();
    ds.insert_events(&conn, old, vec![aw_models::Event::default()])
        .unwrap();
    let other_conn = Connection::open(&path).unwrap();
    let mut peer = DatastoreInstance::new(&other_conn, true).unwrap();
    peer.create_bucket(&other_conn, bucket(new, "peer", "peer"))
        .unwrap();
    assert!(matches!(
        ds.migrate_test_bucket_names(&conn),
        Err(DatastoreError::BucketAlreadyExists(_))
    ));
    let names: Vec<(String, String)> = conn
        .prepare("SELECT device_id, name FROM buckets ORDER BY device_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        names,
        vec![("local".into(), old.into()), ("peer".into(), new.into())]
    );
    assert_eq!(ds.get_event_count(&conn, old, None, None).unwrap(), 1);
    DatastoreInstance::new(&conn, false).unwrap();
}

#[test]
fn failed_scoped_request_restores_unscoped_resolution() {
    let ds = Datastore::new_in_memory(false);
    ds.create_bucket(&bucket("window", "host", "local"))
        .unwrap();
    assert!(ds.for_device("peer").get_bucket("window").is_err());
    assert_eq!(ds.get_bucket("window").unwrap().device_id, "local");
}

#[test]
fn scoped_alias_privacy_matches_stored_names_without_filtering_other_devices() {
    use aw_models::Event;
    let ds = Datastore::new_in_memory(false);
    ds.for_device("local")
        .create_bucket(&bucket("window_private_host", "private_host", "local"))
        .unwrap();
    ds.for_device("peer")
        .create_bucket(&bucket("window_public_host", "public_host", "peer"))
        .unwrap();
    ds.set_key_value_and_commit(
        "settings.privacy_filters",
        &json!([{
            "enabled": true, "bucket_prefix": "window_private_host", "field": "title",
            "pattern": "secret", "action": "redact", "replacement": "hidden"
        }])
        .to_string(),
    )
    .unwrap();
    let event: Event = serde_json::from_value(
        json!({"timestamp":"2026-01-01T00:00:00Z", "duration": 1, "data":{"title":"secret"}}),
    )
    .unwrap();
    for (device, expected) in [("local", "hidden"), ("peer", "secret")] {
        let scoped = ds.for_device(device);
        let inserted = scoped
            .insert_events("window", std::slice::from_ref(&event))
            .unwrap();
        assert_eq!(inserted[0].data["title"], expected);
        let stored = scoped.get_events("window", None, None, None).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].data["title"], expected);
    }
}

#[test]
fn open_reader_refreshes_device_ownership_after_metadata_repair() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("ownership.sqlite")
        .to_str()
        .unwrap()
        .to_owned();
    let writer = Datastore::new(path.clone(), false);
    writer
        .create_bucket(&bucket("window", "host", "local"))
        .unwrap();
    writer.force_commit().unwrap();
    writer.close();
    let reader = Datastore::open_reader(path.clone()).unwrap();
    assert_eq!(
        reader
            .for_device("local")
            .get_bucket("window")
            .unwrap()
            .device_id,
        "local"
    );
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE buckets SET device_id = 'peer' WHERE name = 'window'",
        [],
    )
    .unwrap();
    assert!(reader.for_device("local").get_bucket("window").is_err());
    assert_eq!(
        reader
            .for_device("peer")
            .get_bucket("window")
            .unwrap()
            .device_id,
        "peer"
    );
}

#[test]
fn reader_rejects_stale_identity_when_catalogue_reload_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("reload-failure.sqlite")
        .to_str()
        .unwrap()
        .to_owned();
    let writer = Datastore::new(path.clone(), false);
    writer
        .create_bucket(&bucket("window", "host", "local"))
        .unwrap();
    writer.force_commit().unwrap();
    writer.close();
    let reader = Datastore::open_reader(path.clone()).unwrap();
    assert_eq!(reader.get_bucket("window").unwrap().device_id, "local");
    let conn = rusqlite::Connection::open(&path).unwrap();
    // Simulate a catalogue state rejected by the still-required cache guard.
    conn.execute("INSERT INTO buckets (name, device_id, type, client, hostname, created, data) VALUES ('window', 'peer', 'test', 'test', 'host', '2026-01-01T00:00:00Z', '{}')", []).unwrap();
    assert!(reader.get_bucket("window").is_err());
    assert!(reader.for_device("local").get_bucket("window").is_err());
    assert!(reader.get_buckets().is_err());
    conn.execute("DELETE FROM buckets WHERE device_id = 'peer'", [])
        .unwrap();
    assert_eq!(reader.get_bucket("window").unwrap().device_id, "local");
}
