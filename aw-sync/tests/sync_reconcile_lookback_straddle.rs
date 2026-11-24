//! Reconcile must not truncate an event that straddles the edit lookback start
//! (ActivityWatch/aw-server-rust#875 item 3).
use chrono::{DateTime, Duration, TimeZone, Utc};
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

/// Replace an event in place (same timestamp + duration, new data) the way the
/// WebUI / Android edit path does: delete + insert.
fn edit_data(ds: &Datastore, bucket: &str, ts: DateTime<Utc>, data: serde_json::Value) {
    let target = ds
        .get_events(bucket, None, None, None)
        .unwrap()
        .into_iter()
        .find(|e| e.timestamp == ts)
        .unwrap();
    ds.delete_events_by_id(bucket, vec![target.id.unwrap()])
        .unwrap();
    ds.insert_events(bucket, &[ev(target.timestamp, target.duration, data)])
        .unwrap();
    ds.force_commit().unwrap();
}
/// An edited event that starts before `resume - 7d` and ends inside the window
/// must keep its full extent on the destination. Before the fix, reconcile saw
/// both copies clipped to the lookback start and replaced the dest row with
/// the clipped copy, permanently deleting the part before the boundary.
#[test]
fn edit_of_event_straddling_lookback_start_does_not_truncate_it() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-afk_laptop", "afkstatus", "laptop");
    let base = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    // Long event (10 days) followed by a short one. resume = base+10d+1h,
    // lookback_start = base+3d+1h, which falls inside the long event.
    src.insert_events(
        &b,
        &[
            ev(
                base,
                Duration::days(10),
                json!({"status": "afk", "note": "a"}),
            ),
            ev(
                base + Duration::days(10),
                Duration::hours(1),
                json!({"status": "not-afk"}),
            ),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let dest_b = "aw-watcher-afk_laptop-synced-from-laptop";
    assert_eq!(all_sorted(&dest, dest_b).len(), 2);

    // Owner edits the long event's data (same timestamp + duration).
    edit_data(&src, &b, base, json!({"status": "afk", "note": "edited"}));
    pull(&src, &dest).unwrap();

    let got = all_sorted(&dest, dest_b);
    let afk: Vec<_> = got
        .iter()
        .filter(|e| e.data.get("status") == Some(&json!("afk")))
        .map(|e| (e.timestamp, e.duration))
        .collect();
    assert_eq!(
        afk,
        vec![(base, Duration::days(10))],
        "straddling event must keep its full extent on dest: {got:?}"
    );
}
