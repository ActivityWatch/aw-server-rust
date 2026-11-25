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
                base + Duration::days(3) + Duration::hours(1),
                Duration::minutes(5),
                json!({"status": "cutoff", "note": "a"}),
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
    assert_eq!(all_sorted(&dest, dest_b).len(), 3);

    // Owner edits the long event's data (same timestamp + duration).
    edit_data(&src, &b, base, json!({"status": "afk", "note": "edited"}));
    edit_data(
        &src,
        &b,
        base + Duration::days(3) + Duration::hours(1),
        json!({"status": "cutoff", "note": "edited"}),
    );
    pull(&src, &dest).unwrap();

    let got = all_sorted(&dest, dest_b);
    let cutoff = got
        .iter()
        .find(|e| e.timestamp == base + Duration::days(3) + Duration::hours(1))
        .unwrap();
    assert_eq!(cutoff.duration, Duration::minutes(5));
    assert_eq!(cutoff.data.get("note"), Some(&json!("edited")));
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

/// An edit of the bucket's newest event propagates even when it starts before
/// `resume - 7d`: reconcile always covers dest's newest row. Before, the main
/// pull clipped the edited source copy to a zero-length row at `resume` and
/// stored that fragment next to the untouched old row.
///
/// A genuine point at `resume` with the *same* payload as the edited event is
/// not asserted here: it is heartbeat-merged in the single-page case, exactly
/// as for an unedited event, since reconcile is a no-op once payloads match.
#[test]
fn latest_long_edit_propagates_without_a_zero_length_fragment() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-afk_laptop", "afkstatus", "laptop");
    let base = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    let old_data = json!({"status": "afk", "note": "a"});
    let edited_data = json!({"status": "afk", "note": "edited"});
    src.insert_events(&b, &[ev(base, Duration::days(10), old_data.clone())])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let dest_b = "aw-watcher-afk_laptop-synced-from-laptop";

    edit_data(&src, &b, base, edited_data.clone());
    for _ in 0..2 {
        pull(&src, &dest).unwrap();
        let got = all_sorted(&dest, dest_b);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].timestamp, base);
        assert_eq!(got[0].duration, Duration::days(10));
        assert_eq!(got[0].data, *edited_data.as_object().unwrap());
    }
}

#[test]
fn latest_long_event_duration_extension_still_syncs() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-afk_laptop", "afkstatus", "laptop");
    let base = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    let data = json!({"status": "afk"});
    src.insert_events(&b, &[ev(base, Duration::days(10), data.clone())])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();

    src.heartbeat(
        &b,
        ev(base + Duration::days(10), Duration::hours(1), data.clone()),
        0.0,
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();

    let got = all_sorted(&dest, "aw-watcher-afk_laptop-synced-from-laptop");
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].timestamp, base);
    assert_eq!(got[0].duration, Duration::days(10) + Duration::hours(1));
    assert_eq!(got[0].data, *data.as_object().unwrap());
}

/// Greptile P1 on #890 (r4688969008): a genuine zero-length point at `resume`
/// that carries the edited newest straddler's *old* payload must survive. The
/// boundary dedup budget is built from dest after reconcile; if the straddler
/// edit is not reconciled, dest still holds the old payload and its
/// `(end, old data)` count is spent on the genuine point instead of the clip.
#[test]
fn edited_newest_straddler_does_not_consume_a_genuine_point_with_old_data() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-afk_laptop", "afkstatus", "laptop");
    let base = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    let resume = base + Duration::days(10);
    let old_data = json!({"status": "afk", "note": "old"});
    let edited_data = json!({"status": "afk", "note": "edited"});
    src.insert_events(&b, &[ev(base, Duration::days(10), old_data.clone())])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let dest_b = "aw-watcher-afk_laptop-synced-from-laptop";

    edit_data(&src, &b, base, edited_data.clone());
    src.insert_events(&b, &[ev(resume, Duration::zero(), old_data.clone())])
        .unwrap();
    src.force_commit().unwrap();

    for _ in 0..2 {
        pull(&src, &dest).unwrap();
        let got = all_sorted(&dest, dest_b);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].timestamp, base);
        assert_eq!(got[0].duration, Duration::days(10));
        assert_eq!(got[0].data, *edited_data.as_object().unwrap());
        assert_eq!(got[1].timestamp, resume);
        assert_eq!(got[1].duration, Duration::zero());
        assert_eq!(got[1].data, *old_data.as_object().unwrap());
    }
}

