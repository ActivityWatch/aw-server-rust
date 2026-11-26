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
    assert_eq!(got[0].timestamp, t);
    assert_eq!(got[0].duration, Duration::minutes(25));
    assert_eq!(got[0].data.get("running"), Some(&json!(false)));
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

#[test]
fn overlapping_stopped_timer_keeps_its_full_duration() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "timers", "general.stopwatch", "laptop");
    let t = Utc::now() - Duration::hours(3);
    let inserted = src
        .insert_events(
            &b,
            &[
                ev(t, Duration::hours(2), json!({"label": "long"})),
                ev(
                    t + Duration::hours(1),
                    Duration::zero(),
                    json!({"label": "short", "running": true}),
                ),
            ],
        )
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let mut stopped = ev(
        t + Duration::hours(1),
        Duration::minutes(30),
        json!({"label": "short", "running": false}),
    );
    stopped.id = inserted[1].id;
    src.insert_events(&b, &[stopped]).unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "timers-synced-from-laptop");
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[1].timestamp, t + Duration::hours(1));
    assert_eq!(got[1].duration, Duration::minutes(30));
    assert_eq!(got[1].data.get("running"), Some(&json!(false)));
}

#[test]
fn touching_title_edit_keeps_two_separate_events() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "timers", "general.stopwatch", "laptop");
    let t = Utc::now() - Duration::hours(3);
    src.insert_events(
        &b,
        &[
            ev(t, Duration::minutes(10), json!({"label": "same"})),
            ev(
                t + Duration::minutes(10),
                Duration::minutes(10),
                json!({"label": "old"}),
            ),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    edit_data(
        &src,
        &b,
        t + Duration::minutes(10),
        json!({"label": "same"}),
    );
    pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "timers-synced-from-laptop");
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].duration, Duration::minutes(10));
    assert_eq!(got[1].timestamp, t + Duration::minutes(10));
    assert_eq!(got[1].duration, Duration::minutes(10));
    assert_eq!(got[0].data, got[1].data);
}

/// Fail one destination write while retaining real datastore semantics for all
/// other operations. This models an interrupted pass at each persistence boundary.
#[derive(Debug)]
struct FailingDestination<'a> {
    inner: &'a Datastore,
    fail_insert: std::cell::Cell<bool>,
    fail_delete: std::cell::Cell<bool>,
}

impl aw_sync::AccessMethod for FailingDestination<'_> {
    fn get_buckets(&self) -> Result<std::collections::HashMap<String, Bucket>, String> {
        aw_sync::AccessMethod::get_buckets(self.inner)
    }
    fn get_bucket(&self, id: &str) -> Result<Bucket, aw_datastore::DatastoreError> {
        aw_sync::AccessMethod::get_bucket(self.inner, id)
    }
    fn create_bucket(&self, bucket: &Bucket) -> Result<(), aw_datastore::DatastoreError> {
        aw_sync::AccessMethod::create_bucket(self.inner, bucket)
    }
    fn get_events(
        &self,
        id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: Option<u64>,
    ) -> Result<Vec<Event>, String> {
        aw_sync::AccessMethod::get_events(self.inner, id, start, end, limit)
    }
    fn get_event(&self, id: &str, event_id: i64) -> Result<Event, String> {
        aw_sync::AccessMethod::get_event(self.inner, id, event_id)
    }
    fn get_event_count(&self, id: &str) -> Result<i64, String> {
        aw_sync::AccessMethod::get_event_count(self.inner, id)
    }
    fn insert_events(&self, id: &str, events: Vec<Event>) -> Result<(), String> {
        if self.fail_insert.replace(false) {
            return Err("injected insert failure".into());
        }
        aw_sync::AccessMethod::insert_events(self.inner, id, events)
    }
    fn heartbeat(&self, id: &str, event: Event, duration: f64) -> Result<(), String> {
        if self.fail_insert.replace(false) {
            return Err("injected insert failure".into());
        }
        aw_sync::AccessMethod::heartbeat(self.inner, id, event, duration)
    }
    fn delete_events_by_id(&self, id: &str, ids: Vec<i64>) -> Result<(), String> {
        if self.fail_delete.replace(false) {
            return Err("injected delete failure".into());
        }
        aw_sync::AccessMethod::delete_events_by_id(self.inner, id, ids)
    }
}

