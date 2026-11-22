//! Read-only segment reader and validator for aw-sync v2.
//!
//! Decodes peer-authored `devices/{device_id}/manifest.json` and
//! `{bucket_slug}.{generation}.jsonl.zst` files without modifying the sync
//! folder or local datastore.
//!
//! Security and integrity invariants:
//! - Every path is validated and confined under `<sync_root>/devices/<device_id>`
//!   with symlink rejection inside the sync root.
//! - Format versions outside `1..=MAX_V` are refused.
//! - Compressed-byte SHA-256 is verified before any decompression or JSON
//!   parsing occurs; a hash mismatch never yields decoded events.
//! - A checksum mismatch on a `sealed` segment is a hard corruption error; a
//!   mismatch on an `unsealed` tail segment (or a not-yet-synced segment file)
//!   returns [`ReadOutcome::Retryable`] so callers can retry on a later pass.
//! - Manifest and segment-header identity (`device_id`, `bucket_id`,
//!   `generation`, bucket metadata, and event count) must agree.
//! - Manifest size, compressed size, decompressed size, per-line length, and
//!   per-segment event count are strictly bounded against peer-controlled input.

use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use aw_models::Event;

use super::manifest::{
    bucket_slug, device_dir, manifest_path, validate_device_id, BucketEntry, Manifest, SegmentEntry,
};
use super::segment::{ReplaceRange, SegmentHeader};
use super::MAX_V;

/// Resource bounds enforced when reading peer-controlled v2 sync files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderLimits {
    /// Maximum size of a `manifest.json` file in bytes.
    pub max_manifest_bytes: u64,
    /// Maximum size of a compressed `.jsonl.zst` segment file in bytes.
    pub max_compressed_segment_bytes: u64,
    /// Maximum decompressed byte size of a single segment (guards against zstd bombs).
    pub max_decompressed_segment_bytes: u64,
    /// Maximum byte length of a single JSONL line (header or event).
    pub max_line_bytes: usize,
    /// Maximum number of event records decoded from a single segment.
    pub max_events_per_segment: usize,
    /// Maximum cumulative event records buffered by [`SegmentReader::read_bucket`].
    /// Callers scanning arbitrarily long buckets should use [`SegmentReader::iter_bucket`]
    /// to consume one segment at a time without buffering full bucket history.
    pub max_events_per_bucket: usize,
}

impl Default for ReaderLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 16 * 1024 * 1024,
            max_compressed_segment_bytes: 64 * 1024 * 1024,
            max_decompressed_segment_bytes: 256 * 1024 * 1024,
            max_line_bytes: 4 * 1024 * 1024,
            max_events_per_segment: 1_000_000,
            max_events_per_bucket: 5_000_000,
        }
    }
}

/// Verified and decoded contents of a single v2 segment file.
#[derive(Debug, Clone)]
pub struct DecodedSegment {
    pub header: SegmentHeader,
    pub entry: SegmentEntry,
    /// Verified lowercase hex SHA-256 of the compressed segment bytes.
    pub verified_sha256: String,
    /// Decoded events in segment order, preserving source `id` and nanosecond `timestamp`.
    pub events: Vec<Event>,
}

impl DecodedSegment {
    /// Optional range-replace marker from the segment header. Exposed without applying it.
    pub fn replaces(&self) -> Option<&ReplaceRange> {
        self.header.replaces.as_ref()
    }
}

/// Reason a segment could not be read on this pass, but may succeed on a later pass
/// once the file syncer finishes transferring the manifest and segment files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryReason {
    /// Manifest references a segment file that does not exist on disk yet.
    SegmentNotYetPresent { file: String, sealed: bool },
    /// Unsealed open-tail segment's compressed bytes differ from the manifest's SHA-256.
    UnsealedTailChanged {
        file: String,
        expected_sha256: String,
        actual_sha256: String,
    },
}

impl fmt::Display for RetryReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SegmentNotYetPresent { file, sealed } => write!(
                f,
                "segment {file:?} (sealed={sealed}) referenced by manifest is not present yet"
            ),
            Self::UnsealedTailChanged {
                file,
                expected_sha256,
                actual_sha256,
            } => write!(
                f,
                "unsealed tail segment {file:?} changed relative to manifest (expected {expected_sha256}, got {actual_sha256})"
            ),
        }
    }
}

/// Outcome of reading a single v2 segment.
#[derive(Debug, Clone)]
pub enum ReadOutcome {
    /// Segment passed all integrity, version, and identity checks and was decoded.
    Ready(Box<DecodedSegment>),
    /// Manifest and segment file are temporarily out of step; retry on a later pass.
    /// No events are yielded.
    Retryable(RetryReason),
}

impl ReadOutcome {
    /// Returns the decoded segment if [`ReadOutcome::Ready`], otherwise `None`.
    pub fn into_ready(self) -> Option<DecodedSegment> {
        match self {
            Self::Ready(seg) => Some(*seg),
            Self::Retryable(_) => None,
        }
    }

    /// True if the outcome is [`ReadOutcome::Retryable`].
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}

/// Hard errors returned by [`SegmentReader`].
#[derive(Debug)]
pub enum ReadError {
    InvalidDeviceId(String),
    InvalidPath {
        path: String,
        reason: String,
    },
    ManifestNotFound {
        device_id: String,
        path: PathBuf,
    },
    BucketNotFound {
        device_id: String,
        bucket_id: String,
    },
    UnsupportedVersion {
        context: String,
        found: u32,
        max_supported: u32,
    },
    SizeLimitExceeded {
        context: String,
        limit: u64,
    },
    SealedChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    IdentityMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    Decompression {
        file: String,
        reason: String,
    },
    MalformedManifest {
        path: PathBuf,
        reason: String,
    },
    MalformedSegment {
        file: String,
        line: usize,
        reason: String,
    },
    Io {
        context: String,
        path: PathBuf,
        source: io::Error,
    },
}

impl ReadError {
    /// True when the error reflects an out-of-order sync arrival (such as a device
    /// directory whose segments arrived before `manifest.json`).
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::ManifestNotFound { .. })
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDeviceId(msg) => write!(f, "invalid device_id: {msg}"),
            Self::InvalidPath { path, reason } => write!(f, "invalid path {path:?}: {reason}"),
            Self::ManifestNotFound { device_id, path } => {
                write!(
                    f,
                    "manifest not found for device {device_id:?} at {}",
                    path.display()
                )
            }
            Self::BucketNotFound {
                device_id,
                bucket_id,
            } => write!(
                f,
                "bucket {bucket_id:?} not found in manifest for device {device_id:?}"
            ),
            Self::UnsupportedVersion {
                context,
                found,
                max_supported,
            } => write!(
                f,
                "{context}: unsupported format version v{found} (supported: 1..={max_supported})"
            ),
            Self::SizeLimitExceeded { context, limit } => {
                write!(f, "{context}: exceeded limit of {limit} bytes/records")
            }
            Self::SealedChecksumMismatch {
                file,
                expected,
                actual,
            } => write!(
                f,
                "sealed segment {file:?} failed SHA-256 verification (expected {expected}, got {actual})"
            ),
            Self::IdentityMismatch {
                field,
                expected,
                actual,
            } => write!(
                f,
                "identity mismatch for {field}: expected {expected:?}, got {actual:?}"
            ),
            Self::Decompression { file, reason } => {
                write!(f, "failed to decompress segment {file:?}: {reason}")
            }
            Self::MalformedManifest { path, reason } => {
                write!(f, "malformed manifest {}: {reason}", path.display())
            }
            Self::MalformedSegment { file, line, reason } => {
                write!(f, "malformed segment {file:?} at line {line}: {reason}")
            }
            Self::Io {
                context,
                path,
                source,
            } => write!(f, "{context} ({}): {source}", path.display()),
        }
    }
}

