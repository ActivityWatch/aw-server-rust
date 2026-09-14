/// Owner-originated historical edits (ActivityWatch/aw-android#253).
///
/// WebUI/Android title edits are delete+insert at the same timestamp. aw-sync
/// resumes at the destination's latest event end, so those replacements sit
/// outside the fetch window unless a reconcile pass updates matching timestamps.
use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use aw_datastore::Datastore;
use aw_models::{Bucket, Event};
use aw_sync::SyncSpec;

fn memory_pair() -> (Datastore, Datastore) {
    (
        Datastore::new_in_memory(false),
        Datastore::new_in_memory(false),
    )
}

fn create_bucket(ds: &Datastore, id: &str, hostname: &str) -> String {
    let bucket_json = format!(
        r#"{{
            "id": "{id}",
            "type": "currentwindow",
            "hostname": "{hostname}",
            "client": "test"
        }}"#
    );
    let bucket: Bucket = serde_json::from_str(&bucket_json).unwrap();
    ds.create_bucket(&bucket).unwrap();
    ds.force_commit().unwrap();
    id.to_string()
}

fn event_at(ts: DateTime<Utc>, duration: Duration, title: &str) -> Event {
    let mut data = serde_json::Map::new();
    data.insert("app".to_string(), json!("VLC"));
    data.insert("title".to_string(), json!(title));
    Event {
        id: None,
        timestamp: ts,
        duration,
        data,
    }
}

fn titles(ds: &Datastore, bucket_id: &str) -> Vec<(DateTime<Utc>, String)> {
    let mut events = ds.get_events(bucket_id, None, None, None).unwrap();
    events.sort_by_key(|e| e.timestamp);
    events
        .into_iter()
        .map(|e| {
            let title = e
                .data
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (e.timestamp, title)
        })
        .collect()
}

/// Match the WebUI/Android edit path: delete the local row, insert a replacement
/// with the same timestamp and duration.
fn edit_title(ds: &Datastore, bucket_id: &str, ts: DateTime<Utc>, new_title: &str) {
    let events = ds.get_events(bucket_id, None, None, None).unwrap();
    let target = events
        .iter()
        .find(|e| e.timestamp == ts)
        .expect("event to edit");
    let id = target.id.expect("source event id");
    ds.delete_events_by_id(bucket_id, vec![id]).unwrap();
    let mut data = target.data.clone();
    data.insert("title".to_string(), json!(new_title));
    ds.insert_events(
        bucket_id,
        &[Event {
            id: None,
            timestamp: target.timestamp,
            duration: target.duration,
            data,
        }],
    )
    .unwrap();
    ds.force_commit().unwrap();
}

fn sync_push(src: &Datastore, dest: &Datastore) {
    aw_sync::sync_datastores(src, dest, true, Some("device-phone"), &SyncSpec::default());
}

fn sync_pull(src: &Datastore, dest: &Datastore) {
    aw_sync::sync_datastores(src, dest, false, None, &SyncSpec::default());
}

