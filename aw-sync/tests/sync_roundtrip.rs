/// Regression test for the "synced-from-<own hostname>" duplication reported in
/// https://github.com/orgs/ActivityWatch/discussions/1373
///
/// aw-sync already refuses to import its *own* export (`find_remotes_nonlocal`
/// filters the local device_id out of the remote list). That guard only covers
/// the direct path. It does not cover the case where a host's data is laundered
/// through a *peer*: the peer imports HOSTA's buckets, then re-exports them as
/// part of its own push, and HOSTA imports them back as
/// `<bucket>_HOSTA-synced-from-HOSTA` sitting next to the real local bucket.
///
/// Both copies then render in /timeline, so every event is shown twice.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aw_datastore::Datastore;
use aw_models::{Bucket, BucketMetadata};
use aw_sync::{sync_datastores, AccessMethod, SyncSpec};

// Tests in this binary run in parallel. Use a monotonic counter to guarantee
// each datastore gets a unique path even when two tests start within the same
// clock tick (seen on macOS where SystemTime resolution can be coarser than
// nanoseconds, causing path collisions and SQLite migration races).
static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmp_db(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "aw-sync-roundtrip-{}-{}-{}.db",
        std::process::id(),
        name,
        DB_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    p
}

fn datastore(name: &str) -> Datastore {
    Datastore::new(tmp_db(name).to_str().unwrap().to_string(), false)
}

fn bucket(id: &str, hostname: &str) -> Bucket {
    Bucket {
        bid: None,
        id: id.to_string(),
        _type: "currentwindow".to_string(),
        client: "aw-watcher-window".to_string(),
        hostname: hostname.to_string(),
        created: None,
        data: serde_json::Map::new(),
        metadata: BucketMetadata::default(),
        events: None,
        last_updated: None,
    }
}

fn bucket_ids(ds: &dyn AccessMethod) -> Vec<String> {
    let mut ids: Vec<String> = ds.get_buckets().unwrap().keys().cloned().collect();
    ids.sort();
    ids
}

/// Two hosts sharing a sync folder. Returns (a_local, b_export) after a full
/// push/pull round: HOSTA pushes, HOSTB pulls, HOSTB pushes, HOSTA pulls.
fn round_trip() -> (Datastore, Datastore) {
    let spec = SyncSpec::default();

    // HOSTA: local server datastore + the db it pushes into the sync folder.
    let a_local = datastore("a-local");
    let a_export = datastore("a-export");
    // HOSTB: same.
    let b_local = datastore("b-local");
    let b_export = datastore("b-export");

    // HOSTA collects some data locally.
    a_local
        .create_bucket(&bucket("aw-watcher-window_HOSTA", "HOSTA"))
        .unwrap();

    // 1. HOSTA pushes to its own folder in the sync dir.
    sync_datastores(&a_local, &a_export, true, Some("device-A"), &spec);

    // 2. HOSTB pulls HOSTA's export. This copy is correct and expected.
    sync_datastores(&a_export, &b_local, false, None, &spec);
    assert!(
        bucket_ids(&b_local).contains(&"aw-watcher-window_HOSTA-synced-from-HOSTA".to_string()),
        "precondition: HOSTB should hold HOSTA's data as a synced-from-HOSTA bucket, got {:?}",
        bucket_ids(&b_local)
    );

    // 3. HOSTB pushes its own data to the sync folder.
    sync_datastores(&b_local, &b_export, true, Some("device-B"), &spec);

    // 4. HOSTA pulls HOSTB's export.
    sync_datastores(&b_export, &a_local, false, None, &spec);

    (a_local, b_export)
}

/// The fix location: a push must export only buckets that originate on this
/// host. Re-exporting buckets pulled from a peer is what lets data round-trip.
#[test]
fn test_push_does_not_reexport_synced_buckets() {
    let (_a_local, b_export) = round_trip();
    assert_eq!(
        bucket_ids(&b_export),
        Vec::<String>::new(),
        "HOSTB's export must not contain buckets it merely synced from HOSTA"
    );
}

/// The reported symptom: HOSTA must never end up with a `-synced-from-HOSTA`
/// copy of its own bucket, which renders every event twice in /timeline.
#[test]
fn test_own_data_does_not_return_via_peer() {
    let (a_local, _b_export) = round_trip();
    assert_eq!(
        bucket_ids(&a_local),
        vec!["aw-watcher-window_HOSTA".to_string()],
        "HOSTA must not gain a synced-from-HOSTA copy of its own bucket"
    );
}

/// `-synced-from-` is a reserved token in aw-sync's ID grammar, not just a
/// naming convention. `get_or_create_sync_bucket` splits on it to recover the
/// original bucket ID, so a first-hand bucket whose own ID contains it is
/// already mangled on import regardless of provenance filtering.
///
/// This pins that pre-existing behaviour so the reservation is explicit: such a
/// bucket is treated as a synced copy. Issue #649 tracks moving provenance into
/// bucket metadata, which removes the dependency on the ID string.
#[test]
fn test_synced_from_is_a_reserved_id_token() {
    let spec = SyncSpec::default();
    let local = datastore("reserved-local");
    let export = datastore("reserved-export");

    // A bucket the sync layer would mangle anyway: the ID grammar reserves the
    // token, so this is not a supported first-hand bucket name.
    local
        .create_bucket(&bucket(
            "aw-watcher-window_HOSTA-synced-from-HOSTB",
            "HOSTA",
        ))
        .unwrap();
    // A normally-named bucket alongside it, to prove the filter is not blanket.
    local
        .create_bucket(&bucket("aw-watcher-afk_HOSTA", "HOSTA"))
        .unwrap();

    sync_datastores(&local, &export, true, Some("device-A"), &spec);

    assert_eq!(
        bucket_ids(&export),
        vec!["aw-watcher-afk_HOSTA".to_string()],
        "the reserved token marks a bucket as synced; unmarked buckets still export"
    );
}