impl Error for ReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Standalone read-only reader for aw-sync v2 manifests and segment files.
#[derive(Debug, Clone)]
pub struct SegmentReader {
    sync_root: PathBuf,
    limits: ReaderLimits,
}

impl SegmentReader {
    /// Create a reader rooted at `sync_root` with default resource limits.
    pub fn new(sync_root: impl Into<PathBuf>) -> Self {
        Self {
            sync_root: sync_root.into(),
            limits: ReaderLimits::default(),
        }
    }

    /// Create a reader rooted at `sync_root` with custom resource limits.
    pub fn with_limits(sync_root: impl Into<PathBuf>, limits: ReaderLimits) -> Self {
        Self {
            sync_root: sync_root.into(),
            limits,
        }
    }

    /// Return the configured sync root directory.
    pub fn sync_root(&self) -> &Path {
        &self.sync_root
    }

    /// Return the configured resource limits.
    pub fn limits(&self) -> ReaderLimits {
        self.limits
    }

    /// Discover device IDs under `<sync_root>/devices/` that have a readable
    /// `manifest.json` path (sorted lexicographically).
    ///
    /// Device directories whose `manifest.json` has not arrived yet, hidden/tmp
    /// entries, symlinks, and non-directories are skipped without error.
    pub fn discover_devices(&self) -> Result<Vec<String>, ReadError> {
        let devices_root = self.sync_root.join("devices");
        let root_meta = match fs::symlink_metadata(&devices_root) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(ReadError::Io {
                    context: "inspect devices directory".to_string(),
                    path: devices_root,
                    source,
                });
            }
        };
        if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
            return Err(ReadError::InvalidPath {
                path: devices_root.display().to_string(),
                reason: "devices root must be a real directory, not a symlink".to_string(),
            });
        }

        let entries = fs::read_dir(&devices_root).map_err(|source| ReadError::Io {
            context: "read devices directory".to_string(),
            path: devices_root.clone(),
            source,
        })?;

        let mut devices = Vec::new();
        for entry_res in entries {
            let entry = entry_res.map_err(|source| ReadError::Io {
                context: "iterate devices directory".to_string(),
                path: devices_root.clone(),
                source,
            })?;
            let file_type = entry.file_type().map_err(|source| ReadError::Io {
                context: "inspect device entry".to_string(),
                path: entry.path(),
                source,
            })?;
            if file_type.is_symlink() || !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if name.starts_with('.') || validate_device_id(&name).is_err() {
                continue;
            }
            let manifest_file = entry.path().join("manifest.json");
            if let Ok(meta) = fs::symlink_metadata(&manifest_file) {
                if !meta.file_type().is_symlink() && meta.is_file() {
                    devices.push(name);
                }
            }
        }
        devices.sort();
        Ok(devices)
    }

    /// Read and validate `devices/{device_id}/manifest.json` without creating or
    /// modifying any files on disk.
    pub fn load_manifest(&self, device_id: &str) -> Result<Manifest, ReadError> {
        validate_device_id(device_id).map_err(ReadError::InvalidDeviceId)?;

        let path = manifest_path(&self.sync_root, device_id);
        let Some(file) =
            confined_open::open_confined_file(&self.sync_root, device_id, "manifest.json")?
        else {
            return Err(ReadError::ManifestNotFound {
                device_id: device_id.to_string(),
                path,
            });
        };

        let bytes =
            read_bounded_from_file(file, &path, self.limits.max_manifest_bytes, "manifest.json")?;
        let text = String::from_utf8(bytes).map_err(|e| ReadError::MalformedManifest {
            path: path.clone(),
            reason: format!("invalid UTF-8: {e}"),
        })?;
        let manifest: Manifest =
            serde_json::from_str(&text).map_err(|e| ReadError::MalformedManifest {
                path: path.clone(),
                reason: format!("invalid JSON: {e}"),
            })?;

        check_supported_version("manifest", manifest.v)?;
        if manifest.device_id != device_id {
            return Err(ReadError::IdentityMismatch {
                field: "manifest.device_id",
                expected: device_id.to_string(),
                actual: manifest.device_id,
            });
        }

        Ok(manifest)
    }

    /// Verify and decode a single segment referenced by `manifest` for `bucket_id`.
    pub fn read_segment(
        &self,
        manifest: &Manifest,
        bucket_id: &str,
        entry: &SegmentEntry,
    ) -> Result<ReadOutcome, ReadError> {
        check_supported_version("manifest", manifest.v)?;
        validate_device_id(&manifest.device_id).map_err(ReadError::InvalidDeviceId)?;

        let bucket_entry =
            manifest
                .buckets
                .get(bucket_id)
                .ok_or_else(|| ReadError::BucketNotFound {
                    device_id: manifest.device_id.clone(),
                    bucket_id: bucket_id.to_string(),
                })?;

        validate_bucket_slug(bucket_id, bucket_entry)?;
        validate_segment_filename(&entry.file)?;
        let expected_file = expected_segment_filename(&bucket_entry.slug, entry.generation);
        if entry.file != expected_file {
            return Err(ReadError::IdentityMismatch {
                field: "segment.file",
                expected: expected_file,
                actual: entry.file.clone(),
            });
        }

        let seg_path = device_dir(&self.sync_root, &manifest.device_id).join(&entry.file);
        let Some(file) =
            confined_open::open_confined_file(&self.sync_root, &manifest.device_id, &entry.file)?
        else {
            return Ok(ReadOutcome::Retryable(RetryReason::SegmentNotYetPresent {
                file: entry.file.clone(),
                sealed: entry.sealed,
            }));
        };

        // Read compressed bytes (bounded) from the already-verified handle and
        // verify SHA-256 BEFORE decompressing.
        let compressed = read_bounded_from_file(
            file,
            &seg_path,
            self.limits.max_compressed_segment_bytes,
            &format!("compressed segment {:?}", entry.file),
        )?;

        let actual_sha256 = sha256_hex(&compressed);
        let expected_sha256 = entry.sha256.to_ascii_lowercase();
        if actual_sha256 != expected_sha256 {
            if entry.sealed {
                return Err(ReadError::SealedChecksumMismatch {
                    file: entry.file.clone(),
                    expected: entry.sha256.clone(),
                    actual: actual_sha256,
                });
            }
            return Ok(ReadOutcome::Retryable(RetryReason::UnsealedTailChanged {
                file: entry.file.clone(),
                expected_sha256: entry.sha256.clone(),
                actual_sha256,
            }));
        }

        // Stream-decompress with hard upper bound against zip bombs.
        let (header, events) = self.decode_verified_segment(&entry.file, &compressed)?;

        // Validate header version and manifest/header agreement before returning events.
        check_supported_version(&format!("segment {:?}", entry.file), header.v)?;
        validate_header_identity(&manifest.device_id, bucket_id, bucket_entry, entry, &header)?;

        if events.len() as u64 != entry.n_events {
            return Err(ReadError::IdentityMismatch {
                field: "segment.n_events",
                expected: entry.n_events.to_string(),
                actual: events.len().to_string(),
            });
        }

        Ok(ReadOutcome::Ready(Box::new(DecodedSegment {
            header,
            entry: entry.clone(),
            verified_sha256: actual_sha256,
            events,
        })))
    }

    /// Lazily stream all segments listed for `bucket_id` in ascending `generation` order.
    ///
    /// Preferred over [`Self::read_bucket`] for long-lived buckets: only one segment's
    /// decoded events live in memory at a time.
    pub fn iter_bucket<'a>(
        &'a self,
        manifest: &'a Manifest,
        bucket_id: &'a str,
    ) -> Result<BucketSegmentIter<'a>, ReadError> {
        let bucket_entry =
            manifest
                .buckets
                .get(bucket_id)
                .ok_or_else(|| ReadError::BucketNotFound {
                    device_id: manifest.device_id.clone(),
                    bucket_id: bucket_id.to_string(),
                })?;

        let mut ordered: Vec<&'a SegmentEntry> = bucket_entry.segments.iter().collect();
        ordered.sort_by_key(|seg| seg.generation);
        for pair in ordered.windows(2) {
            if pair[0].generation == pair[1].generation {
                return Err(ReadError::MalformedManifest {
                    path: manifest_path(&self.sync_root, &manifest.device_id),
                    reason: format!(
                        "duplicate generation {} for bucket {bucket_id:?}",
                        pair[0].generation
                    ),
                });
            }
        }

        Ok(BucketSegmentIter {
            reader: self,
            manifest,
            bucket_id,
            entries: ordered.into_iter(),
        })
    }

    /// Read all segments listed for `bucket_id` in ascending `generation` order,
    /// enforcing [`ReaderLimits::max_events_per_bucket`] across the collected result.
    ///
    /// For arbitrarily large buckets, prefer [`Self::iter_bucket`] to process one
    /// segment at a time without retaining the full bucket history in memory.
    pub fn read_bucket(
        &self,
        manifest: &Manifest,
        bucket_id: &str,
    ) -> Result<Vec<ReadOutcome>, ReadError> {
        let iter = self.iter_bucket(manifest, bucket_id)?;
        let mut outcomes = Vec::with_capacity(iter.len());
        let mut total_events: usize = 0;
        for outcome_res in iter {
            let outcome = outcome_res?;
            if let ReadOutcome::Ready(ref decoded) = outcome {
                total_events = total_events.saturating_add(decoded.events.len());
                if total_events > self.limits.max_events_per_bucket {
                    return Err(ReadError::SizeLimitExceeded {
                        context: format!("bucket {bucket_id:?} total event count"),
                        limit: self.limits.max_events_per_bucket as u64,
                    });
                }
            }
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    fn decode_verified_segment(
        &self,
        file: &str,
        compressed: &[u8],
    ) -> Result<(SegmentHeader, Vec<Event>), ReadError> {
        let decoder =
            zstd::stream::read::Decoder::new(compressed).map_err(|e| ReadError::Decompression {
                file: file.to_string(),
                reason: e.to_string(),
            })?;

        let max_decompressed = self.limits.max_decompressed_segment_bytes;
        let mut bounded = decoder.take(max_decompressed.saturating_add(1));
        let mut reader = BufReader::new(&mut bounded);

        let mut total_bytes: u64 = 0;
        let mut header: Option<SegmentHeader> = None;
        let mut events: Vec<Event> = Vec::new();
        let mut line_no: usize = 0;

        loop {
            let line = match read_bounded_line(&mut reader, self.limits.max_line_bytes) {
                Ok(None) => break,
                Ok(Some(raw)) => raw,
                Err(LineReadError::TooLong) => {
                    return Err(ReadError::SizeLimitExceeded {
                        context: format!("segment {file:?} line {}", line_no + 1),
                        limit: self.limits.max_line_bytes as u64,
                    });
                }
                Err(LineReadError::Io(e)) => {
                    return Err(ReadError::Decompression {
                        file: file.to_string(),
                        reason: e.to_string(),
                    });
                }
            };

            total_bytes = total_bytes.saturating_add(line.len() as u64);
            if total_bytes > max_decompressed {
                return Err(ReadError::SizeLimitExceeded {
                    context: format!("decompressed segment {file:?}"),
                    limit: max_decompressed,
                });
            }

            line_no += 1;
            let trimmed = strip_line_ending(&line);
            let text = std::str::from_utf8(trimmed).map_err(|e| ReadError::MalformedSegment {
                file: file.to_string(),
                line: line_no,
                reason: format!("invalid UTF-8: {e}"),
            })?;

            if text.is_empty() {
                return Err(ReadError::MalformedSegment {
                    file: file.to_string(),
                    line: line_no,
                    reason: "unexpected empty line in segment JSONL".to_string(),
                });
            }

            if line_no == 1 {
                let parsed: SegmentHeader =
                    serde_json::from_str(text).map_err(|e| ReadError::MalformedSegment {
                        file: file.to_string(),
                        line: 1,
                        reason: format!("invalid segment header JSON: {e}"),
                    })?;
                header = Some(parsed);
            } else {
                if events.len() >= self.limits.max_events_per_segment {
                    return Err(ReadError::SizeLimitExceeded {
                        context: format!("segment {file:?} event count"),
                        limit: self.limits.max_events_per_segment as u64,
                    });
                }
                let event: Event =
                    serde_json::from_str(text).map_err(|e| ReadError::MalformedSegment {
                        file: file.to_string(),
                        line: line_no,
                        reason: format!("invalid event JSON: {e}"),
                    })?;
                events.push(event);
            }
        }

        let header = header.ok_or_else(|| ReadError::MalformedSegment {
            file: file.to_string(),
            line: 0,
            reason: "segment is empty (missing header line)".to_string(),
        })?;

        Ok((header, events))
    }
}

enum LineReadError {
    TooLong,
    Io(io::Error),
}

/// Read a single `\n`-delimited line without allocating more than `max_bytes + 1`.
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, LineReadError> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(LineReadError::Io)?;
        if available.is_empty() {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Ok(Some(buf))
            };
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            let take = pos + 1;
            if buf.len().saturating_add(take) > max_bytes {
                return Err(LineReadError::TooLong);
            }
            buf.extend_from_slice(&available[..take]);
            reader.consume(take);
            return Ok(Some(buf));
        }
        let len = available.len();
        if buf.len().saturating_add(len) > max_bytes {
            return Err(LineReadError::TooLong);
        }
        buf.extend_from_slice(available);
        reader.consume(len);
    }
}

