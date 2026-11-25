//! A reconcile pass that removes a stale destination row is not a failure
//! (ActivityWatch/aw-server-rust#875 item 14).
use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use aw_datastore::Datastore;
use aw_models::{Bucket, Event};
use aw_sync::SyncSpec;

fn create_bucket(ds: &Datastore, id: &str, btype: &str, hostname: &str) -> String {
    let bucket: Bucket = serde_json::from_value(json!({
        "id": id, "type": btype, "hostname": hostname, "client": "test"
    }))
    .unwrap();
    ds.create_bucket(&bucket).unwrap();
    ds.force_commit().unwrap();
    id.to_string()
}

fn ev(ts: DateTime<Utc>, dur: Duration, data: serde_json::Value) -> Event {
    Event {
        id: None,
        timestamp: ts,
        duration: dur,
        data: data.as_object().unwrap().clone(),
    }
}

fn pull(src: &Datastore, dest: &Datastore) -> Result<Vec<aw_sync::BucketReport>, String> {
    aw_sync::sync_datastores(src, dest, false, None, &SyncSpec::default())
}

fn all_sorted(ds: &Datastore, bucket: &str) -> Vec<Event> {
    let mut v = ds.get_events(bucket, None, None, None).unwrap();
    v.sort_by_key(|e| (e.timestamp, e.duration));
    v
}

/// Reconcile deletes a pre-existing destination duplicate (from before #713).
/// The event count shrinks, but nothing went wrong: the pull must succeed and
/// leave the destination mirroring the source.
#[test]
fn reconcile_dedup_is_not_reported_as_failure() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-window_laptop", "currentwindow", "laptop");
    let t0 = Utc::now() - Duration::hours(3);
    let t1 = t0 + Duration::minutes(10);
    src.insert_events(
        &b,
        &[
            ev(t0, Duration::minutes(10), json!({"title": "x"})),
            ev(t1, Duration::minutes(5), json!({"title": "y"})),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    // Simulate a pre-#713 duplicate row already in the destination.
    let dest_b = "aw-watcher-window_laptop-synced-from-laptop";
    dest.insert_events(
        dest_b,
        &[ev(t0, Duration::minutes(10), json!({"title": "x"}))],
    )
    .unwrap();
    dest.force_commit().unwrap();
    let r = pull(&src, &dest);
    assert!(r.is_ok(), "no-op pass reported failure: {r:?}");
    assert_eq!(all_sorted(&dest, dest_b).len(), 2);
}

/// Cleanup removes one stale row while the copy phase adds one real event.
/// A pre-cleanup count baseline would incorrectly report zero new events.
#[test]
fn reconcile_dedup_still_counts_new_events() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-window_laptop", "currentwindow", "laptop");
    let t0 = Utc::now() - Duration::hours(3);
    let t1 = t0 + Duration::minutes(10);
    let t2 = t0 + Duration::minutes(30);
    src.insert_events(
        &b,
        &[
            ev(t0, Duration::minutes(10), json!({"title": "x"})),
            ev(t1, Duration::minutes(5), json!({"title": "y"})),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let dest_b = "aw-watcher-window_laptop-synced-from-laptop";
    dest.insert_events(
        dest_b,
        &[ev(t0, Duration::minutes(10), json!({"title": "x"}))],
    )
    .unwrap();
    dest.force_commit().unwrap();
    let new_event = ev(t2, Duration::minutes(3), json!({"title": "new"}));
    src.insert_events(&b, &[new_event.clone()]).unwrap();
    src.force_commit().unwrap();

    let report = pull(&src, &dest).unwrap();
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].events_new, 1);
    let got = all_sorted(&dest, dest_b);
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].timestamp, t0);
    assert_eq!(got[1].timestamp, t1);
    assert_eq!(got[2].timestamp, new_event.timestamp);
    assert_eq!(got[2].duration, new_event.duration);
    assert_eq!(got[2].data, new_event.data);
    assert_eq!(pull(&src, &dest).unwrap()[0].events_new, 0);
}

