#[macro_use]
extern crate log;
extern crate aw_sync;

#[cfg(test)]
mod sync_tests {
    use std::collections::HashMap;
    use std::path::Path;

    use chrono::{DateTime, Duration, Utc};

    use aw_datastore::{Datastore, DatastoreError};
    use aw_models::{Bucket, Event};
    use aw_sync::{create_datastore, AccessMethod, SyncSpec};

    struct TestState {
        ds_src: Datastore,
        ds_dest: Datastore,
    }

    fn init_teststate() -> TestState {
        TestState {
            ds_src: Datastore::new_in_memory(false),
            ds_dest: Datastore::new_in_memory(false),
        }
    }

    fn create_bucket(ds: &Datastore, n: i32) -> String {
        // Create a bucket
        let bucket_id = format!("bucket-{n}");
        let bucket_jsonstr = format!(
            r#"{{
            "id": "{bucket_id}",
            "type": "test",
            "hostname": "device-{n}",
            "client": "test"
        }}"#
        );
        let bucket: Bucket = serde_json::from_str(&bucket_jsonstr).unwrap();
        match ds.create_bucket(&bucket) {
            Ok(()) => (),
            Err(e) => match e {
                DatastoreError::BucketAlreadyExists(_) => {
                    debug!("bucket already exists, skipping");
                }
                e => panic!("woops! {e:?}"),
            },
        };
        bucket_id
    }

    fn create_event(data_str: &str) -> Event {
        // A workaround needed because otherwise events might get same timestamp if
        // call is repeated quickly on platforms with a low-precision clock.
        std::thread::sleep(std::time::Duration::from_millis(5));

        let timestamp: DateTime<Utc> = Utc::now();
        let event_jsonstr = format!(
            r#"{{
            "timestamp": "{}",
            "duration": 0,
            "data": {{"test": {} }}
        }}"#,
            timestamp.to_rfc3339(),
            data_str
        );
        serde_json::from_str(&event_jsonstr).unwrap()
    }

    fn create_events(ds: &Datastore, bucket_id: &str, n: i64) {
        let events: Vec<Event> = (0..n)
            .map(|i| create_event(format!("{i}").as_str()))
            .collect::<Vec<Event>>();

        ds.insert_events(bucket_id, &events[..]).unwrap();
        ds.force_commit().unwrap();
    }

    fn get_all_buckets(datastores: Vec<&Datastore>) -> Vec<(&Datastore, Bucket)> {
        let mut all_buckets: Vec<(&Datastore, Bucket)> = Vec::new();
        for ds in datastores {
            let buckets = ds.get_buckets().unwrap();
            for bucket in buckets.values() {
                all_buckets.push((ds, bucket.clone()));
            }
        }
        all_buckets
    }

    fn get_all_buckets_map(datastores: Vec<&Datastore>) -> HashMap<String, (&Datastore, Bucket)> {
        let all_buckets = get_all_buckets(datastores);
        all_buckets
            .iter()
            .cloned()
            .map(|(ds, b)| (b.id.clone(), (ds, b)))
            .collect()
    }

    /// A datastore failure must not panic.
    ///
    /// On Android the sync step runs inside a JNI `extern "C"` frame, where an
    /// unwinding panic aborts the whole app process rather than surfacing as an
    /// exception — the SIGABRT in ActivityWatch/aw-android#220. `sync_datastores`
    /// used to `unwrap()` every datastore call, so any failure here was fatal.
    ///
    /// Since the per-bucket non-fatal change (#692), a broken *sibling* does
    /// not abort the whole pass. A pass where every bucket fails is still Err
    /// so callers can tell it from success. No panic is the key invariant.
    #[test]
    fn test_unusable_datastore_does_not_panic() {
        let state = init_teststate();
        create_bucket(&state.ds_src, 0);

        // A datastore whose database file cannot be opened: its worker is gone,
        // so every request fails.
        let ds_broken = create_datastore(Path::new(
            "/nonexistent-directory-for-aw-sync-tests/test.db",
        ))
        .expect("path is valid UTF-8");

        // Previously this panicked (unwrap on datastore failure). Per-bucket
        // skip makes a *partial* failure non-fatal, but every bucket failing
        // (destination down) must still be Err so callers can tell it from
        // success. The key property: it must not panic.
        let result = aw_sync::sync_datastores(
            &state.ds_src,
            &ds_broken,
            true,
            Some("device-0"),
            &SyncSpec::default(),
        );
        let err =
            result.expect_err("total bucket failure must return Err, not Ok(()); must not panic");
        assert!(
            err.contains("all 1 buckets failed"),
            "error should report total failure, got: {err}"
        );
    }

    /// Pulling a peer whose bucket hostname contains whitespace must not fail with
    /// a 400: aw-server-rust rejects new buckets with whitespace hostnames (#658).
    /// `get_or_create_sync_bucket` must sanitize the hostname (and derived ID)
    /// before creating, while still re-using any legacy unsanitized bucket that
    /// was imported before the sanitization was added.
    #[test]
    fn test_whitespace_hostname_pull_creates_sanitized_bucket() {
        let state = init_teststate();

        // Source bucket whose hostname contains a space, as produced by Android
        // devices that were named before aw-android added hostname sanitization
        // (ActivityWatch/aw-android#272).
        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "aw-watcher-android",
            "type": "currentwindow",
            "hostname": "POCO F8 Ultra",
            "client": "aw-android"
        }))
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        // In-memory datastore does not enforce the server-side whitespace check,
        // so the sync completes without 400.
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false, // pull
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let dest_buckets = state.ds_dest.get_buckets().unwrap();

        // Must match aw-android's sanitizeDeviceHostname, not a whitespace-only
        // replace ("POCO_F8_Ultra" would fork when Android migrates).
        let sanitized_id = "aw-watcher-android-synced-from-poco_f8_ultra";
        assert!(
            dest_buckets.contains_key(sanitized_id),
            "expected sanitized bucket id '{sanitized_id}', got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
        // No whitespace bucket must have been created.
        let whitespace_id = "aw-watcher-android-synced-from-POCO F8 Ultra";
        assert!(
            !dest_buckets.contains_key(whitespace_id),
            "whitespace bucket id '{whitespace_id}' must not be created"
        );
        assert!(
            !dest_buckets.contains_key("aw-watcher-android-synced-from-POCO_F8_Ultra"),
            "whitespace-only replace must not be used; got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );

        // The hostname field on the destination bucket must also be sanitized.
        let dest_bucket = dest_buckets.get(sanitized_id).unwrap();
        assert_eq!(
            dest_bucket.hostname, "poco_f8_ultra",
            "destination hostname must match Android's sanitizer"
        );
        // $aw.sync.origin keeps the raw hostname so the pre-migration phone
        // identity is still recoverable.
        assert_eq!(
            dest_bucket
                .data
                .get("$aw.sync.origin")
                .and_then(|v| v.as_str()),
            Some("POCO F8 Ultra")
        );
    }

    /// If a legacy unsanitized bucket already exists in the destination (imported
    /// before the sanitization was added), re-use it instead of creating a new
    /// sanitized one.  Creating a sanitized copy forks the destination and causes
    /// a full re-import (ActivityWatch/activitywatch#1373).
    #[test]
    fn test_whitespace_hostname_pull_reuses_legacy_unsanitized_bucket() {
        let state = init_teststate();

        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "aw-watcher-android",
            "type": "currentwindow",
            "hostname": "POCO F8 Ultra",
            "client": "aw-android"
        }))
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        // Simulate a pre-existing legacy destination bucket with unsanitized ID.
        let legacy_id = "aw-watcher-android-synced-from-POCO F8 Ultra";
        let legacy_bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": legacy_id,
            "type": "currentwindow",
            "hostname": "POCO F8 Ultra",
            "client": "aw-android"
        }))
        .unwrap();
        state.ds_dest.create_bucket(&legacy_bucket).unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false, // pull
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let dest_buckets = state.ds_dest.get_buckets().unwrap();

        // The legacy bucket must be present and re-used — not replaced.
        assert!(
            dest_buckets.contains_key(legacy_id),
            "legacy unsanitized bucket must be preserved"
        );
        // No new sanitized duplicate must have been created.
        let sanitized_id = "aw-watcher-android-synced-from-poco_f8_ultra";
        assert!(
            !dest_buckets.contains_key(sanitized_id),
            "a sanitized fork must not be created when legacy bucket exists, got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
    }

    /// A desktop that imported the peer **before** ActivityWatch/aw-server-rust#697
    /// landed holds `…-synced-from-POCO F8 Ultra` (raw, with `$aw.sync.origin` set
    /// to the raw value by #697's import stamp).  After ActivityWatch/aw-android#273
    /// migrates the phone's hostname to `poco_f8_ultra`, first-hand buckets carry no
    /// `$aw.sync.origin`, so the two direct lookups miss.  The pre-#697 fallback scan
    /// must find the legacy bucket and resume from it rather than creating a new one
    /// that triggers a full re-import.
    #[test]
    fn test_pre697_origin_scan_resumes_legacy_bucket() {
        let state = init_teststate();

        // Post-migration phone bucket: sanitized hostname, no $aw.sync.origin.
        let src_bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "aw-watcher-android",
            "type": "currentwindow",
            "hostname": "poco_f8_ultra",
            "client": "aw-android"
        }))
        .unwrap();
        state.ds_src.create_bucket(&src_bucket).unwrap();

        // Pre-#697 destination bucket: raw ID + $aw.sync.origin stamped by #697.
        let legacy_id = "aw-watcher-android-synced-from-POCO F8 Ultra";
        let legacy_bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": legacy_id,
            "type": "currentwindow",
            "hostname": "POCO F8 Ultra",
            "client": "aw-android",
            "data": {"$aw.sync.origin": "POCO F8 Ultra"}
        }))
        .unwrap();
        state.ds_dest.create_bucket(&legacy_bucket).unwrap();

        // Insert one event into the source so the sync pass has something to copy.
        let ts = chrono::Utc::now();
        let ev: Event = serde_json::from_value(serde_json::json!({
            "timestamp": ts.to_rfc3339(),
            "duration": 1,
            "data": {"app": "test"}
        }))
        .unwrap();
        state
            .ds_src
            .insert_events("aw-watcher-android", &[ev])
            .unwrap();
        state.ds_src.force_commit().unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false, // pull
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let dest_buckets = state.ds_dest.get_buckets().unwrap();

        // The legacy bucket must be reused, not replaced.
        assert!(
            dest_buckets.contains_key(legacy_id),
            "legacy bucket must be preserved"
        );
        // No new sanitized fork must appear.
        let forked_id = "aw-watcher-android-synced-from-poco_f8_ultra";
        assert!(
            !dest_buckets.contains_key(forked_id),
            "a sanitized fork must not be created; got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
        // Events were imported into the legacy bucket, not lost.
        let event_count = state
            .ds_dest
            .get_event_count(legacy_id, None, None)
            .unwrap();
        assert!(event_count > 0, "legacy bucket must have received events");
    }

    /// Two distinct pre-#697 buckets for the same base ID whose `$aw.sync.origin`
    /// values sanitize to the same target must trigger an error rather than a
    /// silent merge (ActivityWatch/aw-server-rust#697 :368).
    #[test]
    fn test_pre697_origin_scan_refuses_ambiguous_candidates() {
        let state = init_teststate();

        // Post-migration phone bucket.
        let src_bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "aw-watcher-android",
            "type": "currentwindow",
            "hostname": "poco_f8_ultra",
            "client": "aw-android"
        }))
        .unwrap();
        state.ds_src.create_bucket(&src_bucket).unwrap();

        // Two legacy destination buckets whose origins both sanitize to "poco_f8_ultra".
        for (legacy_id, raw_origin) in [
            (
                "aw-watcher-android-synced-from-POCO F8 Ultra",
                "POCO F8 Ultra",
            ),
            (
                "aw-watcher-android-synced-from-Poco F8 Ultra",
                "Poco F8 Ultra",
            ),
        ] {
            let b: Bucket = serde_json::from_value(serde_json::json!({
                "id": legacy_id,
                "type": "currentwindow",
                "hostname": raw_origin,
                "client": "aw-android",
                "data": {"$aw.sync.origin": raw_origin}
            }))
            .unwrap();
            state.ds_dest.create_bucket(&b).unwrap();
        }

        let result = aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false, // pull
            None,
            &SyncSpec::default(),
        );
        // The ambiguous-candidates path must fail rather than silently merge.
        let err = result.expect_err("ambiguous pre-#697 buckets must return Err");
        assert!(
            err.contains("pre-#697 buckets share"),
            "error should explain the ambiguity, got: {err}"
        );
    }

    /// Case-only hostnames (`PIXEL8`) have no whitespace, so a whitespace-only
    /// guard would leave the destination as `…-synced-from-PIXEL8`. Android's
    /// later hostname migration produces `pixel8` and forks the history.
    #[test]
    fn test_case_only_hostname_pull_creates_sanitized_bucket() {
        let state = init_teststate();

        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "aw-watcher-android",
            "type": "currentwindow",
            "hostname": "PIXEL8",
            "client": "aw-android"
        }))
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let dest_buckets = state.ds_dest.get_buckets().unwrap();
        let sanitized_id = "aw-watcher-android-synced-from-pixel8";
        assert!(
            dest_buckets.contains_key(sanitized_id),
            "expected sanitized bucket id '{sanitized_id}', got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
        assert!(
            !dest_buckets.contains_key("aw-watcher-android-synced-from-PIXEL8"),
            "case-only fork must not be created, got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
        assert_eq!(dest_buckets.get(sanitized_id).unwrap().hostname, "pixel8");
    }

    /// Dotted desktop hostnames (`erb-m2.localdomain`) sanitize punctuation to
    /// `_` for *new* imports. Existing raw IDs are still found via the raw lookup.
    #[test]
    fn test_dotted_hostname_pull_creates_sanitized_bucket() {
        let state = init_teststate();

        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "aw-watcher-window",
            "type": "currentwindow",
            "hostname": "erb-m2.localdomain",
            "client": "aw-watcher-window"
        }))
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let dest_buckets = state.ds_dest.get_buckets().unwrap();
        let sanitized_id = "aw-watcher-window-synced-from-erb-m2_localdomain";
        assert!(
            dest_buckets.contains_key(sanitized_id),
            "expected sanitized bucket id '{sanitized_id}', got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
        assert!(
            !dest_buckets.contains_key("aw-watcher-window-synced-from-erb-m2.localdomain"),
            "dotted raw id must not be created for new imports, got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
    }

    /// If a legacy case-only bucket already exists, re-use it rather than
    /// creating `…-synced-from-pixel8` beside `…-synced-from-PIXEL8`.
    #[test]
    fn test_case_only_hostname_pull_reuses_legacy_bucket() {
        let state = init_teststate();

        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "aw-watcher-android",
            "type": "currentwindow",
            "hostname": "PIXEL8",
            "client": "aw-android"
        }))
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        let legacy_id = "aw-watcher-android-synced-from-PIXEL8";
        let legacy_bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": legacy_id,
            "type": "currentwindow",
            "hostname": "PIXEL8",
            "client": "aw-android"
        }))
        .unwrap();
        state.ds_dest.create_bucket(&legacy_bucket).unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let dest_buckets = state.ds_dest.get_buckets().unwrap();
        assert!(
            dest_buckets.contains_key(legacy_id),
            "legacy case-only bucket must be preserved"
        );
        assert!(
            !dest_buckets.contains_key("aw-watcher-android-synced-from-pixel8"),
            "a sanitized fork must not be created when legacy bucket exists, got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
    }

    /// If `$aw.sync.origin` is already clean while `bucket.hostname` still has
    /// whitespace, the sanitizer must still run: otherwise `create_bucket` 400s
    /// on the hostname field even though the derived ID is legal.
    #[test]
    fn test_whitespace_hostname_sanitizes_even_when_id_is_clean() {
        let state = init_teststate();

        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": "aw-watcher-android",
            "type": "currentwindow",
            "hostname": "POCO F8 Ultra",
            "client": "aw-android",
            "data": {"$aw.sync.origin": "poco_f8_ultra"}
        }))
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false, // pull
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let dest_buckets = state.ds_dest.get_buckets().unwrap();
        let sanitized_id = "aw-watcher-android-synced-from-poco_f8_ultra";
        let dest_bucket = dest_buckets.get(sanitized_id).unwrap_or_else(|| {
            panic!(
                "expected sanitized bucket id '{sanitized_id}', got: {:?}",
                dest_buckets.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(dest_bucket.hostname, "poco_f8_ultra");
    }

    /// A hostname that contains whitespace but sanitizes to the "unknown"
    /// sentinel (e.g. `" * "`) must not create `-synced-from-unknown` on pull —
    /// that ID is shared by every such remote and would mix events. The bucket
    /// is skipped; a healthy sibling still syncs (per-bucket non-fatal).
    #[test]
    fn test_whitespace_hostname_that_sanitizes_to_unknown_is_skipped_on_pull() {
        let state = init_teststate();

        let junk: Bucket = serde_json::from_value(serde_json::json!({
            "id": "bucket-junk",
            "type": "test",
            "hostname": " * ",
            "client": "test"
        }))
        .unwrap();
        state.ds_src.create_bucket(&junk).unwrap();

        let healthy: Bucket = serde_json::from_value(serde_json::json!({
            "id": "bucket-healthy",
            "type": "test",
            "hostname": "device-0",
            "client": "test"
        }))
        .unwrap();
        state.ds_src.create_bucket(&healthy).unwrap();

        let result = aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false, // pull
            None,
            &SyncSpec::default(),
        );
        assert!(
            result.is_ok(),
            "junk hostname must skip that bucket, not abort the pass; got {result:?}"
        );

        let dest_buckets = state.ds_dest.get_buckets().unwrap();
        assert!(
            !dest_buckets
                .keys()
                .any(|k| k.contains("bucket-junk") || k.ends_with("-synced-from-unknown")),
            "must not create -synced-from-unknown, got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
        assert!(
            dest_buckets.contains_key("bucket-healthy-synced-from-device-0"),
            "healthy sibling must still sync, got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
    }

    /// Bucket metadata of an unexpected shape must not panic:
    /// `$aw.sync.origin` is read from data written by another host, so it is not
    /// under this host's control.
    ///
    /// With the per-bucket non-fatal change (#692), a malformed bucket is now
    /// skipped (warn + continue) rather than aborting the whole sync pass.
    #[test]
    fn test_non_string_sync_origin_does_not_panic() {
        let state = init_teststate();
        let bucket: Bucket = serde_json::from_str(
            r#"{
            "id": "bucket-weird",
            "type": "test",
            "hostname": "device-0",
            "client": "test",
            "data": {"$aw.sync.origin": 42}
        }"#,
        )
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        // Previously this panicked. A single malformed bucket is a total
        // failure of the pass, so it must return Err (not Ok after skip).
        // No panic is still the key invariant.
        let result = aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false, // pull: this is the path that reads $aw.sync.origin
            None,
            &SyncSpec::default(),
        );
        let err = result.expect_err("total failure of a one-bucket pass must be Err");
        assert!(
            err.contains("all 1 buckets failed"),
            "error should report total failure, got: {err}"
        );
        // The malformed bucket must have been skipped, not imported.
        let dest_buckets = state.ds_dest.get_buckets().unwrap();
        assert!(
            dest_buckets.is_empty(),
            "skipped bucket must not appear in destination, got: {:?}",
            dest_buckets.keys().collect::<Vec<_>>()
        );
    }

    /// A pulled bucket whose hostname is the "unknown" sentinel has no provenance,
    /// and pull passes no source device ID to substitute. Continuing would map
    /// every such bucket from every remote onto one `-synced-from-unknown`
    /// destination, mixing events from unrelated devices — so refuse the sync.
    /// (The old code unwrapped the `None` here, i.e. aborted the app on Android.)
    #[test]
    fn test_unknown_hostname_on_pull_returns_error() {
        let state = init_teststate();
        let bucket: Bucket = serde_json::from_str(
            r#"{
            "id": "bucket-unknown-host",
            "type": "test",
            "hostname": "unknown",
            "client": "test"
        }"#,
        )
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        let result = aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false, // pull: no source device ID is passed
            None,
            &SyncSpec::default(),
        );
        let err = result.expect_err("an unknown hostname on pull must return Err");
        assert!(
            err.contains("bucket-unknown-host"),
            "error should name the bucket, got: {err}"
        );

        // On push the source device ID is known, so the same bucket syncs fine.
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            true,
            Some("device-0"),
            &SyncSpec::default(),
        )
        .unwrap();
    }

    #[test]
    fn test_buckets_created() {
        // TODO: Split up this test
        let state = init_teststate();
        create_bucket(&state.ds_src, 0);

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let buckets_src: HashMap<String, Bucket> = state.ds_src.get_buckets().unwrap();
        let buckets_dest: HashMap<String, Bucket> = state.ds_dest.get_buckets().unwrap();
        assert!(buckets_src.len() == buckets_dest.len());
    }

    #[test]
    fn test_sync_datastores_returns_new_event_counts() {
        let state = init_teststate();
        let bucket_id = create_bucket(&state.ds_src, 0);
        create_events(&state.ds_src, &bucket_id, 3);

        let reports = aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].events_new, 3);
        assert!(
            reports[0].bucket_id.contains("-synced-from-"),
            "pull destination id should carry provenance, got {}",
            reports[0].bucket_id
        );

        let second = aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();
        assert_eq!(second[0].events_new, 0, "second pull must be a no-op");
    }

    /// On pull (is_push=false), the destination bucket should have $aw.sync.origin set to
    /// the source bucket's hostname.  On push (is_push=true), $aw.sync.origin must NOT be
    /// written to the staging copy.
    #[test]
    fn test_sync_origin_metadata() {
        let state = init_teststate();
        let bucket_id = create_bucket(&state.ds_src, 0);

        // --- push: staging copy must have no $aw.sync.origin ---
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            true, // is_push
            Some("device-0"),
            &SyncSpec::default(),
        )
        .unwrap();

        let push_buckets = state.ds_dest.get_buckets().unwrap();
        let push_bucket = push_buckets.get(&bucket_id).expect("push bucket not found");
        assert!(
            !push_bucket.data.contains_key("$aw.sync.origin"),
            "push-staging bucket must not have $aw.sync.origin, got: {:?}",
            push_bucket.data
        );

        // --- pull: imported bucket must have $aw.sync.origin set to the hostname ---
        let ds_pull_dest = Datastore::new_in_memory(false);
        aw_sync::sync_datastores(
            &state.ds_src,
            &ds_pull_dest,
            false, // is_push (pull)
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let pull_buckets = ds_pull_dest.get_buckets().unwrap();
        let pull_bucket_id = format!("{bucket_id}-synced-from-device-0");
        let pull_bucket = pull_buckets
            .get(&pull_bucket_id)
            .expect("pull bucket not found");
        let origin = pull_bucket
            .data
            .get("$aw.sync.origin")
            .expect("pull bucket must have $aw.sync.origin")
            .as_str()
            .expect("$aw.sync.origin must be a string");
        assert_eq!(
            origin, "device-0",
            "$aw.sync.origin should match source hostname"
        );
    }

    /// When bucket_from was previously pulled (carries $aw.sync.origin in its data), pushing it
    /// to staging must strip the stale field so downstream devices don't trust a wrong origin.
    #[test]
    fn test_push_strips_stale_sync_origin() {
        let state = init_teststate();

        // Create a source bucket that already has $aw.sync.origin (simulating a
        // previously-pulled bucket being pushed back to staging).
        let bucket_id = "bucket-with-origin".to_string();
        let bucket: Bucket = serde_json::from_value(serde_json::json!({
            "id": bucket_id,
            "type": "test",
            "hostname": "device-0",
            "client": "test",
            "data": { "$aw.sync.origin": "some-other-host" }
        }))
        .unwrap();
        state.ds_src.create_bucket(&bucket).unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            true, // is_push
            Some("device-0"),
            &SyncSpec::default(),
        )
        .unwrap();

        let push_buckets = state.ds_dest.get_buckets().unwrap();
        let push_bucket = push_buckets.get(&bucket_id).expect("push bucket not found");
        assert!(
            !push_bucket.data.contains_key("$aw.sync.origin"),
            "push-staging bucket must not carry stale $aw.sync.origin from a previously-pulled source, got: {:?}",
            push_bucket.data
        );
    }

    fn check_synced_buckets_equal_to_src(all_buckets_map: &HashMap<String, (&Datastore, Bucket)>) {
        for (ds, bucket) in all_buckets_map.values() {
            if bucket.id.contains("-synced") {
                let bucket_src_id = bucket.id.split("-synced-").next().unwrap();
                let (ds_src, bucket_src) = all_buckets_map.get(bucket_src_id).unwrap();
                let events_synced = ds.get_events(bucket.id.as_str(), None, None, None).unwrap();
                let events_src = ds_src
                    .get_events(bucket_src.id.as_str(), None, None, None)
                    .unwrap();
                println!("{events_synced:?}");
                println!("{events_src:?}");
                assert!(events_synced == events_src);
            }
        }
    }

    #[test]
    fn test_one_updated_event() {
        // This tests the syncing of one single event that is then updated by a heartbeat after the
        // first sync pass.
        let state = init_teststate();

        let bucket_id = create_bucket(&state.ds_src, 0);
        state
            .ds_src
            .heartbeat(bucket_id.as_str(), create_event("1"), 1.0)
            .unwrap();

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let all_datastores: Vec<&Datastore> = [&state.ds_src, &state.ds_dest].to_vec();
        let all_buckets_map = get_all_buckets_map(all_datastores);

        // Check that all synced buckets are identical to source bucket
        check_synced_buckets_equal_to_src(&all_buckets_map);

        // Add some more events
        state
            .ds_src
            .heartbeat(bucket_id.as_str(), create_event("1"), 1.0)
            .unwrap();
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        // Check again that new events were indeed synced
        check_synced_buckets_equal_to_src(&all_buckets_map);
    }

    #[test]
    fn test_events() {
        let state = init_teststate();

        let bucket_id = create_bucket(&state.ds_src, 0);
        create_events(&state.ds_src, bucket_id.as_str(), 10);

        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let all_datastores: Vec<&Datastore> = [&state.ds_src, &state.ds_dest].to_vec();
        let all_buckets_map = get_all_buckets_map(all_datastores);

        // Check that all synced buckets are identical to source bucket
        check_synced_buckets_equal_to_src(&all_buckets_map);

        // Add some more events
        create_events(&state.ds_src, bucket_id.as_str(), 10);
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        // Check again that new events were indeed synced
        check_synced_buckets_equal_to_src(&all_buckets_map);
    }

    #[test]
    fn test_sync_resume_after_partial_sync() {
        // Verify that resuming an interrupted sync picks up where it left off.
        // This exercises the resume_sync_at path in the chunked-fetch loop.
        let state = init_teststate();

        let bucket_id = create_bucket(&state.ds_src, 0);
        create_events(&state.ds_src, bucket_id.as_str(), 15);

        // First sync pass
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let all_datastores_1: Vec<&Datastore> = [&state.ds_src, &state.ds_dest].to_vec();
        check_synced_buckets_equal_to_src(&get_all_buckets_map(all_datastores_1));

        // Add more events and sync again — verifies resume logic
        create_events(&state.ds_src, bucket_id.as_str(), 15);
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let all_datastores_2: Vec<&Datastore> = [&state.ds_src, &state.ds_dest].to_vec();
        check_synced_buckets_equal_to_src(&get_all_buckets_map(all_datastores_2));
    }

    #[test]
    fn test_sync_multipage_no_duplicates() {
        // Verify that multi-page syncs (more events than BATCH_SIZE) produce no duplicates.
        // In tests, BATCH_SIZE=5, so 12 events triggers 3 pages (5+5+2).
        // This exercises the boundary-event cursor logic that previously left one copy of
        // the page-boundary event in the current chunk AND re-fetched it on the next page.
        let state = init_teststate();

        let bucket_id = create_bucket(&state.ds_src, 0);
        create_events(&state.ds_src, bucket_id.as_str(), 12);

        // Initial sync
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let all_datastores_1: Vec<&Datastore> = [&state.ds_src, &state.ds_dest].to_vec();
        check_synced_buckets_equal_to_src(&get_all_buckets_map(all_datastores_1));

        // Resume sync: add another 12 events (3 more pages) and sync again.
        // Exercises multi-page resume path where heartbeat order matters.
        create_events(&state.ds_src, bucket_id.as_str(), 12);
        aw_sync::sync_datastores(
            &state.ds_src,
            &state.ds_dest,
            false,
            None,
            &SyncSpec::default(),
        )
        .unwrap();

        let all_datastores_2: Vec<&Datastore> = [&state.ds_src, &state.ds_dest].to_vec();
        check_synced_buckets_equal_to_src(&get_all_buckets_map(all_datastores_2));
    }

    // TODO: Find a way to reuse this (previously used in an integration test)
    fn setup_test(sync_directory: &Path) -> std::io::Result<Vec<Datastore>> {
        let mut datastores: Vec<Datastore> = Vec::new();
        for n in 0..2 {
            let dspath = sync_directory.join(format!("test-remote-{n}.db"));
            let ds_ = create_datastore(&dspath).expect("test db path is valid UTF-8");
            let ds = &ds_ as &dyn AccessMethod;

            // Create a bucket
            // NOTE: Created with duplicate name to make sure it still works under such conditions
            let bucket_jsonstr = format!(
                r#"{{
                    "id": "bucket",
                    "type": "test",
                    "hostname": "device-{n}",
                    "client": "test"
                }}"#
            );
            let bucket: Bucket = serde_json::from_str(&bucket_jsonstr)?;
            match ds.create_bucket(&bucket) {
                Ok(()) => (),
                Err(e) => match e {
                    DatastoreError::BucketAlreadyExists(_) => {
                        debug!("bucket already exists, skipping");
                    }
                    e => panic!("woops! {e:?}"),
                },
            };

            // Insert some testing events into the bucket
            let events: Vec<Event> = (0..3)
                .map(|i| {
                    let timestamp: DateTime<Utc> = Utc::now() + Duration::milliseconds(i * 10);
                    let event_jsonstr = format!(
                        r#"{{
                            "timestamp": "{}",
                            "duration": 0,
                            "data": {{"test": {} }}
                        }}"#,
                        timestamp.to_rfc3339(),
                        i
                    );
                    serde_json::from_str(&event_jsonstr).unwrap()
                })
                .collect::<Vec<Event>>();

            ds.insert_events(bucket.id.as_str(), events).unwrap();
            //let new_eventcount = ds.get_event_count(bucket.id.as_str(), None, None).unwrap();
            //info!("Eventcount: {:?} ({} new)", new_eventcount, events.len());
            datastores.push(ds_);
        }
        Ok(datastores)
    }
}