fn strip_line_ending(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

/// Lazy streaming iterator over a bucket's segments in ascending `generation` order.
#[derive(Debug)]
pub struct BucketSegmentIter<'a> {
    reader: &'a SegmentReader,
    manifest: &'a Manifest,
    bucket_id: &'a str,
    entries: std::vec::IntoIter<&'a SegmentEntry>,
}

impl Iterator for BucketSegmentIter<'_> {
    type Item = Result<ReadOutcome, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.entries.next()?;
        Some(
            self.reader
                .read_segment(self.manifest, self.bucket_id, entry),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

impl ExactSizeIterator for BucketSegmentIter<'_> {}

fn read_bounded_from_file(
    file: File,
    path: &Path,
    max_bytes: u64,
    context: &str,
) -> Result<Vec<u8>, ReadError> {
    let mut buf = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|source| ReadError::Io {
            context: format!("read {context}"),
            path: path.to_path_buf(),
            source,
        })?;
    if buf.len() as u64 > max_bytes {
        return Err(ReadError::SizeLimitExceeded {
            context: context.to_string(),
            limit: max_bytes,
        });
    }
    Ok(buf)
}

#[cfg(unix)]
mod confined_open {
    use std::ffi::CString;
    use std::fs::File;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
    use std::path::Path;

