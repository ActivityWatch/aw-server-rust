// Transparent compression of event data using zstd with a shared trained dictionary.
//
// ActivityWatch events are tiny (most < 128 bytes) and their redundancy is
// *across* rows: the same app names, JSON keys and window titles repeat
// thousands of times, while a single row holds little repetition for zstd to
// exploit on its own. To capture that cross-row redundancy we train one
// dictionary on a sample of the data, store it once in the database, and
// compress every row against it (the same idea as sqlite-zstd). On real data
// this reduces the stored event JSON by roughly 47%.
//
// Stored BLOB format:
//   [0xCC][u32 LE original_len][zstd frame]   -> dictionary-compressed
//   <raw UTF-8 JSON bytes>                     -> stored uncompressed
//
// A row is stored uncompressed when compression would not make it smaller (or
// when no dictionary exists yet), so the worst case is never larger than the
// plain JSON. JSON event data always starts with '{' (0x7B), so the 0xCC marker
// is unambiguous.
//
// Tradeoff: once rows are compressed, the database can only be read by a build
// with this feature enabled (and with the stored dictionary intact). That is
// why the feature is opt-in at build time.

/// Marker byte prefixed to dictionary-compressed blobs.
const COMPRESSION_MARKER: u8 = 0xCC;

/// Target size of the trained dictionary. 64 KiB was the sweet spot in
/// benchmarks (larger dictionaries started to hurt the ratio).
#[cfg(feature = "compression-zstd")]
const DICT_SIZE: usize = 64 * 1024;

/// zstd compression level. 6 is a good balance for this data: the dictionary
/// provides essentially all of the savings, and higher levels add cost for only
/// ~1% extra reduction.
#[cfg(feature = "compression-zstd")]
const COMPRESSION_LEVEL: i32 = 6;

/// Minimum number of events before it is worth training a dictionary. Below
/// this the database is small enough that the savings are negligible, and there
/// is too little data to train a good dictionary.
pub const MIN_EVENTS_TO_TRAIN: i64 = 2000;

/// Holds the reusable zstd compressor/decompressor (with the loaded
/// dictionary, if any) for the lifetime of a datastore connection. Lives on the
/// single-threaded datastore worker, so the compressor/decompressor — which
/// need `&mut` per call — are wrapped in `RefCell` and reused across calls
/// rather than reallocated each time (allocating a fresh context per row is the
/// dominant cost when compressing hundreds of thousands of tiny events).
pub struct CompressionContext {
    #[cfg(feature = "compression-zstd")]
    dict: Option<Codec>,
}

#[cfg(feature = "compression-zstd")]
struct Codec {
    compressor: std::cell::RefCell<zstd::bulk::Compressor<'static>>,
    decompressor: std::cell::RefCell<zstd::bulk::Decompressor<'static>>,
}

impl CompressionContext {
    /// A context with no dictionary: writes are stored uncompressed, reads still
    /// transparently handle both uncompressed and (if a dictionary is later
    /// available) compressed data.
    pub fn empty() -> Self {
        CompressionContext {
            #[cfg(feature = "compression-zstd")]
            dict: None,
        }
    }

    /// Build a context from raw trained-dictionary bytes. Falls back to an empty
    /// (no-compression) context if the dictionary can't be loaded.
    #[cfg(feature = "compression-zstd")]
    pub fn from_dictionary(dict_bytes: &[u8]) -> Self {
        // with_dictionary copies the dictionary into the (de)compression
        // context, so the returned values own it and are 'static.
        match (
            zstd::bulk::Compressor::with_dictionary(COMPRESSION_LEVEL, dict_bytes),
            zstd::bulk::Decompressor::with_dictionary(dict_bytes),
        ) {
            (Ok(compressor), Ok(decompressor)) => CompressionContext {
                dict: Some(Codec {
                    compressor: std::cell::RefCell::new(compressor),
                    decompressor: std::cell::RefCell::new(decompressor),
                }),
            },
            _ => {
                warn!("Failed to load compression dictionary; storing events uncompressed");
                CompressionContext::empty()
            }
        }
    }

    #[cfg(not(feature = "compression-zstd"))]
    pub fn from_dictionary(_dict_bytes: &[u8]) -> Self {
        CompressionContext {}
    }

    pub fn has_dictionary(&self) -> bool {
        #[cfg(feature = "compression-zstd")]
        {
            self.dict.is_some()
        }
        #[cfg(not(feature = "compression-zstd"))]
        {
            false
        }
    }

    /// Compress a JSON string into the stored blob representation.
    ///
    /// Never fails: if there is no dictionary, the feature is disabled, or
    /// compression would not shrink the row, the raw UTF-8 bytes are returned.
    pub fn compress(&self, json: &str) -> Vec<u8> {
        #[cfg(feature = "compression-zstd")]
        {
            if let Some(codec) = &self.dict {
                if let Ok(frame) = codec.compressor.borrow_mut().compress(json.as_bytes()) {
                    // [marker][u32 LE original length][zstd frame]
                    if 5 + frame.len() < json.len() {
                        let mut blob = Vec::with_capacity(5 + frame.len());
                        blob.push(COMPRESSION_MARKER);
                        blob.extend_from_slice(&(json.len() as u32).to_le_bytes());
                        blob.extend_from_slice(&frame);
                        return blob;
                    }
                }
            }
        }
        json.as_bytes().to_vec()
    }

