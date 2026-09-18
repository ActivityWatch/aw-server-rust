//! Segment writer for aw-sync v2.
//!
//! Produces immutable JSONL+zstd segment files. Write pattern:
//!   1. Open a temp file (.{name}.tmp)
//!   2. Write zstd-compressed JSONL (header + events)
//!   3. fsync + atomic rename to the final path
//!   4. Update the manifest
//!
//! An open-tail segment (below the size threshold) is rewritten in-place each
//! daemon pass using the same tmp+rename sequence, so a reader always sees a
//! complete file.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use aw_models::{Bucket, Event};

use super::manifest::{bucket_slug, device_dir, BucketEntry, Manifest, SegmentEntry};

/// Minimum compressed segment size before sealing (1 MiB).
const SEAL_SIZE_BYTES: u64 = 1024 * 1024;

/// Maximum age of an open-tail generation before it is sealed regardless of
/// size, measured from the generation's first `start_ts`. Ensures a
/// low-volume bucket doesn't rewrite the same tail forever.
fn seal_max_age() -> chrono::Duration {
    chrono::Duration::days(1)
}

/// (n_events, start_ts, end_ts, sha256_hex, compressed_size)
type SegmentWriteResult = (
    u64,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    String,
    u64,
);

/// Segment header (line 1 of each segment JSONL).
#[derive(Debug, Serialize, Deserialize)]
struct SegmentHeader {
    v: u32,
    device_id: String,
    bucket_id: String,
    bucket_type: String,
    bucket_client: String,
    bucket_hostname: String,
    bucket_created: Option<DateTime<Utc>>,
    generation: u64,
    written_at: DateTime<Utc>,
    /// `null` for append-only; `{from, to}` for a range-replace segment.
    replaces: Option<ReplaceRange>,
}

/// Range-replace marker — reserved for future use; writer always emits null.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReplaceRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// Wire event format matching the aw-core REST API response.
#[derive(Debug, Serialize)]
struct WireEvent<'a> {
    id: Option<i64>,
    timestamp: String,
    duration: f64,
    data: &'a serde_json::Map<String, serde_json::Value>,
}

/// Writes v2 segment files for a single bucket.
pub struct SegmentWriter {
    device_id: String,
    bucket_id: String,
    sync_dir: PathBuf,
    generation: u64,
    slug: String,
}