#[test]
fn historical_title_edit_reaches_staging() {
    let (src, dest) = memory_pair();
    let bucket = create_bucket(&src, "aw-watcher-android-test", "phone");
    let t0 = Utc::now() - Duration::hours(2);
    let t1 = t0 + Duration::minutes(30);
    src.insert_events(
        &bucket,
        &[
            event_at(t0, Duration::minutes(20), "sanitized.mp4"),
            event_at(t1, Duration::minutes(5), "later window"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();

    sync_push(&src, &dest);
    assert_eq!(
        titles(&dest, &bucket),
        vec![
            (t0, "sanitized.mp4".to_string()),
            (t1, "later window".to_string()),
        ]
    );

    edit_title(&src, &bucket, t0, "Real Video Title");
    sync_push(&src, &dest);

    assert_eq!(
        titles(&dest, &bucket),
        vec![
            (t0, "Real Video Title".to_string()),
            (t1, "later window".to_string()),
        ],
        "older title edit must update staging without duplicating or dropping the later event"
    );
    assert_eq!(dest.get_event_count(&bucket, None, None).unwrap(), 2);
}

#[test]
fn latest_title_edit_reaches_staging() {
    let (src, dest) = memory_pair();
    let bucket = create_bucket(&src, "aw-watcher-android-latest", "phone");
    let t0 = Utc::now() - Duration::hours(1);
    let t1 = t0 + Duration::minutes(40);
    src.insert_events(
        &bucket,
        &[
            event_at(t0, Duration::minutes(10), "older"),
            event_at(t1, Duration::minutes(15), "sanitized-latest.mp4"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();

    sync_push(&src, &dest);
    edit_title(&src, &bucket, t1, "Latest Real Title");
    sync_push(&src, &dest);

    assert_eq!(
        titles(&dest, &bucket),
        vec![
            (t0, "older".to_string()),
            (t1, "Latest Real Title".to_string()),
        ]
    );
    assert_eq!(
        dest.get_event_count(&bucket, None, None).unwrap(),
        2,
        "latest-event title edit must not insert a clipped duplicate"
    );
}

#[test]
fn historical_title_edit_reaches_peer() {
    let (src, staging) = memory_pair();
    let peer = Datastore::new_in_memory(false);
    let bucket = create_bucket(&src, "aw-watcher-android-peer", "phone");
    let t0 = Utc::now() - Duration::hours(3);
    let t1 = t0 + Duration::hours(1);
    src.insert_events(
        &bucket,
        &[
            event_at(t0, Duration::minutes(25), "sanitized.mp4"),
            event_at(t1, Duration::minutes(8), "later window"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();

    sync_push(&src, &staging);
    sync_pull(&staging, &peer);

    let peer_bucket = format!("{bucket}-synced-from-phone");
    assert_eq!(
        titles(&peer, &peer_bucket)[0].1,
        "sanitized.mp4",
        "precondition: peer imported the original title"
    );

    edit_title(&src, &bucket, t0, "Real Video Title");
    sync_push(&src, &staging);
    sync_pull(&staging, &peer);

    assert_eq!(
        titles(&staging, &bucket),
        vec![
            (t0, "Real Video Title".to_string()),
            (t1, "later window".to_string()),
        ]
    );
    assert_eq!(
        titles(&peer, &peer_bucket),
        vec![
            (t0, "Real Video Title".to_string()),
            (t1, "later window".to_string()),
        ],
        "peer pull must pick up the owner-originated edit from staging"
    );
}

#[test]
fn edits_older_than_lookback_are_not_reconciled() {
    // Peak-memory bound: only the 7 days before the resume cursor are compared.
    let (src, dest) = memory_pair();
    let bucket = create_bucket(&src, "aw-watcher-android-lookback", "phone");
    let t_old = Utc::now() - Duration::days(10);
    let t_new = Utc::now() - Duration::minutes(5);
    src.insert_events(
        &bucket,
        &[
            event_at(t_old, Duration::minutes(20), "sanitized-old.mp4"),
            event_at(t_new, Duration::minutes(5), "recent window"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    sync_push(&src, &dest);
    edit_title(&src, &bucket, t_old, "Too Old To Reconcile");
    sync_push(&src, &dest);
    assert_eq!(
        titles(&dest, &bucket),
        vec![
            (t_old, "sanitized-old.mp4".to_string()),
            (t_new, "recent window".to_string()),
        ],
        "edits older than 7 days stay on the resume-cursor path"
    );
}

#[test]
fn wiping_destination_reexports_source_edits() {
    // Control: a truly empty destination (internal staging gone) re-reads source
    // from the beginning. Deleting only the Android SAF mirror does not do this.
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let bucket = create_bucket(&src, "aw-watcher-android-wipe", "phone");
    let t0 = Utc::now() - Duration::hours(2);
    let t1 = t0 + Duration::minutes(30);
    src.insert_events(
        &bucket,
        &[
            event_at(t0, Duration::minutes(20), "sanitized.mp4"),
            event_at(t1, Duration::minutes(5), "later window"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    sync_push(&src, &dest);
    edit_title(&src, &bucket, t0, "Real Video Title");

    let fresh = Datastore::new_in_memory(false);
    sync_push(&src, &fresh);
    assert_eq!(
        titles(&fresh, &bucket),
        vec![
            (t0, "Real Video Title".to_string()),
            (t1, "later window".to_string()),
        ]
    );
}
