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
use chrono::{DateTime, Duration, Utc};

/// Bounded page size for `dedupe_bucket_paginated`'s fetch loop, so a
/// long-lived synced bucket can't be pulled into memory in one unbounded
/// `get_events` call. Mirrors the pagination shape in `sync::sync_one`
/// (that module's `BATCH_SIZE` is private, so this is a separate constant).
#[cfg(not(test))]
const PAGE_SIZE: usize = 5000;
#[cfg(test)]
const PAGE_SIZE: usize = 3;

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

/// Fetch `bucket_id`'s events via `fetch_page` in bounded pages (newest-first,
/// walking the `end` boundary backwards) and return the total event count and
/// duplicate ids, without ever holding the whole bucket in memory at once.
///
/// A duplicate group's members all share the same `(timestamp, duration,
/// data)`, so as long as a page never splits a run of same-timestamp events,
/// every duplicate group is fully contained within one page and
/// `find_duplicate_ids` can be applied per page independently — no
/// across-page bookkeeping needed.
///
/// Boundary events are carried forward into the next page's batch rather than
/// re-fetched with `end = boundary_ts`, because the server clips event
/// durations at the `end` boundary — events starting exactly at `boundary_ts`
/// would have their durations zeroed, producing false duplicate keys.
fn dedupe_bucket_paginated<F>(mut fetch_page: F) -> Result<(usize, Vec<i64>), Box<dyn Error>>
where
    F: FnMut(Option<DateTime<Utc>>) -> Result<Vec<Event>, Box<dyn Error>>,
{
    let mut total_events = 0usize;
    let mut duplicate_ids = Vec::new();
    let mut fetch_end: Option<DateTime<Utc>> = None;
    // Boundary events popped from the end of a full page, carried into the
    // next iteration so they're processed alongside any remaining events at
    // the same timestamp — without going through a server re-fetch that would
    // clip their durations.
    let mut carry: Vec<Event> = Vec::new();

    loop {
        let fetched = fetch_page(fetch_end)?;
        let is_last_page = fetched.len() < PAGE_SIZE;

        // Combine carried boundary events (newer) with the newly fetched page
        // (older). Carry is always newer: it came from the bottom of the
        // previous full page, so timestamps in carry >= timestamps in fetched.
        let mut page = carry;
        carry = Vec::new();
        page.extend(fetched);

        if page.is_empty() {
            break;
        }

        if !is_last_page {
            // page is newest-first; page.last() = oldest event in this batch.
            // Never split a run of same-timestamp events across two pages, or a
            // duplicate group could be cut in half with its older half missed.
            let boundary_ts = page.last().unwrap().timestamp;
            let newest_ts = page.first().unwrap().timestamp;
            if newest_ts != boundary_ts {
                // Pop all events at boundary_ts into carry. Advance to one ns
                // before boundary_ts so the next server fetch excludes that
                // timestamp — carried events already hold the true-duration
                // copies from this page.
                while page.last().is_some_and(|e| e.timestamp == boundary_ts) {
                    carry.push(page.pop().unwrap());
                }
                fetch_end = Some(boundary_ts - Duration::nanoseconds(1));
            } else {
                // Entire batch shares one timestamp. Advance past it.
                // If more than PAGE_SIZE events share this timestamp, those
                // beyond what fit in this page are not deduped in this pass.
                // This is not expected for normal AW event data; a subsequent
                // run will catch any residual duplicates.
                warn!(
                    "Entire page shares timestamp {}; if more events exist at \
                     this timestamp they will not be deduped in this pass",
                    boundary_ts
                );
                fetch_end = Some(boundary_ts - Duration::nanoseconds(1));
            }
        }

        total_events += page.len();
        duplicate_ids.extend(find_duplicate_ids(&page));

        if is_last_page && carry.is_empty() {
            break;
        }
    }

    Ok((total_events, duplicate_ids))
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
    } else if !dry_run {
        // `-synced-from-` is an ID convention aw-sync itself reserves, but
        // bucket creation doesn't enforce that reservation (ActivityWatch/aw-server-rust#649
        // tracks moving provenance to bucket metadata instead of the ID string) — a
        // hand-created bucket could coincidentally match it and get deleted from
        // unattended. Require the caller to have reviewed a --dry-run report and
        // named buckets explicitly before deleting from every match at once.
        return Err(
            "Refusing to delete from every -synced-from- bucket without --bucket: run with \
             --dry-run first, review the per-bucket counts, then re-run with \
             --bucket <id1,id2,...> naming the buckets you confirmed."
                .into(),
        );
    }
    targets.sort();

    if targets.is_empty() {
        info!("No -synced-from- buckets found, nothing to dedupe");
        return Ok(());
    }

    let mut results = Vec::new();
    for bucket_id in targets {
        let (total_events, duplicate_ids) = dedupe_bucket_paginated(|end| {
            client
                .get_events(&bucket_id, None, end, Some(PAGE_SIZE as u64))
                .map_err(|e| -> Box<dyn Error> { e.into() })
        })?;
        let duplicate_events = duplicate_ids.len();

        if duplicate_events > 0 && !dry_run {
            info!(
                "Deleting from {}: {} duplicate(s)...",
                bucket_id, duplicate_events
            );
            for (i, id) in duplicate_ids.into_iter().enumerate() {
                client.delete_event(&bucket_id, id)?;
                if (i + 1) % 10_000 == 0 {
                    info!("  {}/{} deleted", i + 1, duplicate_events);
                }
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

    /// A fake `get_events(end)`: returns events with `timestamp <= end`
    /// (or all, if `end` is None), newest-first, with server-side duration
    /// clipping applied — same contract as the real client. Events that start
    /// at exactly `end` have their duration clipped to zero (the real server
    /// clips event end-times at the query boundary).
    fn fake_fetch_page(
        all: &[Event],
        end: Option<DateTime<Utc>>,
    ) -> Result<Vec<Event>, Box<dyn Error>> {
        let mut page: Vec<Event> = all
            .iter()
            .filter(|e| end.is_none_or(|end| e.timestamp <= end))
            .map(|e| {
                // Simulate server-side duration clipping: events whose
                // duration extends past `end` are clipped to fit.
                if let Some(end) = end {
                    let event_end = e.timestamp + e.duration;
                    if event_end > end {
                        let clipped_dur = end - e.timestamp;
                        return Event {
                            duration: if clipped_dur < Duration::zero() {
                                Duration::zero()
                            } else {
                                clipped_dur
                            },
                            ..e.clone()
                        };
                    }
                }
                e.clone()
            })
            .collect();
        page.sort_by_key(|e| std::cmp::Reverse(e.id));
        page.truncate(PAGE_SIZE);
        Ok(page)
    }

    /// Regression test for the duration-clipping P1 bug: a boundary event's
    /// duration must not be changed by server-side clipping when the event is
    /// carried into the next batch. Two events with same timestamp but
    /// different durations are distinct; neither should be flagged as a
    /// duplicate of the other.
    #[test]
    fn carry_preserves_duration_across_page_boundary() {
        // PAGE_SIZE=3. Events at ts=200 straddle a page boundary:
        //   page 1 fetched (end=None): ids [20, 11, 10] — ts=200 events fill the boundary
        //   Without the fix, id=10 and id=11 would be re-fetched with end=200ns,
        //   clipping their durations to 0 and making them look like duplicates
        //   of id=9 (which also has ts=200 and dur=0 in the clipped view).
        //
        // The two events at ts=200 have DIFFERENT durations (5s vs 10s) — they
        // are genuinely distinct and must not be deleted.
        let all = vec![
            event(20, 500, 10, "z"),
            event(11, 200, 10, "a"), // distinct: dur=10s — must NOT be flagged
            event(10, 200, 5, "a"),  // distinct: dur=5s  — must NOT be flagged
            event(9, 100, 10, "b"),
        ];
        let (total, dups) = dedupe_bucket_paginated(|end| fake_fetch_page(&all, end)).unwrap();
        assert_eq!(total, all.len());
        assert!(
            dups.is_empty(),
            "expected no duplicates (all events are distinct), got: {dups:?}"
        );
    }

    #[test]
    fn paginated_matches_single_shot_across_page_boundaries() {
        // PAGE_SIZE is 3 under #[cfg(test)]. 7 events, forcing 3 pages, with a
        // duplicate group (ids 11 and 12, both dups of the lowest-id copy 10)
        // that straddles where a naive page cut would land — the boundary-safe
        // pop must keep the whole tied run together in one page.
        let all = vec![
            event(20, 700, 10, "g"),
            event(19, 600, 10, "f"),
            event(18, 500, 10, "e"),
            event(12, 400, 10, "a"), // dup of 10 (lowest id is keeper)
            event(11, 400, 10, "a"), // dup of 10
            event(10, 400, 10, "a"), // keeper: lowest id in the group
            event(1, 100, 10, "z"),
        ];
        let (total, mut dups) = dedupe_bucket_paginated(|end| fake_fetch_page(&all, end)).unwrap();
        dups.sort();
        assert_eq!(total, all.len());
        assert_eq!(dups, vec![11, 12]);

        // Must match the unbounded single-shot result exactly.
        let mut single_shot = find_duplicate_ids(&all);
        single_shot.sort();
        assert_eq!(dups, single_shot);
    }
}