/// Greptile P1 on #890 at `05ec60a`: two events share a start and the shorter
/// one sets `resume`. The reconcile fetch ends at `resume`, which clips the
/// longer event to the shorter one's identity. An edit of the longer event
/// must not replace its full row with that clipped copy.
#[test]
fn edit_of_longer_same_start_event_is_not_truncated_to_resume() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-afk_laptop", "afkstatus", "laptop");
    let base = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    let short = json!({"status": "afk", "note": "short"});
    let long = json!({"status": "afk", "note": "long"});
    let edited = json!({"status": "afk", "note": "long-edited"});
    src.insert_events(
        &b,
        &[
            ev(base, Duration::days(10), short.clone()),
            ev(base, Duration::days(12), long.clone()),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let dest_b = "aw-watcher-afk_laptop-synced-from-laptop";

    let target = src
        .get_events(&b, None, None, None)
        .unwrap()
        .into_iter()
        .find(|e| e.duration == Duration::days(12))
        .unwrap();
    src.delete_events_by_id(&b, vec![target.id.unwrap()])
        .unwrap();
    src.insert_events(&b, &[ev(base, Duration::days(12), edited.clone())])
        .unwrap();
    src.force_commit().unwrap();

    for _ in 0..2 {
        pull(&src, &dest).unwrap();
        let got = all_sorted(&dest, dest_b);
        assert!(
            got.iter().any(|e| e.timestamp == base
                && e.duration == Duration::days(10)
                && e.data == *short.as_object().unwrap()),
            "short event lost: {got:?}"
        );
        assert!(
            got.iter()
                .any(|e| e.timestamp == base && e.duration == Duration::days(12)),
            "long event truncated: {got:?}"
        );
        assert!(
            !got.iter().any(|e| e.timestamp == base
                && e.duration == Duration::days(10)
                && e.data != *short.as_object().unwrap()),
            "clipped copy of the long event stored: {got:?}"
        );
    }
}

/// Greptile P1 on #890 at `5f4da41`: reconcile inserts before it deletes. If
/// a pass stops between the two, dest holds the edited newest straddler and
/// its stale copy under one identity. The retry must remove the stale copy.
#[test]
fn retry_after_interrupted_newest_straddler_edit_removes_stale_copy() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-afk_laptop", "afkstatus", "laptop");
    let base = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    let old_data = json!({"status": "afk", "note": "old"});
    let edited_data = json!({"status": "afk", "note": "edited"});
    src.insert_events(&b, &[ev(base, Duration::days(10), old_data.clone())])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let dest_b = "aw-watcher-afk_laptop-synced-from-laptop";

    edit_data(&src, &b, base, edited_data.clone());
    // The interrupted pass's insert landed; its delete did not.
    dest.insert_events(dest_b, &[ev(base, Duration::days(10), edited_data.clone())])
        .unwrap();
    dest.force_commit().unwrap();

    for pass in 0..2 {
        let result = pull(&src, &dest);
        // Deleting the stale copy shrinks the bucket, which master's shrink
        // check reports as a failure (fixed separately in #891). The cleanup
        // has already happened by then; the next pass must succeed.
        if pass > 0 {
            result.unwrap();
        }
        let got = all_sorted(&dest, dest_b);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].timestamp, base);
        assert_eq!(got[0].duration, Duration::days(10));
        assert_eq!(got[0].data, *edited_data.as_object().unwrap());
    }
}
