use aw_datastore::{Datastore, DatastoreError};
use aw_models::{Bucket, BucketMetadata, Event};
use chrono::{Duration, Utc};
use serde_json::json;

fn bucket(ds: &Datastore, id: &str) {
    ds.create_bucket(&Bucket {
        bid: None,
        id: id.into(),
        _type: "test".into(),
        client: "test".into(),
        hostname: "test".into(),
        created: None,
        data: Default::default(),
        metadata: BucketMetadata::default(),
        events: None,
        last_updated: None,
    })
    .unwrap();
}

fn event() -> Event {
    Event {
        id: None,
        timestamp: Utc::now(),
        duration: Duration::seconds(1),
        data: serde_json::from_value(json!({"app": "editor", "$manual_category": ["spoof"]}))
            .unwrap(),
    }
}

#[test]
fn category_lifecycle_preserves_raw_events_and_heartbeat() {
    let ds = Datastore::new_in_memory(false);
    bucket(&ds, "a");
    bucket(&ds, "b");
    let original = ds.heartbeat("a", event(), 10.0).unwrap();
    let id = original.id.unwrap();
    assert_eq!(ds.get_event_category("a", id).unwrap(), None);
    let path = vec!["Work".to_string(), "Coding".to_string()];
    ds.set_event_category("a", id, path.clone()).unwrap();
    let mut pulse = original.clone();
    pulse.id = None;
    pulse.timestamp += Duration::seconds(2);
    let merged = ds.heartbeat("a", pulse, 10.0).unwrap();
    assert_eq!(merged.id, Some(id));
    assert_eq!(merged.data, original.data);
    assert_eq!(ds.get_event_category("a", id).unwrap(), Some(path.clone()));
    let replaced = ds.insert_events("a", &[merged.clone()]).unwrap();
    assert_eq!(replaced[0].id, Some(id));
    assert_eq!(ds.get_event_category("a", id).unwrap(), Some(path.clone()));
    assert_eq!(
        ds.get_events("a", None, None, None).unwrap()[0].data,
        original.data
    );
    let annotated = ds
        .get_events_with_categories("a", None, None, None)
        .unwrap();
    assert_eq!(annotated[0].data["$manual_category"], json!(path));
    for result in [
        ds.get_event_category("b", id).map(|_| ()),
        ds.set_event_category("b", id, vec!["Other".into()]),
        ds.delete_event_category("b", id),
    ] {
        assert!(matches!(result, Err(DatastoreError::NoSuchEvent(_, _))));
    }
    for invalid in [vec![], vec!["".into()], vec!["Work".into(), " \t".into()]] {
        assert!(matches!(
            ds.set_event_category("a", id, invalid),
            Err(DatastoreError::InvalidCategory(_))
        ));
    }
    assert_eq!(ds.get_event_category("a", id).unwrap(), Some(path));
    ds.delete_event_category("a", id).unwrap();
    ds.delete_event_category("a", id).unwrap();
    assert_eq!(ds.get_event_category("a", id).unwrap(), None);
    assert!(!ds
        .get_events_with_categories("a", None, None, None)
        .unwrap()[0]
        .data
        .contains_key("$manual_category"));
    ds.close();
}

#[test]
fn category_writes_are_durable_and_deletion_cleans_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let filename = dir.path().join("categories.db");
    let ds = Datastore::new(filename.to_str().unwrap().into(), false);
    bucket(&ds, "a");
    bucket(&ds, "b");
    let first = ds.insert_events("a", &[event()]).unwrap()[0].clone();
    let second = ds.insert_events("b", &[event()]).unwrap()[0].clone();
    let id = first.id.unwrap();
    ds.set_event_category("a", id, vec!["Work".into()]).unwrap();
    // No force_commit: the successful annotation response is a durability promise.
    let conn = rusqlite::Connection::open(&filename).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT category_path FROM event_category_overrides WHERE event_id = ?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        r#"["Work"]"#
    );
    // Updating an older ID must not return the most recently inserted rowid.
    assert_eq!(ds.insert_events("a", &[first]).unwrap()[0].id, Some(id));
    assert_eq!(
        ds.get_event_category("a", id).unwrap(),
        Some(vec!["Work".into()])
    );
    ds.close();
    let ds = Datastore::new(filename.to_str().unwrap().into(), false);
    assert_eq!(
        ds.get_event_category("a", id).unwrap(),
        Some(vec!["Work".into()])
    );
    ds.delete_event_category("a", id).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM event_category_overrides", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    ds.set_event_category("a", id, vec!["Work".into()]).unwrap();
    ds.delete_events_by_id("a", vec![id]).unwrap();
    ds.set_event_category("b", second.id.unwrap(), vec!["Other".into()])
        .unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM event_category_overrides", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
    ds.delete_bucket("b").unwrap();
    ds.force_commit().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM event_category_overrides", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    ds.close();
}

#[test]
fn old_readonly_databases_keep_raw_read_compatibility() {
    for version in [4, 5, 6] {
        let dir = tempfile::tempdir().unwrap();
        let filename = dir.path().join("old.db");
        let ds = Datastore::new(filename.to_str().unwrap().into(), false);
        bucket(&ds, "a");
        let original = ds.insert_events("a", &[event()]).unwrap()[0].clone();
        ds.close();
        {
            let conn = rusqlite::Connection::open(&filename).unwrap();
            conn.execute_batch("DROP TRIGGER events_delete_category_override; DROP TABLE event_category_overrides;").unwrap();
            conn.pragma_update(None, "user_version", version).unwrap();
        }
        let ds = Datastore::open_read_only(filename.to_str().unwrap().into()).unwrap();
        assert_eq!(
            ds.get_event("a", original.id.unwrap()).unwrap().data,
            original.data
        );
        assert_eq!(
            ds.get_event_category("a", original.id.unwrap()).unwrap(),
            None
        );
        assert!(!ds
            .get_events_with_categories("a", None, None, None)
            .unwrap()[0]
            .data
            .contains_key("$manual_category"));
        ds.close();
    }
}

#[test]
fn colliding_event_id_cannot_move_an_annotation_to_another_bucket() {
    let ds = Datastore::new_in_memory(false);
    bucket(&ds, "a");
    bucket(&ds, "b");
    let original = ds.insert_events("a", &[event()]).unwrap()[0].clone();
    let id = original.id.unwrap();
    ds.set_event_category("a", id, vec!["Work".into()]).unwrap();

    let mut collision = original.clone();
    collision.data.insert("app".into(), json!("other"));
    assert!(matches!(
        ds.insert_events("b", &[collision]),
        Err(DatastoreError::NoSuchEvent(ref bucket, event_id))
            if bucket == "b" && event_id == id
    ));
    assert_eq!(ds.get_event("a", id).unwrap().data, original.data);
    assert_eq!(
        ds.get_event_category("a", id).unwrap(),
        Some(vec!["Work".into()])
    );
    assert_eq!(ds.get_event_count("b", None, None).unwrap(), 0);
    ds.close();
}