    use super::ReadError;

    fn open_subdir_nofollow(
        parent: &OwnedFd,
        name: &str,
        display_path: &Path,
    ) -> Result<Option<OwnedFd>, ReadError> {
        let c_name = CString::new(name).map_err(|_| ReadError::InvalidPath {
            path: display_path.display().to_string(),
            reason: "path component contains NUL byte".to_string(),
        })?;
        // SAFETY: `parent` is a valid open directory fd and `c_name` is a valid NUL-terminated C string.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let err = io::Error::last_os_error();
            return match err.raw_os_error() {
                Some(libc::ENOENT) => Ok(None),
                Some(libc::ELOOP) | Some(libc::ENOTDIR) => Err(ReadError::InvalidPath {
                    path: display_path.display().to_string(),
                    reason: "directory component is a symlink or not a directory".to_string(),
                }),
                _ => Err(ReadError::Io {
                    context: "open directory".to_string(),
                    path: display_path.to_path_buf(),
                    source: err,
                }),
            };
        }
        // SAFETY: `fd` is a newly opened descriptor returned by `openat`.
        Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Open `<sync_root>/devices/<device_id>/<filename>` strictly relative to
    /// checked directory handles using `O_NOFOLLOW` at every hop inside `sync_root`.
    /// Returns `Ok(None)` when any component does not exist (`ENOENT`).
    pub(super) fn open_confined_file(
        sync_root: &Path,
        device_id: &str,
        filename: &str,
    ) -> Result<Option<File>, ReadError> {
        let c_root =
            CString::new(sync_root.as_os_str().as_bytes()).map_err(|_| ReadError::InvalidPath {
                path: sync_root.display().to_string(),
                reason: "sync root path contains NUL byte".to_string(),
            })?;
        // Allow `sync_root` itself to be a user-configured symlink (e.g. ~/ActivityWatchSync),
        // then enforce O_NOFOLLOW at every component inside `sync_root`.
        // SAFETY: `c_root` is a valid NUL-terminated path string.
        let root_fd = unsafe {
            libc::open(
                c_root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if root_fd < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ENOENT) {
                return Ok(None);
            }
            return Err(ReadError::Io {
                context: "open sync root".to_string(),
                path: sync_root.to_path_buf(),
                source: err,
            });
        }
        // SAFETY: `root_fd` is a newly opened descriptor returned by `open`.
        let root_fd = unsafe { OwnedFd::from_raw_fd(root_fd) };

        let devices_path = sync_root.join("devices");
        let Some(devices_fd) = open_subdir_nofollow(&root_fd, "devices", &devices_path)? else {
            return Ok(None);
        };

        let dev_path = devices_path.join(device_id);
        let Some(dev_fd) = open_subdir_nofollow(&devices_fd, device_id, &dev_path)? else {
            return Ok(None);
        };

        let file_path = dev_path.join(filename);
        let c_file = CString::new(filename).map_err(|_| ReadError::InvalidPath {
            path: filename.to_string(),
            reason: "filename contains NUL byte".to_string(),
        })?;
        // O_NOFOLLOW rejects symlinks atomically during open; O_NONBLOCK prevents
        // blocking on a malicious FIFO before fstat verifies `is_file()`.
        // SAFETY: `dev_fd` is a valid open directory fd and `c_file` is NUL-terminated.
        let file_fd = unsafe {
            libc::openat(
                dev_fd.as_raw_fd(),
                c_file.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if file_fd < 0 {
            let err = io::Error::last_os_error();
            return match err.raw_os_error() {
                Some(libc::ENOENT) => Ok(None),
                Some(libc::ELOOP) => Err(ReadError::InvalidPath {
                    path: filename.to_string(),
                    reason: "file is a symlink".to_string(),
                }),
                Some(libc::EISDIR) => Err(ReadError::InvalidPath {
                    path: filename.to_string(),
                    reason: "expected regular file, found directory".to_string(),
                }),
                _ => Err(ReadError::Io {
                    context: "open file".to_string(),
                    path: file_path,
                    source: err,
                }),
            };
        }
        // SAFETY: `file_fd` is a newly opened descriptor returned by `openat`.
        let file = File::from(unsafe { OwnedFd::from_raw_fd(file_fd) });
        let meta = file.metadata().map_err(|source| ReadError::Io {
            context: "stat opened file".to_string(),
            path: file_path,
            source,
        })?;
        if !meta.is_file() {
            return Err(ReadError::InvalidPath {
                path: filename.to_string(),
                reason: "expected a regular file".to_string(),
            });
        }
        Ok(Some(file))
    }
}

#[cfg(windows)]
mod confined_open {
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    use super::ReadError;

    pub(super) fn open_confined_file(
        sync_root: &Path,
        device_id: &str,
        filename: &str,
    ) -> Result<Option<File>, ReadError> {
        let canon_root = match fs::canonicalize(sync_root) {
            Ok(p) => p,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ReadError::Io {
                    context: "canonicalize sync root".to_string(),
                    path: sync_root.to_path_buf(),
                    source,
                });
            }
        };

        let devices_dir = sync_root.join("devices");
        let dev_dir = devices_dir.join(device_id);
        for dir in [&devices_dir, &dev_dir] {
            let meta = match fs::symlink_metadata(dir) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(source) => {
                    return Err(ReadError::Io {
                        context: "inspect directory".to_string(),
                        path: dir.to_path_buf(),
                        source,
                    });
                }
            };
            if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 || !meta.is_dir() {
                return Err(ReadError::InvalidPath {
                    path: dir.display().to_string(),
                    reason: "directory component is a reparse point or not a directory".to_string(),
                });
            }
        }

        let canon_dev = fs::canonicalize(&dev_dir).map_err(|source| ReadError::Io {
            context: "canonicalize device directory".to_string(),
            path: dev_dir,
            source,
        })?;
        if !canon_dev.starts_with(&canon_root) {
            return Err(ReadError::InvalidPath {
                path: canon_dev.display().to_string(),
                reason: "device directory escapes sync root".to_string(),
            });
        }

        let target = canon_dev.join(filename);
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&target)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ReadError::Io {
                    context: "open file".to_string(),
                    path: target,
                    source,
                });
            }
        };
        let meta = file.metadata().map_err(|source| ReadError::Io {
            context: "inspect opened file".to_string(),
            path: target,
            source,
        })?;
        if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 || !meta.is_file() {
            return Err(ReadError::InvalidPath {
                path: filename.to_string(),
                reason: "file is a reparse point or not a regular file".to_string(),
            });
        }
        Ok(Some(file))
    }
}

fn check_supported_version(context: &str, version: u32) -> Result<(), ReadError> {
    if version == 0 || version > MAX_V {
        return Err(ReadError::UnsupportedVersion {
            context: context.to_string(),
            found: version,
            max_supported: MAX_V,
        });
    }
    Ok(())
}

fn validate_bucket_slug(bucket_id: &str, bucket_entry: &BucketEntry) -> Result<(), ReadError> {
    let expected = bucket_slug(bucket_id);
    if bucket_entry.slug != expected {
        return Err(ReadError::IdentityMismatch {
            field: "bucket.slug",
            expected,
            actual: bucket_entry.slug.clone(),
        });
    }
    Ok(())
}

