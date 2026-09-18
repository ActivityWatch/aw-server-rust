//! Manifest format for aw-sync v2.
//!
//! The manifest is the entry point for importers. It maps bucket ids to their
//! segment list and metadata without requiring importers to open any segment file.

use std::collections::HashMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::MAX_V;

/// Top-level manifest file (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Format version. Importers must refuse if v > MAX_V.
    pub v: u32,
    pub device_id: String,
    pub hostname: String,
    /// ISO-8601 timestamp of the last manifest write.
    pub written_at: DateTime<Utc>,
    /// Keyed by real bucket_id.
    pub buckets: HashMap<String, BucketEntry>,
}

/// Per-bucket metadata and segment list inside the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketEntry {
    /// First 16 hex chars of sha256(bucket_id). Used in filenames.
    pub slug: String,
    #[serde(rename = "type")]
    pub _type: String,
    pub client: String,
    pub hostname: String,
    pub created: Option<DateTime<Utc>>,
    pub latest_generation: u64,
    pub total_events: u64,
    pub segments: Vec<SegmentEntry>,
}

/// One segment's metadata inside a BucketEntry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentEntry {
    pub generation: u64,
    pub file: String,
    pub n_events: u64,
    pub start_ts: Option<DateTime<Utc>>,
    pub end_ts: Option<DateTime<Utc>>,
    /// SHA-256 hex of the compressed segment bytes.
    /// Only authoritative when `sealed == true`.
    pub sha256: String,
    /// True once the segment will not be rewritten. An unsealed segment is the
    /// current open tail; its sha256 changes on every writer pass.
    pub sealed: bool,
    /// Wall-clock time this generation was first written. Unlike `start_ts`
    /// (the oldest *event* timestamp), this never changes across rewrites of
    /// the same open tail — it drives the age-based seal policy so importing
    /// old historical events doesn't look "already expired" on the first pass.
    #[serde(default)]
    pub first_written_at: Option<DateTime<Utc>>,
}

impl Manifest {
    /// Load an existing manifest or return a fresh default.
    pub fn load_or_default(dir: &Path, device_id: &str, hostname: &str) -> Result<Self, String> {
        validate_device_id(device_id)?;
        let path = manifest_path(dir, device_id);
        if path.exists() {
            let data = fs::read_to_string(&path).map_err(|e| format!("read manifest: {e}"))?;
            let m: Manifest =
                serde_json::from_str(&data).map_err(|e| format!("parse manifest: {e}"))?;
            if m.v > MAX_V {
                return Err(format!(
                    "manifest v{} > MAX_V {MAX_V} — refusing to open",
                    m.v
                ));
            }
            Ok(m)
        } else {
            Ok(Manifest {
                v: 1,
                device_id: device_id.to_string(),
                hostname: hostname.to_string(),
                written_at: Utc::now(),
                buckets: HashMap::new(),
            })
        }
    }

    /// Upsert or replace the entry for a bucket.
    pub fn upsert_bucket(&mut self, bucket_id: &str, entry: BucketEntry) {
        self.buckets.insert(bucket_id.to_string(), entry);
    }

    /// Write atomically: temp file → fsync → rename.
    pub fn save(&self, dir: &Path, device_id: &str) -> Result<(), String> {
        let target = manifest_path(dir, device_id);
        fs::create_dir_all(target.parent().unwrap())
            .map_err(|e| format!("create manifest dir: {e}"))?;

        let tmp = target.with_extension("json.tmp");
        {
            let f = fs::File::create(&tmp).map_err(|e| format!("create manifest tmp: {e}"))?;
            let mut w = BufWriter::new(&f);
            let json = serde_json::to_string_pretty(self)
                .map_err(|e| format!("serialize manifest: {e}"))?;
            w.write_all(json.as_bytes())
                .map_err(|e| format!("write manifest: {e}"))?;
            w.flush().map_err(|e| format!("flush manifest: {e}"))?;
            f.sync_all().map_err(|e| format!("fsync manifest: {e}"))?;
        }
        durable_rename(&tmp, &target)?;
        fsync_dir(target.parent().unwrap())?;
        Ok(())
    }
}