#[test]
fn failed_replacement_write_preserves_old_row() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "timers", "general.stopwatch", "laptop");
    let t = Utc::now() - Duration::hours(1);
    src.insert_events(&b, &[ev(t, Duration::zero(), json!({"label": "old"}))])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    edit_data(&src, &b, t, json!({"label": "new"}));
    let failing = FailingDestination {
        inner: &dest,
        fail_insert: true.into(),
        fail_delete: false.into(),
    };
    let result = aw_sync::sync_datastores(&src, &failing, false, None, &SyncSpec::default());
    assert!(result.is_err());
    let got = all_sorted(&dest, "timers-synced-from-laptop");
    assert_eq!(
        got.len(),
        1,
        "failed replacement must preserve the original: {got:?}"
    );
    assert_eq!(got[0].data.get("label"), Some(&json!("old")));
    pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "timers-synced-from-laptop");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].data.get("label"), Some(&json!("new")));
}

#[test]
fn retry_after_failed_delete_keeps_one_full_replacement() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "timers", "general.stopwatch", "laptop");
    let t = Utc::now() - Duration::hours(1);
    let inserted = src
        .insert_events(&b, &[ev(t, Duration::zero(), json!({"running": true}))])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let mut stopped = ev(t, Duration::minutes(25), json!({"running": false}));
    stopped.id = inserted[0].id;
    src.insert_events(
        &b,
        &[
            stopped,
            ev(
                t + Duration::minutes(10),
                Duration::minutes(1),
                json!({"label": "new overlap"}),
            ),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    let failing = FailingDestination {
        inner: &dest,
        fail_insert: false.into(),
        fail_delete: true.into(),
    };
    let result = aw_sync::sync_datastores(&src, &failing, false, None, &SyncSpec::default());
    assert!(result.is_err());
    let got = all_sorted(&dest, "timers-synced-from-laptop");
    assert!(
        got.iter()
            .any(|e| e.duration == Duration::minutes(25)
                && e.data.get("running") == Some(&json!(false))),
        "replacement must be durable before deletion: {got:?}"
    );
    pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "timers-synced-from-laptop");
    assert_eq!(
        got.len(),
        2,
        "retry must remove stale row and copy the overlap: {got:?}"
    );
    assert_eq!(got[0].timestamp, t);
    assert_eq!(got[0].duration, Duration::minutes(25));
    assert_eq!(got[0].data.get("running"), Some(&json!(false)));
    assert_eq!(got[1].timestamp, t + Duration::minutes(10));
    assert_eq!(got[1].duration, Duration::minutes(1));
}

#[test]
fn longer_replacement_does_not_skip_new_overlapping_events() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "timers", "general.stopwatch", "laptop");
    let t = Utc::now() - Duration::hours(1);
    let inserted = src
        .insert_events(&b, &[ev(t, Duration::minutes(5), json!({"running": true}))])
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let mut stopped = ev(t, Duration::minutes(25), json!({"running": false}));
    stopped.id = inserted[0].id;
    src.insert_events(
        &b,
        &[
            stopped,
            ev(
                t + Duration::minutes(8),
                Duration::minutes(1),
                json!({"label": "new"}),
            ),
        ],
    )
    .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "timers-synced-from-laptop");
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].timestamp, t);
    assert_eq!(got[0].duration, Duration::minutes(25));
    assert_eq!(got[1].timestamp, t + Duration::minutes(8));
    assert_eq!(got[1].duration, Duration::minutes(1));
}

#[test]
fn replacement_matching_a_same_timestamp_sibling_keeps_both_durations() {
    let src = Datastore::new_in_memory(false);
    let dest = Datastore::new_in_memory(false);
    let b = create_bucket(&src, "timers", "general.stopwatch", "laptop");
    let t = Utc::now() - Duration::hours(1);
    let inserted = src
        .insert_events(
            &b,
            &[
                ev(t, Duration::minutes(20), json!({"label": "same"})),
                ev(t, Duration::minutes(10), json!({"label": "old"})),
            ],
        )
        .unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let mut edited = ev(t, Duration::minutes(10), json!({"label": "same"}));
    edited.id = inserted[1].id;
    src.insert_events(&b, &[edited]).unwrap();
    src.force_commit().unwrap();
    pull(&src, &dest).unwrap();
    let got = all_sorted(&dest, "timers-synced-from-laptop");
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].duration, Duration::minutes(10));
    assert_eq!(got[1].duration, Duration::minutes(20));
    assert_eq!(got[0].data, got[1].data);
}
