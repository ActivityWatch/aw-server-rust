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

/// Fetch a bucket's events via `fetch_page(end, limit)` in bounded pages
/// (newest-first, walking `end` backwards) and return the total event count and
/// duplicate ids, without ever holding the whole bucket in memory at once.
///
/// A duplicate group's members all share the same `(timestamp, duration,
/// data)`, so as long as every page contains each timestamp's run *in full*,
/// `find_duplicate_ids` can be applied per page independently — no
/// across-page bookkeeping needed.
///
/// A page is cut at the timestamp of its `PAGE_SIZE`-th event (`cut_ts`).
/// The run at `cut_ts` may continue past the fetched events, so the page is
/// only accepted once the fetch also contains an event strictly older than
/// `cut_ts` (proving the run is complete) or has exhausted the bucket; until
/// then the limit is doubled and the same window re-fetched. Only events at or
/// after `cut_ts` are processed; the next page starts at `cut_ts - 1ns`.
///
/// Runs are never re-fetched through `end = cut_ts`: the server clips event
/// durations at the `end` boundary, which would zero the durations of events
/// starting exactly there and make distinct events look like duplicates.
fn dedupe_bucket_paginated<F>(mut fetch_page: F) -> Result<(usize, Vec<i64>), Box<dyn Error>>
where
    F: FnMut(Option<DateTime<Utc>>, usize) -> Result<Vec<Event>, Box<dyn Error>>,
{
    let mut total_events = 0usize;
    let mut duplicate_ids = Vec::new();
    let mut fetch_end: Option<DateTime<Utc>> = None;

    loop {
        let mut limit = PAGE_SIZE + 1;
        let mut cut_ts: Option<DateTime<Utc>> = None;
        let (mut page, exhausted) = loop {
            let page = fetch_page(fetch_end, limit)?;
            if page.len() < limit {
                break (page, true);
            }
            let cut = *cut_ts.get_or_insert(page[PAGE_SIZE - 1].timestamp);
            if page[page.len() - 1].timestamp < cut {
                break (page, false);
            }
            limit *= 2;
        };

        if let Some(cut) = cut_ts.filter(|_| !exhausted) {
            page.retain(|e| e.timestamp >= cut);
            fetch_end = Some(cut - Duration::nanoseconds(1));
        }

        total_events += page.len();
        duplicate_ids.extend(find_duplicate_ids(&page));

        if exhausted {
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
    }
    targets.sort();

    if targets.is_empty() {
        info!("No -synced-from- buckets found, nothing to dedupe");
        return Ok(());
    }

    if buckets_filter.is_none() && !dry_run {
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

    let mut results = Vec::new();
    for bucket_id in targets {
        let (total_events, duplicate_ids) = dedupe_bucket_paginated(|end, limit| {
            client
                .get_events(&bucket_id, None, end, Some(limit as u64))
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

    /// A fake `get_events(end, limit)` following the real server contract:
    /// events with `timestamp <= end` (all, if `end` is None), ordered
    /// `starttime DESC, endtime ASC, id ASC`, truncated to `limit`, with
    /// durations clipped at `end` (an event starting exactly at `end` comes
    /// back with a zero duration).
    fn fake_fetch_page(
        all: &[Event],
        end: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<Event>, Box<dyn Error>> {
        let mut page: Vec<Event> = all
            .iter()
            .filter(|e| end.is_none_or(|end| e.timestamp <= end))
            .map(|e| match end {
                Some(end) if e.timestamp + e.duration > end => Event {
                    duration: end - e.timestamp,
                    ..e.clone()
                },
                _ => e.clone(),
            })
            .collect();
        page.sort_by_key(|e| {
            (
                std::cmp::Reverse(e.timestamp),
                e.timestamp + e.duration,
                e.id,
            )
        });
        page.truncate(limit);
        Ok(page)
    }

    fn paginated(all: &[Event]) -> (usize, Vec<i64>) {
        let (total, mut dups) =
            dedupe_bucket_paginated(|end, limit| fake_fetch_page(all, end, limit)).unwrap();
        dups.sort();
        (total, dups)
    }

    fn single_shot(all: &[Event]) -> Vec<i64> {
        let mut dups = find_duplicate_ids(all);
        dups.sort();
        dups
    }

    /// Two events with the same timestamp and data but different durations are
    /// distinct and must never be collapsed by a page boundary landing between
    /// them (re-fetching with `end = ts` would clip both to zero duration).
    #[test]
    fn distinct_durations_survive_page_boundary() {
        let all = vec![
            event(20, 500, 10, "z"),
            event(11, 200, 10, "a"),
            event(10, 200, 5, "a"),
            event(9, 100, 10, "b"),
        ];
        let (total, dups) = paginated(&all);
        assert_eq!(total, all.len());
        assert!(dups.is_empty(), "all events are distinct, got: {dups:?}");
    }

    #[test]
    fn paginated_matches_single_shot_across_page_boundaries() {
        // PAGE_SIZE is 3 under #[cfg(test)]: 7 events force multiple pages,
        // with a duplicate group at ts=400 sitting on a page boundary.
        let all = vec![
            event(20, 700, 10, "g"),
            event(19, 600, 10, "f"),
            event(18, 500, 10, "e"),
            event(12, 400, 10, "a"),
            event(11, 400, 10, "a"),
            event(10, 400, 10, "a"), // keeper: lowest id in the group
            event(1, 100, 10, "z"),
        ];
        let (total, dups) = paginated(&all);
        assert_eq!(total, all.len());
        assert_eq!(dups, vec![11, 12]);
        assert_eq!(dups, single_shot(&all));
    }

    /// A duplicate run that starts inside a page and continues past its end
    /// must still be found in full (it used to be cut at the page boundary,
    /// leaving the remainder undetected).
    #[test]
    fn run_straddling_page_boundary_is_found_in_full() {
        let mut all = vec![event(100, 900, 10, "new1"), event(101, 800, 10, "new2")];
        all.extend((1..=5).map(|id| event(id, 500, 10, "dup")));
        all.push(event(50, 100, 10, "old"));
        let (total, dups) = paginated(&all);
        assert_eq!(total, all.len());
        assert_eq!(dups, vec![2, 3, 4, 5]);
    }

    /// More same-timestamp events than fit in one page (the reviewer's
    /// "large tie" case): every extra copy must still be found.
    #[test]
    fn run_larger_than_a_page_is_found_in_full() {
        let all: Vec<Event> = (1..=10).map(|id| event(id, 500, 10, "dup")).collect();
        let (total, dups) = paginated(&all);
        assert_eq!(total, 10);
        assert_eq!(dups, (2..=10).collect::<Vec<i64>>());
    }

    #[test]
    fn empty_bucket_is_fine() {
        assert_eq!(paginated(&[]), (0, vec![]));
    }

    /// Randomised layouts (runs of varying length landing on every possible
    /// page offset) must agree exactly with the unbounded single-shot result.
    #[test]
    fn paginated_matches_single_shot_on_random_layouts() {
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: u64| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) % n
        };
        for _ in 0..200 {
            let n_events = next(40) as usize;
            let mut all = Vec::new();
            for id in 1..=n_events as i64 {
                // Spaced 1000s apart with durations <= 10s: no event ever
                // crosses a page cut, so server clipping can't alter keys.
                let ts = 1000 * (1 + next(8) as i64);
                let dur = 1 + next(3) as i64 * 5;
                let data = ["a", "b"][next(2) as usize];
                all.push(event(id, ts, dur, data));
            }
            let (total, dups) = paginated(&all);
            assert_eq!(total, all.len());
            assert_eq!(dups, single_shot(&all), "layout: {all:?}");
        }
    }
}
