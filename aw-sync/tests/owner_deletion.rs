/// Owner-originated event deletions.
///
/// `aw-sync/README.md` states "Event deletion is not synced." The edit
/// reconcile pass (#678) only walks identities that still exist on the source,
/// so a deleted source event has no representation at all and the destination
/// keeps its copy forever under the single-writer model.
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

fn sync_push(src: &Datastore, dest: &Datastore) {
    aw_sync::sync_datastores(src, dest, true, Some("device-phone"), &SyncSpec::default())
        .expect("sync_push failed");
}

fn sync_pull(src: &Datastore, dest: &Datastore) {
    aw_sync::sync_datastores(src, dest, false, None, &SyncSpec::default())
        .expect("sync_pull failed");
}

/// Delete one event on the source, leaving the rest of the bucket intact.
fn delete_event_at(ds: &Datastore, bucket_id: &str, ts: DateTime<Utc>) {
    let events = ds.get_events(bucket_id, None, None, None).unwrap();
    let target = events
        .iter()
        .find(|e| e.timestamp == ts)
        .expect("event to delete");
    let id = target.id.expect("source event id");
    ds.delete_events_by_id(bucket_id, vec![id]).unwrap();
    ds.force_commit().unwrap();
}

#[test]
fn owner_deletion_inside_lookback_reaches_destination() {
    let (src, dest) = memory_pair();
    let bucket = create_bucket(&src, "aw-watcher-android-deletion", "phone");
    let t0 = Utc::now() - Duration::hours(2);
    let t1 = t0 + Duration::minutes(20);
    let t2 = t0 + Duration::minutes(40);
    src.insert_events(
        &bucket,
        &[
            event_at(t0, Duration::minutes(19), "keep-me"),
            event_at(t1, Duration::minutes(19), "delete-me"),
            event_at(t2, Duration::minutes(19), "keep-me-too"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();

    sync_push(&src, &dest);
    assert_eq!(
        titles(&dest, &bucket).len(),
        3,
        "precondition: all three events reach the destination"
    );

    delete_event_at(&src, &bucket, t1);
    sync_push(&src, &dest);

    // Source of truth: the owner deleted this event. The single-writer model
    // makes the source authoritative for its own bucket, so the destination
    // must drop its copy.
    assert_eq!(
        titles(&dest, &bucket),
        vec![(t0, "keep-me".to_string()), (t2, "keep-me-too".to_string()),],
        "owner deletion must not leave the event on the destination"
    );
}

#[test]
fn deletion_of_the_latest_event_reaches_destination() {
    // The deleted event is also the destination's resume cursor in the first
    // sync, so this covers the reconcile-window boundary rather than an
    // interior row. Both sides return it unclipped: the window ends exactly at
    // its own end.
    let (src, dest) = memory_pair();
    let bucket = create_bucket(&src, "aw-watcher-android-deletion-latest", "phone");
    let t0 = Utc::now() - Duration::minutes(40);
    let t1 = t0 + Duration::minutes(20);
    src.insert_events(
        &bucket,
        &[
            event_at(t0, Duration::minutes(19), "keep-me"),
            event_at(t1, Duration::minutes(19), "delete-me"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    sync_push(&src, &dest);

    delete_event_at(&src, &bucket, t1);
    sync_push(&src, &dest);

    assert_eq!(
        titles(&dest, &bucket),
        vec![(t0, "keep-me".to_string())],
        "deleting the newest event must move the destination cursor back"
    );

    // Idempotent: nothing is re-copied on a following pass.
    sync_push(&src, &dest);
    assert_eq!(
        titles(&dest, &bucket),
        vec![(t0, "keep-me".to_string())],
        "a repeated pass must not resurrect or duplicate anything"
    );
}

#[test]
fn deletion_of_latest_zero_duration_event_reaches_destination() {
    let (src, dest) = memory_pair();
    let bucket = create_bucket(&src, "aw-watcher-android-deletion-instant", "phone");
    let t0 = Utc::now() - Duration::minutes(40);
    let t1 = t0 + Duration::minutes(20);
    src.insert_events(
        &bucket,
        &[
            event_at(t0, Duration::minutes(19), "keep-me"),
            event_at(t1, Duration::zero(), "delete-me"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    sync_push(&src, &dest);
    // A live instantaneous boundary event must survive a repeated pass too.
    sync_push(&src, &dest);
    assert_eq!(titles(&dest, &bucket), titles(&src, &bucket));

    delete_event_at(&src, &bucket, t1);
    sync_push(&src, &dest);
    assert_eq!(
        titles(&dest, &bucket),
        vec![(t0, "keep-me".to_string())],
        "a deleted zero-duration event at the resume cursor must be removed"
    );
    sync_push(&src, &dest);
    assert_eq!(titles(&dest, &bucket), titles(&src, &bucket));
}

#[test]
fn deletion_of_one_timestamp_tied_identity_reaches_destination() {
    for deleted_duration in [Duration::zero(), Duration::seconds(60)] {
        let (src, dest) = memory_pair();
        let bucket = create_bucket(&src, "aw-watcher-android-deletion-tied", "phone");
        let t0 = Utc::now() - Duration::minutes(40);
        src.insert_events(
            &bucket,
            &[
                event_at(t0, Duration::zero(), "running"),
                event_at(t0, Duration::seconds(60), "finished"),
                event_at(t0 + Duration::seconds(120), Duration::seconds(60), "later"),
            ],
        )
        .unwrap();
        src.force_commit().unwrap();
        sync_push(&src, &dest);
        assert_eq!(titles(&dest, &bucket).len(), 3);

        let deleted = src
            .get_events(&bucket, None, None, None)
            .unwrap()
            .into_iter()
            .find(|event| event.timestamp == t0 && event.duration == deleted_duration)
            .unwrap();
        src.delete_events_by_id(&bucket, vec![deleted.id.unwrap()])
            .unwrap();
        src.force_commit().unwrap();

        for _ in 0..2 {
            sync_push(&src, &dest);
            assert_eq!(
                titles(&dest, &bucket),
                titles(&src, &bucket),
                "a live timestamp sibling must not preserve the deleted duration {deleted_duration}"
            );
        }
    }
}

#[test]
fn deletion_older_than_lookback_is_not_propagated() {
    // Documents the memory bound: same contract as edits older than 7 days.
    let (src, dest) = memory_pair();
    let bucket = create_bucket(&src, "aw-watcher-android-deletion-old", "phone");
    let t_old = Utc::now() - Duration::days(10);
    let t_new = Utc::now() - Duration::minutes(5);
    src.insert_events(
        &bucket,
        &[
            event_at(t_old, Duration::minutes(19), "old-but-synced"),
            event_at(t_new, Duration::minutes(5), "recent window"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    sync_push(&src, &dest);

    delete_event_at(&src, &bucket, t_old);
    sync_push(&src, &dest);

    assert_eq!(
        titles(&dest, &bucket),
        vec![
            (t_old, "old-but-synced".to_string()),
            (t_new, "recent window".to_string()),
        ],
        "deletions outside the lookback window stay on the documented limitation"
    );
}

#[test]
fn emptied_source_window_does_not_delete_destination_history() {
    // Absence is only evidence of a deletion when the source still shows
    // contemporaneous data. A reset/emptied source datastore must not be read
    // as "the owner deleted everything".
    let (src, dest) = memory_pair();
    let bucket = create_bucket(&src, "aw-watcher-android-deletion-reset", "phone");
    let t0 = Utc::now() - Duration::minutes(40);
    let t1 = t0 + Duration::minutes(20);
    src.insert_events(
        &bucket,
        &[
            event_at(t0, Duration::minutes(19), "keep-me"),
            event_at(t1, Duration::minutes(19), "keep-me-too"),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    sync_push(&src, &dest);

    let ids: Vec<i64> = src
        .get_events(&bucket, None, None, None)
        .unwrap()
        .into_iter()
        .filter_map(|e| e.id)
        .collect();
    src.delete_events_by_id(&bucket, ids).unwrap();
    src.force_commit().unwrap();
    sync_push(&src, &dest);

    assert_eq!(
        titles(&dest, &bucket).len(),
        2,
        "an empty source window is not proof of deletion"
    );
}

#[test]
fn deletion_propagates_through_staging_to_a_third_device() {
    // The desktop pulls from the staging directory, not from the phone. A
    // deletion that stops at the first hop would leave the event on the
    // desktop, and a later desktop push could hand the stale copy back.
    let phone = Datastore::new_in_memory(false);
    let staging = Datastore::new_in_memory(false);
    let desktop = Datastore::new_in_memory(false);
    let bucket = create_bucket(&phone, "aw-watcher-android-deletion-hop", "phone");
    let desktop_bucket = format!("{bucket}-synced-from-phone");

    let t0 = Utc::now() - Duration::hours(2);
    let t1 = t0 + Duration::minutes(20);
    phone
        .insert_events(
            &bucket,
            &[
                event_at(t0, Duration::minutes(19), "keep-me"),
                event_at(t1, Duration::minutes(19), "delete-me"),
            ],
        )
        .unwrap();
    phone.force_commit().unwrap();

    sync_push(&phone, &staging);
    sync_pull(&staging, &desktop);
    assert_eq!(
        titles(&desktop, &desktop_bucket).len(),
        2,
        "precondition: both events reach the second hop"
    );

    delete_event_at(&phone, &bucket, t1);
    sync_push(&phone, &staging);
    sync_pull(&staging, &desktop);

    assert_eq!(
        titles(&desktop, &desktop_bucket),
        vec![(t0, "keep-me".to_string())],
        "the deletion must survive the staging hop"
    );
}
