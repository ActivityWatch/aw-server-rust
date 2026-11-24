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
