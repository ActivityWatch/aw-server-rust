use aw_datastore::{Datastore, DatastoreError};
use rusqlite::Connection;

#[test]
fn null_settings_value_returns_error_without_killing_worker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.db");
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    ds.set_key_value_and_commit("settings.broken", "1").unwrap();
    ds.set_key_value_and_commit("settings.valid", "2").unwrap();
    ds.close();
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE key_value SET value = NULL WHERE key = 'settings.broken'",
        [],
    )
    .unwrap();
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);

    let error = ds.get_key_values("settings.%").unwrap_err();
    assert!(
        matches!(error, DatastoreError::InternalError(_)),
        "{error:?}"
    );
    assert!(
        matches!(&error, DatastoreError::InternalError(message) if message.contains("settings.broken")),
        "the error must identify the key to repair: {error:?}"
    );
    assert!(ds.get_buckets().is_ok(), "a bad setting killed the worker");
    assert_eq!(ds.get_key_value("settings.valid").unwrap(), "2");
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM key_value WHERE key = 'settings.broken'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, None, "reading must not rewrite the corrupt row");

    ds.delete_key_value("settings.broken").unwrap();
    let settings = ds.get_key_values("settings.%").unwrap();
    assert_eq!(settings.len(), 1);
    assert_eq!(settings["settings.valid"], "2");
    ds.close();
}

#[test]
fn failed_settings_delete_returns_error_without_killing_worker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.db");
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);
    ds.set_key_value_and_commit("settings.keep", "1").unwrap();
    ds.close();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_delete BEFORE DELETE ON key_value
         BEGIN SELECT RAISE(ABORT, 'injected delete failure'); END;",
    )
    .unwrap();
    let ds = Datastore::new(path.to_str().unwrap().to_owned(), false);

    let error = ds.delete_key_value("settings.keep").unwrap_err();
    assert!(
        matches!(error, DatastoreError::InternalError(_)),
        "{error:?}"
    );
    assert!(
        ds.get_buckets().is_ok(),
        "a failed delete killed the worker"
    );
    assert_eq!(ds.get_key_value("settings.keep").unwrap(), "1");
    ds.set_key_value_and_commit("settings.other", "2").unwrap();
    assert_eq!(ds.get_key_value("settings.other").unwrap(), "2");
    ds.close();
}