impl SegmentWriter {
    /// Load or initialize the writer for a bucket, restoring the generation
    /// counter from the existing manifest if present.
    pub fn new(sync_dir: &Path, device_id: &str, bucket_id: &str) -> Result<Self, String> {
        let hostname = gethostname::gethostname()
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());
        let manifest = Manifest::load_or_default(sync_dir, device_id, &hostname)?;
        let generation = manifest
            .buckets
            .get(bucket_id)
            .map(|e| e.latest_generation)
            .unwrap_or(0);
        Ok(SegmentWriter {
            device_id: device_id.to_string(),
            bucket_id: bucket_id.to_string(),
            sync_dir: sync_dir.to_path_buf(),
            generation,
            slug: bucket_slug(bucket_id),
        })
    }

    /// Write events to a segment and update the manifest.
    ///
    /// If the previous generation's segment is unsealed (< SEAL_SIZE_BYTES
    /// and younger than `SEAL_MAX_AGE`), it is rewritten under the same
    /// generation number — merged with whatever events that segment already
    /// held, so a caller passing only newly-available events never drops the
    /// existing tail. Otherwise a new generation is started.
    ///
    /// Returns the generation number written.
    pub fn write_events(&mut self, bucket: &Bucket, events: &[Event]) -> Result<u64, String> {
        if events.is_empty() {
            return Ok(self.generation);
        }

        let device_dir = device_dir(&self.sync_dir, &self.device_id);
        fs::create_dir_all(&device_dir).map_err(|e| format!("create device dir: {e}"))?;

        // Load once; reused both for the seal decision and the final update.
        let hostname = gethostname::gethostname()
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());
        let mut manifest = Manifest::load_or_default(&self.sync_dir, &self.device_id, &hostname)?;

        // Determine whether to seal the previous segment and start a new one.
        let should_start_new = if self.generation == 0 {
            true // First ever segment
        } else {
            let prev_path = self.segment_path(self.generation);
            if prev_path.exists() {
                let size = fs::metadata(&prev_path).map(|m| m.len()).unwrap_or(0);
                let prev_start = manifest
                    .buckets
                    .get(&self.bucket_id)
                    .and_then(|e| e.segments.iter().find(|s| s.generation == self.generation))
                    .and_then(|s| s.start_ts);
                let too_old = prev_start
                    .map(|start| Utc::now() - start >= seal_max_age())
                    .unwrap_or(false);
                size >= SEAL_SIZE_BYTES || too_old
            } else {
                true
            }
        };

        let write_gen = if should_start_new && self.generation > 0 {
            self.generation + 1
        } else if should_start_new {
            1
        } else {
            self.generation
        };

        // Seal previous generation if we're moving forward
        let prev_sealed = should_start_new && self.generation > 0;

        // When rewriting the open tail, merge with whatever it already holds
        // so a caller passing only the events new since the last pass can't
        // make earlier events vanish from the sync folder.
        let merged_events;
        let events_to_write: &[Event] = if !should_start_new {
            let prev_path = self.segment_path(write_gen);
            let existing = Self::read_segment_events(&prev_path)?;
            merged_events = merge_events(existing, events);
            &merged_events
        } else {
            events
        };

        // Write the segment
        let (n_events, start_ts, end_ts, sha256_hex, compressed_size) =
            self.write_segment_file(bucket, events_to_write, write_gen)?;

        let sealed = compressed_size >= SEAL_SIZE_BYTES;

        let slug = self.slug.clone();
        let existing = manifest.buckets.remove(&self.bucket_id);
        let mut segments: Vec<SegmentEntry> = existing
            .as_ref()
            .map(|e| e.segments.clone())
            .unwrap_or_default();

        // If sealing the previous generation, mark it sealed in the manifest
        if prev_sealed {
            if let Some(s) = segments
                .iter_mut()
                .find(|s| s.generation == self.generation)
            {
                // Recompute sha256 for the now-final file
                let prev_path = self.segment_path(self.generation);
                if let Ok(bytes) = fs::read(&prev_path) {
                    let mut h = Sha256::new();
                    h.update(&bytes);
                    s.sha256 = format!("{:x}", h.finalize());
                }
                s.sealed = true;
            }
        }

        // Remove any existing entry for this generation (open-tail rewrite)
        segments.retain(|s| s.generation != write_gen);
        segments.push(SegmentEntry {
            generation: write_gen,
            file: self.segment_filename(write_gen),
            n_events,
            start_ts,
            end_ts,
            sha256: sha256_hex,
            sealed,
        });
        segments.sort_by_key(|s| s.generation);

        let total_events = segments.iter().map(|s| s.n_events).sum();
        let bucket_entry = BucketEntry {
            slug,
            _type: bucket._type.clone(),
            client: bucket.client.clone(),
            hostname: bucket.hostname.clone(),
            created: bucket.created,
            latest_generation: write_gen,
            total_events,
            segments,
        };
        manifest.upsert_bucket(&self.bucket_id, bucket_entry);
        manifest.written_at = Utc::now();
        manifest.save(&self.sync_dir, &self.device_id)?;

        self.generation = write_gen;
        Ok(write_gen)
    }

    /// Write the compressed JSONL segment file atomically.
    /// Returns (n_events, start_ts, end_ts, sha256_hex, compressed_size).
    fn write_segment_file(
        &self,
        bucket: &Bucket,
        events: &[Event],
        generation: u64,
    ) -> Result<SegmentWriteResult, String> {
        let segment_path = self.segment_path(generation);
        let tmp_path = segment_path
            .parent()
            .unwrap()
            .join(format!(".{}.tmp", self.segment_filename(generation)));

        // Build JSONL in memory, then compress
        let mut jsonl = Vec::new();

        // Line 1: header
        let header = SegmentHeader {
            v: 1,
            device_id: self.device_id.clone(),
            bucket_id: self.bucket_id.clone(),
            bucket_type: bucket._type.clone(),
            bucket_client: bucket.client.clone(),
            bucket_hostname: bucket.hostname.clone(),
            bucket_created: bucket.created,
            generation,
            written_at: Utc::now(),
            replaces: None,
        };
        serde_json::to_writer(&mut jsonl, &header).map_err(|e| format!("serialize header: {e}"))?;
        jsonl.push(b'\n');

        // Lines 2+: events in chronological order. Sort here rather than
        // trusting the caller — the datastore's own retrieval order is
        // newest-first, so passing that straight through would silently
        // violate the segment format's ordering contract.
        let mut sorted_events: Vec<&Event> = events.iter().collect();
        sorted_events.sort_by_key(|e| e.timestamp);

        let mut start_ts: Option<DateTime<Utc>> = None;
        let mut end_ts: Option<DateTime<Utc>> = None;
        let mut n_events: u64 = 0;

        for event in sorted_events {
            let ts_str = event
                .timestamp
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
            let duration_secs =
                event.duration.num_nanoseconds().unwrap_or(0) as f64 / 1_000_000_000.0;
            let wire = WireEvent {
                id: event.id,
                timestamp: ts_str,
                duration: duration_secs,
                data: &event.data,
            };
            serde_json::to_writer(&mut jsonl, &wire)
                .map_err(|e| format!("serialize event: {e}"))?;
            jsonl.push(b'\n');

            if start_ts.is_none() || event.timestamp < start_ts.unwrap() {
                start_ts = Some(event.timestamp);
            }
            let end = event.timestamp + event.duration;
            if end_ts.is_none() || end > end_ts.unwrap() {
                end_ts = Some(end);
            }
            n_events += 1;
        }

        // Compress with zstd
        let compressed =
            zstd::encode_all(jsonl.as_slice(), 0).map_err(|e| format!("zstd compress: {e}"))?;

        let compressed_size = compressed.len() as u64;

        // SHA-256 of compressed bytes
        let mut hasher = Sha256::new();
        hasher.update(&compressed);
        let sha256_hex = format!("{:x}", hasher.finalize());

        // Atomic write
        {
            let f = fs::File::create(&tmp_path).map_err(|e| format!("create segment tmp: {e}"))?;
            let mut w = BufWriter::new(&f);
            w.write_all(&compressed)
                .map_err(|e| format!("write segment: {e}"))?;
            w.flush().map_err(|e| format!("flush segment: {e}"))?;
            f.sync_all().map_err(|e| format!("fsync segment: {e}"))?;
        }
        fs::rename(&tmp_path, &segment_path).map_err(|e| format!("rename segment: {e}"))?;
        super::manifest::fsync_dir(segment_path.parent().unwrap())?;

        Ok((n_events, start_ts, end_ts, sha256_hex, compressed_size))
    }

    /// Read and decode an existing segment's events (skipping the header
    /// line). Returns an empty vec if the segment doesn't exist yet.
    fn read_segment_events(path: &Path) -> Result<Vec<Event>, String> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let compressed = fs::read(path).map_err(|e| format!("read segment: {e}"))?;
        let decompressed = zstd::decode_all(compressed.as_slice())
            .map_err(|e| format!("zstd decompress segment: {e}"))?;
        let text = String::from_utf8(decompressed).map_err(|e| format!("segment utf8: {e}"))?;
        text.lines()
            .skip(1) // header
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_str::<Event>(line).map_err(|e| format!("parse event line: {e}"))
            })
            .collect()
    }

    fn segment_filename(&self, generation: u64) -> String {
        format!("{}.{:08}.jsonl.zst", self.slug, generation)
    }

    fn segment_path(&self, generation: u64) -> PathBuf {
        device_dir(&self.sync_dir, &self.device_id).join(self.segment_filename(generation))
    }
}

