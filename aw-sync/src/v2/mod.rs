//! aw-sync v2: immutable JSONL+zstd segment format
//!
//! Layout under the sync root:
//!   devices/{device_id}/manifest.json
//!   devices/{device_id}/{bucket_slug}.{gen:08d}.jsonl.zst
//!
//! Gated by the `sync-v2` feature flag. No existing sync code paths are
//! modified; this module is entirely additive.

pub mod manifest;
pub mod segment;

pub use manifest::Manifest;
pub use segment::SegmentWriter;

/// Maximum format version this build understands.
pub const MAX_V: u32 = 1;
