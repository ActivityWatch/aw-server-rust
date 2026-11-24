use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use crate::report::{PeerReport, SyncMode, SyncReport};
use crate::sync::{sync_run, SyncSpec};
use aw_client_rust::blocking::AwClient;

// Unused by the CLI binary since its sync pushes through `pull_all_partial`;
// still the library's pull-only entry point (Android `syncPull`).
#[allow(dead_code)]
pub fn pull_all(client: &AwClient) -> Result<SyncReport, Box<dyn Error>> {
    let (report, failure) = pull_all_partial(client)?;
    match failure {
        Some(e) => {
            // Persist the aggregate (the failures) so a total abort is
            // visible afterwards, then fail the pass.
            crate::report::persist_last_report_warn(&report);
            Err(e.into())
        }
        None => Ok(report),
    }
}

/// `pull_all`, but a total pull failure comes back next to the report
/// instead of replacing it.
///
/// A caller that pushes after pulling must still push when every peer failed
/// (ActivityWatch/aw-server-rust#875, item 8), and must keep the failed-peer
/// rows in the report it persists. `Err` is left for setup failures before
/// any peer was tried.
pub fn pull_all_partial(client: &AwClient) -> Result<(SyncReport, Option<String>), Box<dyn Error>> {
    let sync_root = crate::dirs::get_sync_dir().map_err(|_| "Could not get sync dir")?;
    let dbs = crate::util::list_remote_dbs(&sync_root)?;
    let selection = crate::util::select_remote_dbs_detailed(dbs);
    let mut report = SyncReport::new(SyncMode::Pull);
    for skipped in &selection.skipped {
        report.peers.push(PeerReport::skipped(
            skipped.db.device_id.clone(),
            skipped.db.hostname.clone(),
            skipped.db.path.clone(),
            skipped.reason.clone(),
        ));
    }
    let found: Vec<_> = selection.selected.iter().map(|d| d.path.clone()).collect();
    // No get_info() here: reqwest's client timeout is 120s, and an empty-dir
    // pass (#682) must stay a local filesystem check. `None` is unknown, not
    // `Some("")` — empty string does not unclassify entries, and leftover
    // 2-level own staging then shows up as "peer db(s) that pull did not select".
    report.capture_warnings(crate::util::pull_discovery_warnings(
        &sync_root, None, &found,
    ));
    if selection.selected.is_empty() {
        info!("No remote databases found in {:?}", sync_root);
        report.finish();
        return Ok((report, None));
    }
    info!(
        "Pulling {} remote database(s): {:?}",
        selection.selected.len(),
        selection
            .selected
            .iter()
            .map(|d| d.path.display().to_string())
            .collect::<Vec<_>>()
    );
    // Partial failure is non-fatal (one bad peer must not skip the rest).
    // Total failure must still be Err so CLI/JNI/supervisor can tell a
    // destination-down pass from a successful one.
    let mut attempted = 0usize;
    let mut succeeded = 0usize;
    let mut last_err: Option<String> = None;
    for remote in selection.selected {
        attempted += 1;
        // Per-peer isolation: a peer that fails to open must not abort the
        // pass and skip every peer after it. Bucket-level errors are already
        // non-fatal in `sync_datastores`; peer-level open failures need the
        // same warn+continue so a later skip-on-mismatch (#693) has somewhere
        // to go instead of becoming "abort pass" (#688).
        match pull_db(client, &remote.hostname, &remote.path) {
            Ok(one) => {
                succeeded += 1;
                report.merge(one);
            }
            Err(e) => {
                warn!(
                    "Skipping peer '{}' ({:?}): {e}",
                    remote.hostname, remote.path
                );
                warn!(
                    "Skipping peer '{}' ({:?}): {e}",
                    remote.hostname, remote.path
                );
                report.peers.push(PeerReport::failed(
                    remote.device_id,
                    remote.hostname,
                    remote.path,
                    e.to_string(),
                ));
                last_err = Some(e.to_string());
            }
        }
    }
    report.finish();
    if attempted > 0 && succeeded == 0 {
        let err = format!(
            "all {attempted} peers failed; last error: {}",
            last_err.as_deref().unwrap_or("unknown")
        );
        return Ok((report, Some(err)));
    }
    Ok((report, None))
}