    /// Decompress a stored blob back into its JSON string.
    pub fn decompress(&self, bytes: &[u8]) -> Result<String, String> {
        if bytes.first() == Some(&COMPRESSION_MARKER) {
            #[cfg(feature = "compression-zstd")]
            {
                if bytes.len() < 5 {
                    return Err("compressed blob too short to contain a length header".to_string());
                }
                let orig_len =
                    u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
                let frame = &bytes[5..];
                let codec = self
                    .dict
                    .as_ref()
                    .ok_or("event data is compressed but no dictionary is loaded")?;
                let out = codec
                    .decompressor
                    .borrow_mut()
                    .decompress(frame, orig_len)
                    .map_err(|e| format!("failed to decompress event data: {e}"))?;
                String::from_utf8(out)
                    .map_err(|e| format!("decompressed data is not valid UTF-8: {e}"))
            }
            #[cfg(not(feature = "compression-zstd"))]
            {
                Err(
                    "event data is zstd-compressed but the compression-zstd feature is disabled"
                        .to_string(),
                )
            }
        } else {
            String::from_utf8(bytes.to_vec())
                .map_err(|e| format!("event data is not valid UTF-8: {e}"))
        }
    }
}

/// Train a zstd dictionary from a set of JSON samples. Returns `None` if there
/// is not enough data to train a usable dictionary.
#[cfg(feature = "compression-zstd")]
pub fn train_dictionary(samples: &[&[u8]]) -> Option<Vec<u8>> {
    if (samples.len() as i64) < MIN_EVENTS_TO_TRAIN {
        return None;
    }
    match zstd::dict::from_samples(samples, DICT_SIZE) {
        Ok(dict) => Some(dict),
        Err(e) => {
            warn!("Failed to train zstd dictionary: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uncompressed_roundtrip() {
        // empty() context stores raw and reads it back
        let ctx = CompressionContext::empty();
        let json = r#"{"app":"Firefox","title":"Hello"}"#;
        let stored = ctx.compress(json);
        assert_eq!(stored, json.as_bytes());
        assert_eq!(ctx.decompress(&stored).unwrap(), json);
    }

    #[cfg(feature = "compression-zstd")]
    #[test]
    fn test_dictionary_roundtrip_and_size() {
        // Build a realistic corpus with heavy cross-row redundancy.
        let mut samples: Vec<String> = Vec::new();
        for i in 0..5000 {
            let app = ["Firefox", "Terminal", "Code", "Slack"][i % 4];
            samples.push(format!(
                r#"{{"app":"{app}","title":"Some window title number {}"}}"#,
                i % 50
            ));
        }
        let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_bytes()).collect();
        let dict = train_dictionary(&sample_refs).expect("training should succeed");
        let ctx = CompressionContext::from_dictionary(&dict);

        let mut total_raw = 0usize;
        let mut total_stored = 0usize;
        for s in &samples {
            let blob = ctx.compress(s);
            // roundtrip is exact
            assert_eq!(ctx.decompress(&blob).unwrap(), *s);
            total_raw += s.len();
            total_stored += blob.len();
        }
        // dictionary compression must save substantial space on this corpus
        // (without a dictionary, per-row zstd saves ~0% on data this small)
        assert!(
            total_stored * 10 < total_raw * 6,
            "expected >40% reduction, got {total_stored} vs {total_raw}"
        );
    }

    #[cfg(feature = "compression-zstd")]
    #[test]
    fn test_incompressible_row_stored_raw() {
        // A context with a dictionary still stores tiny/incompressible rows raw
        // when compression would not help, so a row is never larger than raw.
        let samples: Vec<String> = (0..2000)
            .map(|i| format!(r#"{{"app":"A","title":"{i}"}}"#))
            .collect();
        let refs: Vec<&[u8]> = samples.iter().map(|s| s.as_bytes()).collect();
        let dict = train_dictionary(&refs).expect("training should succeed");
        let ctx = CompressionContext::from_dictionary(&dict);

        let tiny = r#"{"a":1}"#;
        let blob = ctx.compress(tiny);
        assert!(blob.len() <= tiny.len());
        assert_eq!(ctx.decompress(&blob).unwrap(), tiny);
    }

    #[cfg(feature = "compression-zstd")]
    #[test]
    fn test_too_few_samples_no_dict() {
        let samples: Vec<String> = (0..10).map(|i| format!("{{\"x\":{i}}}")).collect();
        let refs: Vec<&[u8]> = samples.iter().map(|s| s.as_bytes()).collect();
        assert!(train_dictionary(&refs).is_none());
    }
}