fn validate_segment_filename(file: &str) -> Result<(), ReadError> {
    if file.is_empty()
        || file.starts_with('.')
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.contains('\0')
    {
        return Err(ReadError::InvalidPath {
            path: file.to_string(),
            reason:
                "segment filename must be a plain filename without path separators or hidden prefix"
                    .to_string(),
        });
    }
    if !file.ends_with(".jsonl.zst") {
        return Err(ReadError::InvalidPath {
            path: file.to_string(),
            reason: "segment filename must end with '.jsonl.zst'".to_string(),
        });
    }
    let mut components = Path::new(file).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err(ReadError::InvalidPath {
            path: file.to_string(),
            reason: "segment filename must consist of a single normal path component".to_string(),
        }),
    }
}

fn validate_header_identity(
    device_id: &str,
    bucket_id: &str,
    bucket_entry: &BucketEntry,
    entry: &SegmentEntry,
    header: &SegmentHeader,
) -> Result<(), ReadError> {
    if header.device_id != device_id {
        return Err(ReadError::IdentityMismatch {
            field: "header.device_id",
            expected: device_id.to_string(),
            actual: header.device_id.clone(),
        });
    }
    if header.bucket_id != bucket_id {
        return Err(ReadError::IdentityMismatch {
            field: "header.bucket_id",
            expected: bucket_id.to_string(),
            actual: header.bucket_id.clone(),
        });
    }
    if header.generation != entry.generation {
        return Err(ReadError::IdentityMismatch {
            field: "header.generation",
            expected: entry.generation.to_string(),
            actual: header.generation.to_string(),
        });
    }
    if header.bucket_type != bucket_entry._type {
        return Err(ReadError::IdentityMismatch {
            field: "header.bucket_type",
            expected: bucket_entry._type.clone(),
            actual: header.bucket_type.clone(),
        });
    }
    if header.bucket_client != bucket_entry.client {
        return Err(ReadError::IdentityMismatch {
            field: "header.bucket_client",
            expected: bucket_entry.client.clone(),
            actual: header.bucket_client.clone(),
        });
    }
    if header.bucket_hostname != bucket_entry.hostname {
        return Err(ReadError::IdentityMismatch {
            field: "header.bucket_hostname",
            expected: bucket_entry.hostname.clone(),
            actual: header.bucket_hostname.clone(),
        });
    }
    if header.bucket_created != bucket_entry.created {
        return Err(ReadError::IdentityMismatch {
            field: "header.bucket_created",
            expected: format!("{:?}", bucket_entry.created),
            actual: format!("{:?}", header.bucket_created),
        });
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn expected_segment_filename(slug: &str, generation: u64) -> String {
    format!("{slug}.{generation:08}.jsonl.zst")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{DateTime, Duration, TimeZone, Utc};
    use serde_json::{json, Map};

    use aw_models::Bucket;

    use super::*;
    use crate::v2::SegmentWriter;

    fn sample_bucket(bucket_id: &str, hostname: &str) -> Bucket {
        Bucket {
            bid: None,
            id: bucket_id.to_string(),
            _type: "currentwindow".to_string(),
            client: "aw-watcher-window".to_string(),
            hostname: hostname.to_string(),
            created: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            data: Map::new(),
            metadata: Default::default(),
            events: None,
            last_updated: None,
        }
    }

    fn sample_event(id: Option<i64>, ts: DateTime<Utc>, dur_ms: i64, app: &str) -> Event {
        let mut data = Map::new();
        data.insert("app".to_string(), json!(app));
        data.insert("title".to_string(), json!(format!("{app} window")));
        Event {
            id,
            timestamp: ts,
            duration: Duration::milliseconds(dur_ms),
            data,
        }
    }

    /// Build a synthetic zstd-compressed segment file and matching manifest entry.
    fn write_synthetic_segment(
        sync_root: &Path,
        device_id: &str,
        bucket: &Bucket,
        header: &SegmentHeader,
        lines: &[String],
        sealed: bool,
    ) -> (Manifest, SegmentEntry, PathBuf) {
        let slug = bucket_slug(&bucket.id);
        let file = expected_segment_filename(&slug, header.generation);
        let dev_dir = device_dir(sync_root, device_id);
        fs::create_dir_all(&dev_dir).unwrap();

        let mut jsonl = Vec::new();
        serde_json::to_writer(&mut jsonl, header).unwrap();
        jsonl.push(b'\n');
        for line in lines {
            jsonl.extend_from_slice(line.as_bytes());
            jsonl.push(b'\n');
        }
        let compressed = zstd::encode_all(jsonl.as_slice(), 0).unwrap();
        let digest = sha256_hex(&compressed);
        let seg_path = dev_dir.join(&file);
        fs::write(&seg_path, &compressed).unwrap();

        let entry = SegmentEntry {
            generation: header.generation,
            file,
            n_events: lines.len() as u64,
            start_ts: None,
            end_ts: None,
            sha256: digest,
            sealed,
            first_written_at: Some(Utc::now()),
        };

        let mut buckets = HashMap::new();
        buckets.insert(
            bucket.id.clone(),
            BucketEntry {
                slug,
                _type: bucket._type.clone(),
                client: bucket.client.clone(),
                hostname: bucket.hostname.clone(),
                created: bucket.created,
                latest_generation: header.generation,
                total_events: lines.len() as u64,
                segments: vec![entry.clone()],
            },
        );
        let manifest = Manifest {
            v: 1,
            device_id: device_id.to_string(),
            hostname: bucket.hostname.clone(),
            written_at: Utc::now(),
            buckets,
        };
        manifest.save(sync_root, device_id).unwrap();
        (manifest, entry, seg_path)
    }

    fn base_header(device_id: &str, bucket: &Bucket, generation: u64) -> SegmentHeader {
        SegmentHeader {
            v: 1,
            device_id: device_id.to_string(),
            bucket_id: bucket.id.clone(),
            bucket_type: bucket._type.clone(),
            bucket_client: bucket.client.clone(),
            bucket_hostname: bucket.hostname.clone(),
            bucket_created: bucket.created,
            generation,
            written_at: Utc::now(),
            replaces: None,
        }
    }

    #[test]
    fn test_writer_to_reader_round_trip_preserves_ids_and_nanoseconds() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-laptop-01";
        let bucket = sample_bucket("aw-watcher-window_laptop", "laptop");

        let ts1 = Utc.with_ymd_and_hms(2026, 11, 22, 10, 15, 30).unwrap()
            + Duration::nanoseconds(123_456_789);
        let ts2 = ts1 + Duration::seconds(45) + Duration::nanoseconds(987_654_321);
        // Intentionally pass newest-first to verify writer sorts chronologically and reader preserves it.
        let events = vec![
            sample_event(None, ts2, 15_500, "Firefox"),
            sample_event(Some(42), ts1, 30_250, "Terminal"),
        ];

        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();
        let gen = writer.write_events(&bucket, &events).unwrap();
        assert_eq!(gen, 1);

        let reader = SegmentReader::new(dir.path());
        assert_eq!(
            reader.discover_devices().unwrap(),
            vec![device_id.to_string()]
        );

        let manifest = reader.load_manifest(device_id).unwrap();
        assert_eq!(manifest.device_id, device_id);
        let outcomes = reader.read_bucket(&manifest, &bucket.id).unwrap();
        assert_eq!(outcomes.len(), 1);

        let decoded = outcomes.into_iter().next().unwrap().into_ready().unwrap();
        assert_eq!(decoded.header.v, 1);
        assert_eq!(decoded.header.device_id, device_id);
        assert_eq!(decoded.header.bucket_id, bucket.id);
        assert_eq!(decoded.header.generation, 1);
        assert!(decoded.replaces().is_none());
        assert_eq!(decoded.events.len(), 2);

        assert_eq!(decoded.events[0].id, Some(42));
        assert_eq!(decoded.events[0].timestamp, ts1);
        assert_eq!(
            decoded.events[0].timestamp.timestamp_subsec_nanos(),
            123_456_789
        );
        assert_eq!(decoded.events[0].data["app"], json!("Terminal"));

        assert_eq!(decoded.events[1].id, None);
        assert_eq!(decoded.events[1].timestamp, ts2);
        assert_eq!(decoded.events[1].data["app"], json!("Firefox"));
    }

    #[test]
    fn test_round_trip_sealed_and_unsealed_generations() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-multi-gen";
        let bucket = sample_bucket("aw-watcher-window_multi", "multi");

        let ts1 = Utc.with_ymd_and_hms(2026, 11, 22, 8, 0, 0).unwrap();
        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();
        assert_eq!(
            writer
                .write_events(&bucket, &[sample_event(Some(1), ts1, 5_000, "Editor")])
                .unwrap(),
            1
        );

        // Age-seal generation 1 by backdating first_written_at in the manifest so generation 2 starts cleanly.
        let mut manifest = Manifest::load_or_default(dir.path(), device_id, "multi").unwrap();
        manifest.buckets.get_mut(&bucket.id).unwrap().segments[0].first_written_at =
            Some(Utc::now() - Duration::days(2));
        manifest.save(dir.path(), device_id).unwrap();

        let ts2 = ts1 + Duration::minutes(5);
        assert_eq!(
            writer
                .write_events(&bucket, &[sample_event(Some(2), ts2, 8_000, "Browser")])
                .unwrap(),
            2
        );

        let reader = SegmentReader::new(dir.path());
        let loaded = reader.load_manifest(device_id).unwrap();
        let outcomes = reader.read_bucket(&loaded, &bucket.id).unwrap();
        assert_eq!(outcomes.len(), 2);

        let seg1 = outcomes[0].clone().into_ready().unwrap();
        let seg2 = outcomes[1].clone().into_ready().unwrap();
        assert!(seg1.entry.sealed);
        assert!(!seg2.entry.sealed);
        assert_eq!(seg1.header.generation, 1);
        assert_eq!(seg2.header.generation, 2);
        assert_eq!(seg1.events[0].id, Some(1));
        assert_eq!(seg2.events[0].id, Some(2));
    }

    #[test]
    fn test_exposes_replaces_range_without_applying_it() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-replace";
        let bucket = sample_bucket("aw-watcher-window_rep", "rep");

        let from = Utc.with_ymd_and_hms(2026, 11, 22, 9, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 11, 22, 10, 0, 0).unwrap();
        let mut header = base_header(device_id, &bucket, 1);
        header.replaces = Some(ReplaceRange { from, to });

        let (manifest, entry, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &header, &[], true);

        let reader = SegmentReader::new(dir.path());
        let decoded = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap()
            .into_ready()
            .unwrap();
        assert_eq!(decoded.replaces(), Some(&ReplaceRange { from, to }));
        assert!(decoded.events.is_empty());
    }

    #[test]
    fn test_sealed_corruption_is_hard_error_and_yields_no_events() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-corrupt-sealed";
        let bucket = sample_bucket("aw-watcher-window_sealed", "sealed");
        let header = base_header(device_id, &bucket, 1);
        let ev = json!({
            "id": 7,
            "timestamp": "2026-11-22T09:00:00.000000000Z",
            "duration": 10.0,
            "data": {"app": "Firefox"}
        })
        .to_string();

        let (manifest, entry, seg_path) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &header, &[ev], true);

        // Flip a byte in the sealed segment file.
        let mut bytes = fs::read(&seg_path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&seg_path, &bytes).unwrap();

        let reader = SegmentReader::new(dir.path());
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(!err.is_retryable());
        assert!(
            matches!(err, ReadError::SealedChecksumMismatch { .. }),
            "expected SealedChecksumMismatch, got {err:?}"
        );
    }

    #[test]
    fn test_changed_unsealed_tail_is_retryable_and_yields_no_events() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-unsealed-race";
        let bucket = sample_bucket("aw-watcher-window_tail", "tail");

        let ts1 = Utc.with_ymd_and_hms(2026, 11, 22, 11, 0, 0).unwrap();
        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();
        writer
            .write_events(&bucket, &[sample_event(Some(1), ts1, 5_000, "AppA")])
            .unwrap();

        // Reader loads the manifest BEFORE the writer rewrites the open tail.
        let reader = SegmentReader::new(dir.path());
        let stale_manifest = reader.load_manifest(device_id).unwrap();
        let stale_entry = stale_manifest.buckets[&bucket.id].segments[0].clone();
        assert!(!stale_entry.sealed);

        // Writer rewrites the same unsealed generation with new events.
        let ts2 = ts1 + Duration::seconds(10);
        writer
            .write_events(&bucket, &[sample_event(Some(2), ts2, 5_000, "AppB")])
            .unwrap();

        // Reading with the stale manifest must return Retryable (no events decoded).
        let outcome = reader
            .read_segment(&stale_manifest, &bucket.id, &stale_entry)
            .unwrap();
        assert!(matches!(
            outcome,
            ReadOutcome::Retryable(RetryReason::UnsealedTailChanged { .. })
        ));
        assert!(outcome.into_ready().is_none());

        // Reloading the updated manifest now succeeds and returns both events.
        let fresh_manifest = reader.load_manifest(device_id).unwrap();
        let fresh_entry = &fresh_manifest.buckets[&bucket.id].segments[0];
        let decoded = reader
            .read_segment(&fresh_manifest, &bucket.id, fresh_entry)
            .unwrap()
            .into_ready()
            .unwrap();
        assert_eq!(decoded.events.len(), 2);
    }

    #[test]
    fn test_manifest_and_segment_arriving_out_of_order() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-ooo";
        let bucket = sample_bucket("aw-watcher-window_ooo", "ooo");
        let header = base_header(device_id, &bucket, 1);

        // Case 1: Manifest arrived ahead of the segment file.
        let (manifest, entry, seg_path) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &header, &[], true);
        let saved_bytes = fs::read(&seg_path).unwrap();
        fs::remove_file(&seg_path).unwrap();

        let reader = SegmentReader::new(dir.path());
        let outcome = reader.read_segment(&manifest, &bucket.id, &entry).unwrap();
        assert!(matches!(
            outcome,
            ReadOutcome::Retryable(RetryReason::SegmentNotYetPresent { sealed: true, .. })
        ));

        // Case 2: Segment file arrived ahead of manifest.json.
        fs::write(&seg_path, &saved_bytes).unwrap();
        fs::remove_file(manifest_path(dir.path(), device_id)).unwrap();
        assert!(reader.discover_devices().unwrap().is_empty());
        let err = reader.load_manifest(device_id).unwrap_err();
        assert!(err.is_retryable());
        assert!(matches!(err, ReadError::ManifestNotFound { .. }));
    }

    #[test]
    fn test_unsupported_versions_refused_in_manifest_and_header() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-ver";
        let bucket = sample_bucket("aw-watcher-window_ver", "ver");

        // Manifest v=0 and v=MAX_V+1 are both refused.
        for bad_v in [0u32, MAX_V + 1] {
            let dev_dir = device_dir(dir.path(), device_id);
            fs::create_dir_all(&dev_dir).unwrap();
            let raw = format!(
                r#"{{"v":{bad_v},"device_id":"{device_id}","hostname":"ver","written_at":"2026-01-01T00:00:00Z","buckets":{{}}}}"#
            );
            fs::write(manifest_path(dir.path(), device_id), raw).unwrap();
            let reader = SegmentReader::new(dir.path());
            let err = reader.load_manifest(device_id).unwrap_err();
            assert!(matches!(
                err,
                ReadError::UnsupportedVersion { found, .. } if found == bad_v
            ));
        }

        // Segment header v=MAX_V+1 inside an otherwise valid manifest is refused.
        let mut bad_header = base_header(device_id, &bucket, 1);
        bad_header.v = MAX_V + 1;
        let (manifest, entry, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &bad_header, &[], true);
        let reader = SegmentReader::new(dir.path());
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(
            err,
            ReadError::UnsupportedVersion { found, .. } if found == MAX_V + 1
        ));
    }

    #[test]
    fn test_malformed_jsonl_and_corrupt_zstd_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-malformed";
        let bucket = sample_bucket("aw-watcher-window_bad", "bad");
        let header = base_header(device_id, &bucket, 1);
        let reader = SegmentReader::new(dir.path());

        // Malformed event JSON line.
        let (manifest, entry, _) = write_synthetic_segment(
            dir.path(),
            device_id,
            &bucket,
            &header,
            &["{not-json".to_string()],
            true,
        );
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(err, ReadError::MalformedSegment { line: 2, .. }));

        // Corrupt zstd payload with matching SHA-256 -> Decompression error.
        let (mut manifest, mut entry, seg_path) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &header, &[], true);
        let garbage = b"not valid zstd bytes";
        fs::write(&seg_path, garbage).unwrap();
        entry.sha256 = sha256_hex(garbage);
        manifest.buckets.get_mut(&bucket.id).unwrap().segments = vec![entry.clone()];
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(err, ReadError::Decompression { .. }));

        // Empty decompressed segment (missing header line).
        let empty_zstd = zstd::encode_all(&b""[..], 0).unwrap();
        fs::write(&seg_path, &empty_zstd).unwrap();
        entry.sha256 = sha256_hex(&empty_zstd);
        manifest.buckets.get_mut(&bucket.id).unwrap().segments = vec![entry.clone()];
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(err, ReadError::MalformedSegment { line: 0, .. }));
    }

    #[test]
    fn test_path_traversal_and_symlink_escape_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let reader = SegmentReader::new(dir.path());

        // Bad device IDs.
        for bad_id in ["", ".", "..", "../escape", "a/b", "a\\b", "bad\0id"] {
            let err = reader.load_manifest(bad_id).unwrap_err();
            assert!(
                matches!(err, ReadError::InvalidDeviceId(_)),
                "expected InvalidDeviceId for {bad_id:?}, got {err:?}"
            );
        }

        // Bad segment filenames (traversal, subdirs, hidden/tmp, wrong extension).
        let device_id = "dev-path";
        let bucket = sample_bucket("aw-watcher-window_path", "path");
        let header = base_header(device_id, &bucket, 1);
        let (manifest, base_entry, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &header, &[], true);

        for bad_file in [
            "",
            "../other.jsonl.zst",
            "sub/seg.jsonl.zst",
            "sub\\seg.jsonl.zst",
            ".hidden.jsonl.zst",
            "a1b2c3d4e5f60718.00000001.jsonl.zst.tmp",
            "sync.db",
        ] {
            let mut bad_entry = base_entry.clone();
            bad_entry.file = bad_file.to_string();
            let err = reader
                .read_segment(&manifest, &bucket.id, &bad_entry)
                .unwrap_err();
            assert!(
                matches!(err, ReadError::InvalidPath { .. }),
                "expected InvalidPath for {bad_file:?}, got {err:?}"
            );
        }

        // Symlink inside device dir pointing outside sync_root must be rejected.
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = tempfile::tempdir().unwrap();
            let secret = outside.path().join("outside.jsonl.zst");
            let seg_path = device_dir(dir.path(), device_id).join(&base_entry.file);
            fs::copy(&seg_path, &secret).unwrap();
            fs::remove_file(&seg_path).unwrap();
            symlink(&secret, &seg_path).unwrap();

            let err = reader
                .read_segment(&manifest, &bucket.id, &base_entry)
                .unwrap_err();
            assert!(
                matches!(err, ReadError::InvalidPath { .. }),
                "expected InvalidPath for symlink segment, got {err:?}"
            );
        }
    }

    #[test]
    fn test_identity_mismatches_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-id-a";
        let bucket = sample_bucket("aw-watcher-window_a", "host-a");
        let reader = SegmentReader::new(dir.path());

        // 1. manifest.device_id != directory device_id.
        let dev_dir = device_dir(dir.path(), device_id);
        fs::create_dir_all(&dev_dir).unwrap();
        let wrong_manifest = Manifest {
            v: 1,
            device_id: "dev-id-other".to_string(),
            hostname: "host-a".to_string(),
            written_at: Utc::now(),
            buckets: HashMap::new(),
        };
        fs::write(
            manifest_path(dir.path(), device_id),
            serde_json::to_string(&wrong_manifest).unwrap(),
        )
        .unwrap();
        let err = reader.load_manifest(device_id).unwrap_err();
        assert!(matches!(
            err,
            ReadError::IdentityMismatch {
                field: "manifest.device_id",
                ..
            }
        ));

        // 2. header.device_id mismatch.
        let mut h = base_header(device_id, &bucket, 1);
        h.device_id = "dev-impostor".to_string();
        let (manifest, entry, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &h, &[], true);
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(
            err,
            ReadError::IdentityMismatch {
                field: "header.device_id",
                ..
            }
        ));

        // 3. header.bucket_id mismatch.
        let mut h = base_header(device_id, &bucket, 1);
        h.bucket_id = "aw-watcher-window_other".to_string();
        let (manifest, entry, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &h, &[], true);
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(
            err,
            ReadError::IdentityMismatch {
                field: "header.bucket_id",
                ..
            }
        ));

        // 4. header.generation mismatch (manifest says gen 1, header says gen 2).
        let mut h = base_header(device_id, &bucket, 1);
        h.generation = 2;
        let slug = bucket_slug(&bucket.id);
        let file = expected_segment_filename(&slug, 1);
        let mut jsonl = Vec::new();
        serde_json::to_writer(&mut jsonl, &h).unwrap();
        jsonl.push(b'\n');
        let compressed = zstd::encode_all(jsonl.as_slice(), 0).unwrap();
        fs::write(device_dir(dir.path(), device_id).join(&file), &compressed).unwrap();
        let mut manifest = manifest;
        let mut gen1_entry = entry;
        gen1_entry.generation = 1;
        gen1_entry.file = file;
        gen1_entry.sha256 = sha256_hex(&compressed);
        manifest.buckets.get_mut(&bucket.id).unwrap().segments = vec![gen1_entry.clone()];
        let err = reader
            .read_segment(&manifest, &bucket.id, &gen1_entry)
            .unwrap_err();
        assert!(matches!(
            err,
            ReadError::IdentityMismatch {
                field: "header.generation",
                ..
            }
        ));

        // 5. segment.n_events mismatch.
        let h = base_header(device_id, &bucket, 1);
        let (mut manifest, mut entry, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &h, &[], true);
        entry.n_events = 99;
        manifest.buckets.get_mut(&bucket.id).unwrap().segments = vec![entry.clone()];
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(
            err,
            ReadError::IdentityMismatch {
                field: "segment.n_events",
                ..
            }
        ));
    }

    #[test]
    fn test_bounds_reject_oversized_manifest_decompression_bomb_line_and_events() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-bounds";
        let bucket = sample_bucket("aw-watcher-window_bounds", "bounds");

        // Highly compressible payload (zstd bomb pattern): small compressed size, large decompressed size.
        let header = base_header(device_id, &bucket, 1);
        let big_event = json!({
            "id": 1,
            "timestamp": "2026-11-22T09:00:00.000000000Z",
            "duration": 1.0,
            "data": {"pad": "A".repeat(4096)}
        })
        .to_string();
        let (manifest, entry, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &header, &[big_event], true);

        // 1. Decompression limit triggers even when compressed bytes are tiny.
        let bomb_reader = SegmentReader::with_limits(
            dir.path(),
            ReaderLimits {
                max_decompressed_segment_bytes: 1024,
                ..ReaderLimits::default()
            },
        );
        let err = bomb_reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(err, ReadError::SizeLimitExceeded { .. }));

        // 2. Single-line limit triggers.
        let line_reader = SegmentReader::with_limits(
            dir.path(),
            ReaderLimits {
                max_line_bytes: 512,
                ..ReaderLimits::default()
            },
        );
        let err = line_reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(err, ReadError::SizeLimitExceeded { .. }));

        // 3. Manifest size limit triggers.
        let tiny_manifest_reader = SegmentReader::with_limits(
            dir.path(),
            ReaderLimits {
                max_manifest_bytes: 32,
                ..ReaderLimits::default()
            },
        );
        let err = tiny_manifest_reader.load_manifest(device_id).unwrap_err();
        assert!(matches!(err, ReadError::SizeLimitExceeded { .. }));

        // 4. Event-count limit triggers.
        let ev_lines: Vec<String> = (0..3)
            .map(|idx| {
                json!({
                    "id": idx,
                    "timestamp": format!("2026-11-22T09:00:0{idx}.000000000Z"),
                    "duration": 1.0,
                    "data": {"app": "A"}
                })
                .to_string()
            })
            .collect();
        let (manifest2, entry2, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &header, &ev_lines, true);
        let count_reader = SegmentReader::with_limits(
            dir.path(),
            ReaderLimits {
                max_events_per_segment: 2,
                ..ReaderLimits::default()
            },
        );
        let err = count_reader
            .read_segment(&manifest2, &bucket.id, &entry2)
            .unwrap_err();
        assert!(matches!(err, ReadError::SizeLimitExceeded { .. }));
    }

    #[test]
    fn test_bucket_created_mismatch_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-created";
        let bucket = sample_bucket("aw-watcher-window_created", "host-created");
        let reader = SegmentReader::new(dir.path());

        let mut h = base_header(device_id, &bucket, 1);
        h.bucket_created = Some(Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap());
        let (manifest, entry, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &h, &[], true);
        let err = reader
            .read_segment(&manifest, &bucket.id, &entry)
            .unwrap_err();
        assert!(matches!(
            err,
            ReadError::IdentityMismatch {
                field: "header.bucket_created",
                ..
            }
        ));

        let mut h_none = base_header(device_id, &bucket, 1);
        h_none.bucket_created = None;
        let (manifest2, entry2, _) =
            write_synthetic_segment(dir.path(), device_id, &bucket, &h_none, &[], true);
        let err = reader
            .read_segment(&manifest2, &bucket.id, &entry2)
            .unwrap_err();
        assert!(matches!(
            err,
            ReadError::IdentityMismatch {
                field: "header.bucket_created",
                ..
            }
        ));
    }

    #[test]
    fn test_iter_bucket_streams_and_read_bucket_enforces_bucket_limit() {
        let dir = tempfile::tempdir().unwrap();
        let device_id = "dev-bucket-limit";
        let bucket = sample_bucket("aw-watcher-window_lim", "lim");

        let ts1 = Utc.with_ymd_and_hms(2026, 11, 22, 8, 0, 0).unwrap();
        let mut writer = SegmentWriter::new(dir.path(), device_id, &bucket.id).unwrap();
        writer
            .write_events(
                &bucket,
                &[
                    sample_event(Some(1), ts1, 1_000, "A"),
                    sample_event(Some(2), ts1 + Duration::seconds(2), 1_000, "B"),
                ],
            )
            .unwrap();

        let mut manifest = Manifest::load_or_default(dir.path(), device_id, "lim").unwrap();
        manifest.buckets.get_mut(&bucket.id).unwrap().segments[0].first_written_at =
            Some(Utc::now() - Duration::days(2));
        manifest.save(dir.path(), device_id).unwrap();

        writer
            .write_events(
                &bucket,
                &[
                    sample_event(Some(3), ts1 + Duration::seconds(10), 1_000, "C"),
                    sample_event(Some(4), ts1 + Duration::seconds(12), 1_000, "D"),
                ],
            )
            .unwrap();

        let reader = SegmentReader::with_limits(
            dir.path(),
            ReaderLimits {
                max_events_per_segment: 2,
                max_events_per_bucket: 3,
                ..ReaderLimits::default()
            },
        );
        let loaded = reader.load_manifest(device_id).unwrap();

        let err = reader.read_bucket(&loaded, &bucket.id).unwrap_err();
        assert!(matches!(err, ReadError::SizeLimitExceeded { .. }));

        let mut streamed_ids = Vec::new();
        for item in reader.iter_bucket(&loaded, &bucket.id).unwrap() {
            let decoded = item.unwrap().into_ready().unwrap();
            assert_eq!(decoded.events.len(), 2);
            streamed_ids.extend(decoded.events.iter().filter_map(|e| e.id));
        }
        assert_eq!(streamed_ids, vec![1, 2, 3, 4]);
    }

    #[cfg(unix)]
    #[test]
    fn test_symlinked_device_dir_rejected_while_symlinked_sync_root_works() {
        use std::os::unix::fs::symlink;

        let real_root = tempfile::tempdir().unwrap();
        let link_parent = tempfile::tempdir().unwrap();
        let linked_root = link_parent.path().join("ActivityWatchSync");
        symlink(real_root.path(), &linked_root).unwrap();

        let device_id = "dev-sym-ok";
        let bucket = sample_bucket("aw-watcher-window_sym", "sym");
        let ts = Utc.with_ymd_and_hms(2026, 11, 22, 9, 0, 0).unwrap();
        let mut writer = SegmentWriter::new(&linked_root, device_id, &bucket.id).unwrap();
        writer
            .write_events(&bucket, &[sample_event(Some(1), ts, 1_000, "App")])
            .unwrap();

        let reader = SegmentReader::new(&linked_root);
        let manifest = reader.load_manifest(device_id).unwrap();
        let decoded = reader
            .read_bucket(&manifest, &bucket.id)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .into_ready()
            .unwrap();
        assert_eq!(decoded.events.len(), 1);

        let outside = tempfile::tempdir().unwrap();
        let evil_dev_dir = real_root.path().join("devices").join("dev-sym-evil");
        symlink(outside.path(), &evil_dev_dir).unwrap();
        let err = reader.load_manifest("dev-sym-evil").unwrap_err();
        assert!(
            matches!(err, ReadError::InvalidPath { .. }),
            "expected InvalidPath for symlinked device dir, got {err:?}"
        );
    }

    #[test]
    fn test_reader_is_strictly_read_only_on_missing_root() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let reader = SegmentReader::new(&missing);
        assert!(reader.discover_devices().unwrap().is_empty());
        let err = reader.load_manifest("dev-missing").unwrap_err();
        assert!(err.is_retryable());
        assert!(matches!(err, ReadError::ManifestNotFound { .. }));
        assert!(
            !missing.exists(),
            "reader must never create missing directories"
        );
    }
}
