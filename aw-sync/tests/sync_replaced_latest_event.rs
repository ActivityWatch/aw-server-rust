//! A replaced latest event is replaced on the destination, not duplicated
//! (ActivityWatch/aw-server-rust#875 item 4).
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

/// A zero-duration latest event (a running stopwatch timer) that is later
/// replaced on the source (stopped: data + duration change) must replace the
/// running row on the destination, not sit next to it.
#[test]
fn stopped_stopwatch_replaces_running_row_on_dest() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-stopwatch", "general.stopwatch", "laptop");
    let t = Utc::now() - Duration::hours(1);
    let inserted = src
        .insert_events(
            &b,
            &[ev(
                t,
                Duration::zero(),
                json!({"label": "work", "running": true}),
            )],
        )
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();

    // Stop the timer: aw-webui replaces the event by id with the final
    // duration and running=false.
    let mut stopped = ev(
        t,
        Duration::minutes(25),
        json!({"label": "work", "running": false}),
    );
    stopped.id = inserted[0].id;
    src.insert_events(&b, &[stopped]).unwrap();
    src.force_commit().unwrap();
    assert_eq!(src.get_events(&b, None, None, None).unwrap().len(), 1);

    pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "aw-stopwatch-synced-from-laptop");
    assert_eq!(
        got.len(),
        1,
        "dest should mirror the single stopped timer, got {got:?}"
    );
}

/// A title edit of a zero-duration latest event replaces it on the destination.
#[test]
fn title_edit_of_zero_duration_latest_event_replaces_it() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-window_laptop", "currentwindow", "laptop");
    let t0 = Utc::now() - Duration::hours(2);
    let t1 = t0 + Duration::minutes(10);
    src.insert_events(
        &b,
        &[
            ev(t0, Duration::minutes(10), json!({"app": "a", "title": "x"})),
            ev(t1, Duration::zero(), json!({"app": "b", "title": "old"})),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    edit_data(&src, &b, t1, json!({"app": "b", "title": "new"}));
    pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "aw-watcher-window_laptop-synced-from-laptop");
    let titles: Vec<_> = got
        .iter()
        .map(|e| e.data.get("title").cloned().unwrap())
        .collect();
    assert_eq!(
        titles,
        vec![json!("x"), json!("new")],
        "stale title must be replaced, got {got:?}"
    );
}

/// Same with a non-zero latest event whose data and duration both change.
#[test]
fn replaced_nonzero_latest_event_is_not_duplicated() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-stopwatch", "general.stopwatch", "laptop");
    let t = Utc::now() - Duration::hours(1);
    let inserted = src
        .insert_events(&b, &[ev(t, Duration::minutes(5), json!({"label": "a"}))])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();

    let mut replaced = ev(t, Duration::minutes(10), json!({"label": "b"}));
    replaced.id = inserted[0].id;
    src.insert_events(&b, &[replaced]).unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();

    let got: Vec<_> = all_sorted(&dest, "aw-stopwatch-synced-from-laptop")
        .into_iter()
        .map(|e| (e.timestamp, e.duration, e.data))
        .collect();
    assert_eq!(
        got,
        vec![(
            t,
            Duration::minutes(10),
            json!({"label": "b"}).as_object().unwrap().clone()
        )]
    );
}

/// A duration-only extension keeps the destination row (heartbeat path).
#[test]
fn extended_latest_event_is_kept_and_extended() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "aw-watcher-window_laptop", "currentwindow", "laptop");
    let t = Utc::now() - Duration::hours(1);
    let inserted = src
        .insert_events(&b, &[ev(t, Duration::minutes(5), json!({"app": "a"}))])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let dest_id = all_sorted(&dest, "aw-watcher-window_laptop-synced-from-laptop")[0].id;

    let mut extended = ev(t, Duration::minutes(10), json!({"app": "a"}));
    extended.id = inserted[0].id;
    src.insert_events(&b, &[extended]).unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();

    let got = all_sorted(&dest, "aw-watcher-window_laptop-synced-from-laptop");
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(
        (got[0].id, got[0].duration),
        (dest_id, Duration::minutes(10))
    );
}