/// Reconcile can also add rows without deleting any: a source row sharing an
/// existing row's `(timestamp, duration)` with different data is inserted.
/// That pass imported data, so it must not report zero new events.
#[test]
fn rows_added_by_reconcile_are_counted() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-window_laptop", "currentwindow", "laptop");
    let t0 = Utc::now() - Duration::hours(3);
    let t1 = t0 + Duration::minutes(10);
    src.insert_events(
        &b,
        &[
            ev(t0, Duration::minutes(10), json!({"title": "x"})),
            ev(t1, Duration::minutes(5), json!({"title": "y"})),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    src.insert_events(&b, &[ev(t0, Duration::minutes(10), json!({"title": "z"}))])
        .unwrap();
    src.force_commit().unwrap();

    let report = pull(&src, &dest).unwrap();
    assert_eq!(report[0].events_new, 1, "{report:?}");
    let dest_b = "aw-watcher-window_laptop-synced-from-laptop";
    assert_eq!(all_sorted(&dest, dest_b).len(), 3);
    assert_eq!(pull(&src, &dest).unwrap()[0].events_new, 0);
}

/// An inserted source sibling must count even if cleanup removes a duplicate
/// in the same pass, whether at the same identity or a different one.
#[test]
fn reconcile_insert_is_counted_alongside_duplicate_cleanup() {
    for cleanup_same_identity in [true, false] {
        let src = Datastore::new_in_memory(false);
        let dest = Datastore::new_in_memory(false);
        let b = create_bucket(&src, "aw-watcher-window_laptop", "currentwindow", "laptop");
        let t0 = Utc::now() - Duration::hours(3);
        let t1 = t0 + Duration::minutes(10);
        let x = ev(t0, Duration::minutes(10), json!({"title": "x"}));
        let y = ev(t1, Duration::minutes(5), json!({"title": "y"}));
        src.insert_events(&b, &[x.clone(), y.clone()]).unwrap();
        src.force_commit().unwrap();
        pull(&src, &dest).unwrap();
        let dest_b = "aw-watcher-window_laptop-synced-from-laptop";
        dest.insert_events(dest_b, &[if cleanup_same_identity { x } else { y }])
            .unwrap();
        dest.force_commit().unwrap();
        let z = ev(t0, Duration::minutes(10), json!({"title": "z"}));
        src.insert_events(&b, &[z.clone()]).unwrap();
        src.force_commit().unwrap();

        let report = pull(&src, &dest).unwrap();
        let got = all_sorted(&dest, dest_b);
        assert_eq!(got.len(), 3);
        assert_eq!(got.iter().filter(|e| e.data == z.data).count(), 1);
        assert_eq!(report[0].events_new, 1, "{report:?}");
        assert_eq!(pull(&src, &dest).unwrap()[0].events_new, 0);
    }
}

/// Replacing an edited payload imports a row even though bucket size is stable.
#[test]
fn reconcile_replacement_is_reported_as_imported() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-window_laptop", "currentwindow", "laptop");
    let t0 = Utc::now() - Duration::hours(3);
    src.insert_events(
        &b,
        &[ev(t0, Duration::minutes(10), json!({"title": "old"}))],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let old_id = all_sorted(&src, &b)[0].id.unwrap();
    src.delete_events_by_id(&b, vec![old_id]).unwrap();
    let edited = ev(t0, Duration::minutes(10), json!({"title": "edited"}));
    src.insert_events(&b, &[edited.clone()]).unwrap();
    src.force_commit().unwrap();

    let report = pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "aw-watcher-window_laptop-synced-from-laptop");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].data, edited.data);
    assert_eq!(report[0].events_new, 1, "{report:?}");
    assert_eq!(pull(&src, &dest).unwrap()[0].events_new, 0);
}