/// Union of `existing` and `new`, deduped by event id (later entries win),
/// events without an id kept as-is, sorted chronologically.
fn merge_events(existing: Vec<Event>, new: &[Event]) -> Vec<Event> {
    let mut by_id: BTreeMap<i64, Event> = BTreeMap::new();
    let mut unkeyed: Vec<Event> = Vec::new();

    for event in existing.into_iter().chain(new.iter().cloned()) {
        match event.id {
            Some(id) => {
                by_id.insert(id, event);
            }
            None => unkeyed.push(event),
        }
    }

    let mut merged: Vec<Event> = by_id.into_values().collect();
    merged.extend(unkeyed);
    merged.sort_by_key(|e| e.timestamp);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use aw_models::Event;
    use chrono::Duration;
    use serde_json::json;

    fn make_event(ts_offset_s: i64, id: i64) -> Event {
        let ts = chrono::Utc::now() + Duration::seconds(ts_offset_s);
        let mut data = serde_json::Map::new();
        data.insert("app".to_string(), json!("TestApp"));
        Event {
            id: Some(id),
            timestamp: ts,
            duration: Duration::seconds(30),
            data,
        }
    }

    fn make_bucket() -> Bucket {
        Bucket {
            bid: None,
            id: "aw-watcher-window_test-host".to_string(),
            _type: "currentwindow".to_string(),
            client: "aw-watcher-window".to_string(),
            hostname: "test-host".to_string(),
            created: Some(Utc::now()),
            data: serde_json::Map::new(),
            metadata: Default::default(),
            events: None,
            last_updated: None,
        }
    }

    #[test]
    fn test_segment_write_produces_valid_zstd() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "test-host_abc123";
        let bucket = make_bucket();
        let events = vec![make_event(0, 1), make_event(30, 2)];

        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();
        let gen = writer.write_events(&bucket, &events).unwrap();
        assert_eq!(gen, 1);

        // File must exist and be decompressible
        let seg_path = dir
            .path()
            .join("devices")
            .join(device_id)
            .join(format!("{}.00000001.jsonl.zst", bucket_slug(&bucket.id)));
        assert!(seg_path.exists(), "segment file not found: {seg_path:?}");

        let compressed = fs::read(&seg_path).unwrap();
        let decompressed = zstd::decode_all(compressed.as_slice()).unwrap();
        let text = String::from_utf8(decompressed).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "header + 2 events");

        // Line 1 is the header
        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["v"], 1);
        assert_eq!(header["bucket_id"], "aw-watcher-window_test-host");
        assert_eq!(header["generation"], 1);
        assert!(header["replaces"].is_null());
    }

    #[test]
    fn test_segment_immutability_after_seal() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "test-host_seal";
        let bucket = make_bucket();

        // Write enough data to cross the seal threshold
        // We use a compressed-size trick: write a first small segment, manually
        // mark it as >= SEAL_SIZE_BYTES by creating a large fake file.
        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();
        let events_a = vec![make_event(0, 1)];
        let gen_a = writer.write_events(&bucket, &events_a).unwrap();
        assert_eq!(gen_a, 1);

        // Read first segment bytes before the second write
        let seg_path_1 = dir
            .path()
            .join("devices")
            .join(device_id)
            .join(format!("{}.00000001.jsonl.zst", bucket_slug(&bucket.id)));
        let bytes_before = fs::read(&seg_path_1).unwrap();

        // Force the first segment to be "large" so the writer seals it
        // by replacing it with a file of >= SEAL_SIZE_BYTES
        let fake_large = vec![0u8; SEAL_SIZE_BYTES as usize];
        fs::write(&seg_path_1, &fake_large).unwrap();

        // Second write — should create generation 2, not overwrite generation 1
        let events_b = vec![make_event(60, 2)];
        let gen_b = writer.write_events(&bucket, &events_b).unwrap();
        assert_eq!(gen_b, 2, "second write should start a new generation");

        // Generation 1 must NOT have been touched (it was large/sealed)
        let bytes_after = fs::read(&seg_path_1).unwrap();
        assert_eq!(
            bytes_after.len(),
            fake_large.len(),
            "sealed segment must not be overwritten"
        );
        // The original small content must not have come back
        assert_ne!(
            bytes_after, bytes_before,
            "sanity: the large file we wrote is still there"
        );

        // Generation 2 must exist
        let seg_path_2 = dir
            .path()
            .join("devices")
            .join(device_id)
            .join(format!("{}.00000002.jsonl.zst", bucket_slug(&bucket.id)));
        assert!(seg_path_2.exists(), "generation 2 segment not found");
    }

    #[test]
    fn test_manifest_reflects_two_segments() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "test-host_manifest";
        let bucket = make_bucket();
        let hostname = gethostname::gethostname()
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());

        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();

        // Write first segment (small, will stay unsealed)
        writer.write_events(&bucket, &[make_event(0, 1)]).unwrap();

        // Force segment 1 to be large so segment 2 starts
        let seg1_path = dir
            .path()
            .join("devices")
            .join(device_id)
            .join(format!("{}.00000001.jsonl.zst", bucket_slug(&bucket.id)));
        fs::write(&seg1_path, vec![0u8; SEAL_SIZE_BYTES as usize]).unwrap();

        // Write second segment
        writer.write_events(&bucket, &[make_event(60, 2)]).unwrap();

        // Manifest must reflect both segments
        let manifest = Manifest::load_or_default(dir.path(), device_id, &hostname).unwrap();
        let entry = manifest
            .buckets
            .get(&bucket.id)
            .expect("bucket in manifest");
        assert_eq!(entry.latest_generation, 2);
        assert_eq!(entry.segments.len(), 2, "both segments must be listed");
        assert_eq!(entry.segments[0].generation, 1);
        assert_eq!(entry.segments[1].generation, 2);
        // Generation 1 is now sealed (was made large)
        assert!(entry.segments[0].sealed, "gen 1 should be sealed");
        // Generation 2 is small, unsealed
        assert!(
            !entry.segments[1].sealed,
            "gen 2 should be unsealed (small)"
        );
    }

    #[test]
    fn test_no_events_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "test-host_noop";
        let bucket = make_bucket();

        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();
        let gen = writer.write_events(&bucket, &[]).unwrap();
        // Should not advance generation or create any files
        assert_eq!(gen, 0);
        let device_path = dir.path().join("devices").join(device_id);
        assert!(!device_path.exists() || fs::read_dir(&device_path).unwrap().count() == 0);
    }

    #[test]
    fn test_open_tail_rewrite_preserves_earlier_events() {
        // Regression test for the open-tail data loss found in review: a
        // daemon pass that supplies only the events new since the last pass
        // must not make the earlier events of that generation disappear.
        let dir = tempfile::tempdir().unwrap();
        let device_id = "test-host_tail_merge";
        let bucket = make_bucket();
        let hostname = gethostname::gethostname()
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());

        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();

        // Pass 1: write event A.
        let events_a = vec![make_event(0, 1)];
        let gen_a = writer.write_events(&bucket, &events_a).unwrap();
        assert_eq!(gen_a, 1);

        // Pass 2: caller only passes event B (new since last pass) — the
        // segment stays well under SEAL_SIZE_BYTES, so this rewrites gen 1.
        let events_b = vec![make_event(60, 2)];
        let gen_b = writer.write_events(&bucket, &events_b).unwrap();
        assert_eq!(gen_b, 1, "still the open tail, not a new generation");

        // The decoded tail must contain A ∪ B, not just B.
        let seg_path = dir
            .path()
            .join("devices")
            .join(device_id)
            .join(format!("{}.00000001.jsonl.zst", bucket_slug(&bucket.id)));
        let compressed = fs::read(&seg_path).unwrap();
        let decompressed = zstd::decode_all(compressed.as_slice()).unwrap();
        let text = String::from_utf8(decompressed).unwrap();
        let ids: Vec<i64> = text
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str::<Event>(line).unwrap().id.unwrap())
            .collect();
        assert_eq!(ids, vec![1, 2], "decoded tail must contain A union B");

        // The manifest's n_events must reflect the union too.
        let manifest = Manifest::load_or_default(dir.path(), device_id, &hostname).unwrap();
        let entry = manifest.buckets.get(&bucket.id).unwrap();
        assert_eq!(
            entry.total_events, 2,
            "manifest n_events must be |A union B|"
        );
        assert_eq!(entry.segments.len(), 1);
        assert_eq!(entry.segments[0].n_events, 2);
    }

    #[test]
    fn test_segments_written_chronologically_regardless_of_input_order() {
        // Regression test: the datastore's natural retrieval order is
        // newest-first. The writer must not trust caller order.
        let dir = tempfile::tempdir().unwrap();
        let device_id = "test-host_chrono";
        let bucket = make_bucket();

        // Pass events in reverse-chronological order (newest first).
        let events = vec![make_event(60, 2), make_event(0, 1)];

        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();
        writer.write_events(&bucket, &events).unwrap();

        let seg_path = dir
            .path()
            .join("devices")
            .join(device_id)
            .join(format!("{}.00000001.jsonl.zst", bucket_slug(&bucket.id)));
        let compressed = fs::read(&seg_path).unwrap();
        let decompressed = zstd::decode_all(compressed.as_slice()).unwrap();
        let text = String::from_utf8(decompressed).unwrap();
        let ids: Vec<i64> = text
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str::<Event>(line).unwrap().id.unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![1, 2],
            "segment lines must be chronological, oldest first"
        );
    }
}