pub fn pull(host: &str, client: &AwClient) -> Result<SyncReport, Box<dyn Error>> {
    // Path to the sync folder
    // Sync folder is structured ./{hostname}/{device_id}/sync.db
    let sync_root_dir = crate::dirs::get_sync_dir().map_err(|_| "Could not get sync dir")?;
    let sync_dir = sync_root_dir.join(host);
    let dbs = fs::read_dir(&sync_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| fs::read_dir(entry.path()))
        .filter_map(Result::ok)
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.path().is_file()
                && entry.path().extension().and_then(|os_str| os_str.to_str()) == Some("db")
        })
        .collect::<Vec<_>>();

    // if more than one db, warn and use the best candidate
    if dbs.len() > 1 {
        warn!(
            "More than one db found in sync folder for host, choosing best db {:?}",
            dbs
        );
    }

    // The preference for `sync.db` over a sibling `test.db` applies only within
    // one device folder; between folders the original largest-file rule stands.
    // Dropping the shadowed legacy candidate before comparing sizes keeps the
    // choice a total order (ActivityWatch/aw-server-rust#800).
    let candidates: Vec<(PathBuf, u64)> = dbs
        .iter()
        .map(|entry| {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            (entry.path(), size)
        })
        .collect();
    let winner = crate::util::select_best_candidate(&candidates)
        .ok_or_else(|| format!("No db found in sync folder {:?}", sync_dir))?;

    pull_db(client, host, &candidates[winner].0)
}

fn pull_db(client: &AwClient, host: &str, db_path: &Path) -> Result<SyncReport, Box<dyn Error>> {
    client.wait_for_start()?;
    let sync_root_dir = crate::dirs::get_sync_dir().map_err(|_| "Could not get sync dir")?;
    let sync_dir = sync_root_dir.join(host);
    let sync_spec = SyncSpec {
        path: sync_dir,
        path_db: Some(db_path.to_path_buf()),
        buckets: None, // Sync all buckets by default
        start: None,
    };
    sync_run(client, &sync_spec, SyncMode::Pull)
}

pub fn push(client: &AwClient) -> Result<SyncReport, Box<dyn Error>> {
    push_with_hostname(client, &client.hostname)
}

pub fn push_with_hostname(client: &AwClient, hostname: &str) -> Result<SyncReport, Box<dyn Error>> {
    let sync_dir = crate::dirs::get_sync_dir()
        .map_err(|_| "Could not get sync dir")?
        .join(hostname);

    let sync_spec = SyncSpec {
        path: sync_dir,
        path_db: None,
        buckets: None, // Sync all buckets by default
        start: None,
    };
    sync_run(client, &sync_spec, SyncMode::Push)
}

/// Keep the pull report when a full sync proceeds to its push phase.
#[cfg(any(target_os = "android", test))]
pub(crate) fn full_sync_pull_result(
    pulled: Result<(SyncReport, Option<String>), Box<dyn Error>>,
) -> (SyncReport, Option<String>) {
    let (mut report, pull_failure) = match pulled {
        Ok(pulled) => pulled,
        Err(e) => (SyncReport::new(SyncMode::Both), Some(e.to_string())),
    };
    report.mode = SyncMode::Both;
    if let Some(e) = &pull_failure {
        report.record_pull_phase_failure(e);
    }
    (report, pull_failure)
}

#[cfg(test)]
mod full_sync_tests {
    use super::*;

    #[test]
    fn failed_full_sync_reports_both_attempted_phases() {
        for pulled in [
            Err("discovery failed".into()),
            Ok((
                SyncReport::new(SyncMode::Pull),
                Some("all peers failed".into()),
            )),
        ] {
            let (mut report, failure) = full_sync_pull_result(pulled);
            assert!(failure.is_some());
            report.record_push_failure("local server unavailable");
            report.finish();
            assert_eq!(report.mode, SyncMode::Both);
            assert_eq!(serde_json::to_value(&report).unwrap()["mode"], "both");
            assert!(report
                .warnings
                .iter()
                .any(|w| w.starts_with("pull failed:")));
            assert!(report
                .warnings
                .iter()
                .any(|w| w.starts_with("push failed:")));
        }
    }
}