/// fsync a directory so a preceding `rename` into it survives a crash.
///
/// No-op on Windows: directory handles opened via `File::open` cannot be
/// fsynced on that platform. Durability for the rename itself is instead
/// provided by `durable_rename`, which uses `MOVEFILE_WRITE_THROUGH` there.
pub(crate) fn fsync_dir(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        fs::File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| format!("fsync dir {}: {e}", dir.display()))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Rename `from` to `to`, replacing any existing file at `to`, with the same
/// crash-durability guarantee on every supported platform.
///
/// On Unix, a plain `rename` is already durable once the directory entry is
/// fsynced (`fsync_dir`, called by the caller). Windows has no directory-fsync
/// primitive, so the equivalent guarantee there is `MOVEFILE_WRITE_THROUGH`:
/// `MoveFileExW` does not return until the rename is flushed to disk.
pub(crate) fn durable_rename(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };

        let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: both buffers are valid, NUL-terminated UTF-16 strings that
        // outlive the call.
        let ok = unsafe {
            MoveFileExW(
                from_wide.as_ptr(),
                to_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ok == 0 {
            return Err(format!(
                "rename {} -> {}: {}",
                from.display(),
                to.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to)
            .map_err(|e| format!("rename {} -> {}: {e}", from.display(), to.display()))
    }
}

/// Path for manifest inside the device directory.
pub fn manifest_path(sync_root: &Path, device_id: &str) -> PathBuf {
    device_dir(sync_root, device_id).join("manifest.json")
}

/// `<sync_root>/devices/<device_id>`
pub fn device_dir(sync_root: &Path, device_id: &str) -> PathBuf {
    sync_root.join("devices").join(device_id)
}

/// Reject a device_id that could escape the sync root via path traversal.
pub(super) fn validate_device_id(device_id: &str) -> Result<(), String> {
    if device_id.is_empty() {
        return Err("device_id must not be empty".to_string());
    }
    if device_id.contains('/') || device_id.contains('\\') || device_id.contains("..") {
        return Err(format!(
            "invalid device_id {device_id:?}: must not contain path separators or '..'"
        ));
    }
    Ok(())
}

/// Compute the 16-char slug used in segment filenames.
pub fn bucket_slug(bucket_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bucket_id.as_bytes());
    let result = h.finalize();
    // First 8 bytes = 16 hex chars
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        result[0], result[1], result[2], result[3], result[4], result[5], result[6], result[7]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bucket_slug_length() {
        let slug = bucket_slug("aw-watcher-window_my-desktop");
        assert_eq!(slug.len(), 16, "slug must be exactly 16 hex chars");
    }

    #[test]
    fn test_bucket_slug_stable() {
        // Same input must always produce the same slug.
        let s1 = bucket_slug("aw-watcher-window_test");
        let s2 = bucket_slug("aw-watcher-window_test");
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_bucket_slug_distinct() {
        let s1 = bucket_slug("aw-watcher-window_host-a");
        let s2 = bucket_slug("aw-watcher-afk_host-a");
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_manifest_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "test-host_abc123";
        let hostname = "test-host";

        let mut m = Manifest::load_or_default(dir.path(), device_id, hostname).unwrap();
        assert_eq!(m.v, 1);
        assert!(m.buckets.is_empty());

        // Upsert a bucket entry
        m.upsert_bucket(
            "aw-watcher-window_test-host",
            BucketEntry {
                slug: bucket_slug("aw-watcher-window_test-host"),
                _type: "currentwindow".to_string(),
                client: "aw-watcher-window".to_string(),
                hostname: hostname.to_string(),
                created: None,
                latest_generation: 1,
                total_events: 42,
                segments: vec![SegmentEntry {
                    generation: 1,
                    file: "a1b2c3d4e5f60718.00000001.jsonl.zst".to_string(),
                    n_events: 42,
                    start_ts: None,
                    end_ts: None,
                    sha256: "abc".to_string(),
                    sealed: false,
                    first_written_at: Some(Utc::now()),
                }],
            },
        );
        m.save(dir.path(), device_id).unwrap();

        // Reload and verify
        let m2 = Manifest::load_or_default(dir.path(), device_id, hostname).unwrap();
        assert_eq!(m2.buckets.len(), 1);
        let entry = m2.buckets.get("aw-watcher-window_test-host").unwrap();
        assert_eq!(entry.latest_generation, 1);
        assert_eq!(entry.total_events, 42);
        assert_eq!(entry.segments.len(), 1);
        assert!(!entry.segments[0].sealed);
    }

    #[test]
    fn test_manifest_version_guard() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "test-host_abc123";
        let hostname = "test-host";

        // Write a manifest with v=99
        let device_path = device_dir(dir.path(), device_id);
        fs::create_dir_all(&device_path).unwrap();
        let manifest_json = r#"{"v":99,"device_id":"test-host_abc123","hostname":"test-host","written_at":"2026-01-01T00:00:00Z","buckets":{}}"#;
        fs::write(manifest_path(dir.path(), device_id), manifest_json).unwrap();

        let result = Manifest::load_or_default(dir.path(), device_id, hostname);
        assert!(result.is_err(), "should refuse v > MAX_V");
        assert!(result.unwrap_err().contains("refusing to open"));
    }

    #[test]
    fn test_device_id_path_traversal_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let hostname = "test-host";
        for bad_id in &["../evil", "../../etc/passwd", "foo/bar", "foo\\bar"] {
            let result = Manifest::load_or_default(dir.path(), bad_id, hostname);
            assert!(
                result.is_err(),
                "should reject path-unsafe device_id: {bad_id}"
            );
            assert!(result.unwrap_err().contains("invalid device_id"));
        }
    }
}
