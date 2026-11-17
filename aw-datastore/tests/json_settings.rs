use aw_datastore::Datastore;
use serde_json::{json, Value};
use std::sync::mpsc;

fn update_enabled(ds: &Datastore, ready: mpsc::Sender<()>, resume: mpsc::Receiver<()>) {
    // Capture the old value as the previous tray implementation did. The
    // replacement operation must not use this stale snapshot.
    let _old = ds.get_key_value("settings.aw-notify").unwrap();
    ready.send(()).unwrap();
    resume.recv().unwrap();
    ds.set_json_field_and_commit("settings.aw-notify", "enabled", json!(true))
        .unwrap();
}

#[test]
fn json_field_update_preserves_interleaved_settings_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.db");
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    ds.set_key_value_and_commit("settings.aw-notify", r#"{"enabled":false,"interval":60}"#)
        .unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let tray = ds.clone();
    let toggle = std::thread::spawn(move || update_enabled(&tray, ready_tx, resume_rx));
    ready_rx.recv().unwrap();
    // Same shared-worker call used by the settings HTTP endpoint. This is
    // deliberately uncommitted: the atomic operation must read the current
    // transaction, not a second connection or an earlier caller snapshot.
    ds.set_key_value(
        "settings.aw-notify",
        r#"{"enabled":false,"interval":120,"sound":"quiet"}"#,
    )
    .unwrap();
    resume_tx.send(()).unwrap();
    toggle.join().unwrap();

    // Independent SQLite connection: success must mean committed visibility.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let stored: String = conn
        .query_row(
            "SELECT value FROM key_value WHERE key = 'settings.aw-notify'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    ds.close();
    drop(conn);
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap(),
        json!({"enabled":true,"interval":120,"sound":"quiet"})
    );
}

#[test]
fn json_field_update_normalizes_missing_malformed_and_non_object_settings() {
    let ds = Datastore::new_in_memory(false);
    for original in [
        None,
        Some("not json"),
        Some("null"),
        Some("[]"),
        Some("true"),
        Some("42"),
        Some(r#""text""#),
    ] {
        if let Some(value) = original {
            ds.set_key_value("settings.aw-notify", value).unwrap();
        }
        ds.set_json_field_and_commit("settings.aw-notify", "enabled", json!(true))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&ds.get_key_value("settings.aw-notify").unwrap())
                .unwrap(),
            json!({"enabled":true}),
            "original: {original:?}"
        );
    }
    ds.close();
}

#[test]
fn json_field_update_replaces_only_the_literal_top_level_field() {
    let ds = Datastore::new_in_memory(false);
    ds.set_key_value(
        "settings.test",
        r#"{"a.b":false,"a":{"b":7},"other":[1,2]}"#,
    )
    .unwrap();
    for value in [
        json!(null),
        json!({"nested":"value"}),
        json!([1, 2]),
        json!(false),
    ] {
        ds.set_json_field_and_commit("settings.test", "a.b", value.clone())
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&ds.get_key_value("settings.test").unwrap()).unwrap(),
            json!({"a.b":value,"a":{"b":7},"other":[1,2]})
        );
    }
    ds.close();
}

#[test]
fn json_field_update_survives_reopen_and_can_disable_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.db").to_str().unwrap().to_owned();
    let ds = Datastore::new(path.clone(), false);
    ds.set_key_value("settings.aw-notify", r#"{"enabled":true,"interval":120}"#)
        .unwrap();
    ds.set_json_field_and_commit("settings.aw-notify", "enabled", json!(false))
        .unwrap();
    ds.close();
    let reopened = Datastore::new(path, false);
    assert_eq!(
        serde_json::from_str::<Value>(&reopened.get_key_value("settings.aw-notify").unwrap())
            .unwrap(),
        json!({"enabled":false,"interval":120})
    );
    reopened.close();
}

#[test]
fn json_field_update_propagates_database_read_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.db");
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    ds.force_commit().unwrap();
    ds.close();
    let conn = rusqlite::Connection::open(&path).unwrap();
    // SQL NULL is not a missing key or malformed JSON: get_key_value cannot
    // decode it into a String, and the update must not replace it silently.
    conn.execute(
        "INSERT INTO key_value (key, value, last_modified) VALUES (?1, NULL, 0)",
        ["settings.broken"],
    )
    .unwrap();
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    assert!(ds
        .set_json_field_and_commit("settings.broken", "enabled", json!(true))
        .is_err());
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM key_value WHERE key = 'settings.broken'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, None);
    ds.close();
}

#[test]
fn json_field_update_propagates_database_write_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.db");
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    ds.set_key_value_and_commit("settings.aw-notify", r#"{"enabled":false}"#)
        .unwrap();
    ds.close();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_settings BEFORE INSERT ON key_value BEGIN SELECT RAISE(ABORT, 'injected settings failure'); END;").unwrap();
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    assert!(ds
        .set_json_field_and_commit("settings.aw-notify", "enabled", json!(true))
        .is_err());
    assert_eq!(
        ds.get_key_value("settings.aw-notify").unwrap(),
        r#"{"enabled":false}"#
    );
    ds.close();
}

#[test]
fn json_field_update_propagates_commit_failure_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.db");
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    ds.set_key_value_and_commit("settings.aw-notify", r#"{"enabled":false}"#)
        .unwrap();
    ds.close();
    let conn = rusqlite::Connection::open(&path).unwrap();
    // The INSERT succeeds, but its deferred foreign key fails at COMMIT.
    conn.execute_batch(
        "CREATE TABLE parent (id INTEGER PRIMARY KEY);
         CREATE TABLE child (parent_id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);
         CREATE TRIGGER reject_commit AFTER INSERT ON key_value
         BEGIN INSERT INTO child VALUES (1); END;",
    )
    .unwrap();
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    let error = ds
        .set_json_field_and_commit("settings.aw-notify", "enabled", json!(true))
        .unwrap_err();
    assert!(
        matches!(error, aw_datastore::DatastoreError::InternalError(ref message) if message.contains("Failed to commit")),
        "{error:?}"
    );
    assert_eq!(
        ds.get_key_value("settings.aw-notify").unwrap(),
        r#"{"enabled":false}"#
    );
    let stored: String = conn
        .query_row(
            "SELECT value FROM key_value WHERE key = 'settings.aw-notify'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, r#"{"enabled":false}"#);
    ds.close();
}
