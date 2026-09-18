//! `aw-sync dedupe` — one-off cleanup of exact-duplicate events that
//! accumulated in `-synced-from-` buckets before #713 stopped new pull
//! passes from re-importing the resume-boundary event.
//!
//! `-synced-from-` is a reserved marker in aw-sync's own ID grammar (see
//! `sync::is_synced_bucket`); this only ever touches buckets carrying it, so
//! a user's own local (non-synced) buckets are never candidates.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::error::Error;
use std::io::{self, Write};

use aw_client_rust::blocking::AwClient;
use aw_models::Event;

/// Among `events`, return the ids of every event that is an exact duplicate
/// (same timestamp, duration and data — `Event`'s own `PartialEq`) of an
/// event with a lower id. The lowest-id copy of each group is kept.
///
/// `get_events` has no ordering guarantee callers should rely on for this
/// (it's newest-first, clipped by query range), so this sorts by id itself
/// rather than trusting call order.
fn find_duplicate_ids(events: &[Event]) -> Vec<i64> {
    let mut sorted: Vec<&Event> = events.iter().collect();
    sorted.sort_by_key(|e| e.id.unwrap_or(i64::MAX));

    let mut seen: HashMap<(i64, i64, String), ()> = HashMap::new();
    let mut duplicate_ids = Vec::new();
    for event in sorted {
        let Some(id) = event.id else { continue };
        let key = (
            event.timestamp.timestamp_nanos_opt().unwrap_or(0),
            event.duration.num_nanoseconds().unwrap_or(0),
            serde_json::to_string(&event.data).unwrap_or_default(),
        );
        match seen.entry(key) {
            Entry::Occupied(_) => duplicate_ids.push(id),
            Entry::Vacant(e) => {
                e.insert(());
            }
        }
    }
    duplicate_ids
}

pub struct BucketDedupeResult {
    pub bucket_id: String,
    pub total_events: usize,
    pub duplicate_events: usize,
}

pub fn run_dedupe(
    client: &AwClient,
    buckets_filter: Option<Vec<String>>,
    dry_run: bool,
) -> Result<(), Box<dyn Error>> {
    let buckets = client.get_buckets().map_err(|e| e.to_string())?;
    let mut targets: Vec<String> = buckets
        .values()
        .filter(|b| crate::sync::is_synced_bucket(b))
        .map(|b| b.id.clone())
        .collect();
    if let Some(filter) = &buckets_filter {
        targets.retain(|id| filter.contains(id));
        for wanted in filter {
            match buckets.get(wanted) {
                None => warn!("--bucket {wanted}: no such bucket, skipping"),
                Some(b) if !crate::sync::is_synced_bucket(b) => warn!(
                    "--bucket {wanted}: not a -synced-from- bucket, skipping (dedupe only touches synced buckets)"
                ),
                Some(_) => {}
            }
        }
    }
    targets.sort();

    if targets.is_empty() {
        info!("No -synced-from- buckets found, nothing to dedupe");
        return Ok(());
    }

    let mut results = Vec::new();
    for bucket_id in targets {
        let events = client.get_events(&bucket_id, None, None, None)?;
        let total_events = events.len();
        let duplicate_ids = find_duplicate_ids(&events);
        let duplicate_events = duplicate_ids.len();

        if duplicate_events > 0 && !dry_run {
            for id in duplicate_ids {
                client.delete_event(&bucket_id, id)?;
            }
        }

        results.push(BucketDedupeResult {
            bucket_id,
            total_events,
            duplicate_events,
        });
    }

    let mut stdout = io::stdout().lock();
    let total_duplicates: usize = results.iter().map(|r| r.duplicate_events).sum();
    for r in &results {
        if r.duplicate_events > 0 {
            writeln!(
                stdout,
                "{}: {} event(s), {} duplicate(s){}",
                r.bucket_id,
                r.total_events,
                r.duplicate_events,
                if dry_run {
                    " [dry-run, not deleted]"
                } else {
                    ""
                }
            )?;
        }
    }
    if total_duplicates == 0 {
        writeln!(stdout, "No duplicates found in {} bucket(s)", results.len())?;
    } else if dry_run {
        writeln!(
            stdout,
            "Total: {total_duplicates} duplicate(s) found across {} bucket(s) [dry-run, nothing deleted — re-run without --dry-run to delete]",
            results.len()
        )?;
    } else {
        writeln!(
            stdout,
            "Total: {total_duplicates} duplicate(s) deleted across {} bucket(s)",
            results.len()
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aw_models::Event;
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::Map;

    fn event(id: i64, ts_secs: i64, dur_secs: i64, data: &str) -> Event {
        let mut map = Map::new();
        map.insert("k".into(), serde_json::Value::String(data.into()));
        Event {
            id: Some(id),
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            duration: Duration::seconds(dur_secs),
            data: map,
        }
    }

    #[test]
    fn keeps_lowest_id_of_each_duplicate_group() {
        let events = vec![
            event(1, 100, 10, "a"),
            event(2, 100, 10, "a"), // dup of 1
            event(3, 200, 10, "a"), // distinct timestamp
            event(4, 100, 10, "a"), // dup of 1
            event(5, 100, 10, "b"), // distinct data
        ];
        let mut dups = find_duplicate_ids(&events);
        dups.sort();
        assert_eq!(dups, vec![2, 4]);
    }

    #[test]
    fn unaffected_by_input_order() {
        // get_events returns newest-first; dedupe must still keep the
        // lowest id (the original, first-imported copy) regardless of the
        // order events arrive in.
        let events = vec![
            event(4, 100, 10, "a"),
            event(2, 100, 10, "a"),
            event(1, 100, 10, "a"),
        ];
        let mut dups = find_duplicate_ids(&events);
        dups.sort();
        assert_eq!(dups, vec![2, 4]);
    }

    #[test]
    fn no_duplicates_returns_empty() {
        let events = vec![event(1, 100, 10, "a"), event(2, 200, 10, "a")];
        assert!(find_duplicate_ids(&events).is_empty());
    }
}
