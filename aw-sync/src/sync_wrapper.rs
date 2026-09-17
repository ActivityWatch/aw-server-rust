use std::error::Error;
use std::fs;
use std::path::Path;

use crate::report::{PeerReport, SyncMode, SyncReport};
use crate::sync::{sync_run, SyncSpec};
use aw_client_rust::blocking::AwClient;

pub fn pull_all(client: &AwClient) -> Result<SyncReport, Box<dyn Error>> {
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
        return Ok(report);
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
    for remote in selection.selected {
        match pull_db(client, &remote.hostname, &remote.path) {
            Ok(one) => report.merge(one),
            Err(e) => {
                report.peers.push(PeerReport::failed(
                    remote.device_id,
                    remote.hostname,
                    remote.path,
                    e.to_string(),
                ));
                report.finish();
                // Persist the aggregate (earlier peers + this failure), not
                // only the phase-local report from the failing `sync_run`.
                crate::report::persist_last_report_warn(&report);
                return Err(e);
            }
        }
    }
    report.finish();
    Ok(report)
}

pub fn pull(host: &str, client: &AwClient) -> Result<SyncReport, Box<dyn Error>> {
    // Path to the sync folder
    // Sync folder is structured ./{hostname}/{device_id}/test.db
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

    // if more than one db, warn and use the largest one
    if dbs.len() > 1 {
        warn!(
            "More than one db found in sync folder for host, choosing largest db {:?}",
            dbs
        );
    }

    let db = dbs
        .into_iter()
        .max_by_key(|entry| entry.metadata().map(|m| m.len()).unwrap_or(0))
        .ok_or_else(|| format!("No db found in sync folder {:?}", sync_dir))?;

    pull_db(client, host, &db.path())
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
