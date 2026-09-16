//! `aw-sync status` — a doctor command for the sync folder.
//!
//! Read-only: never creates staging databases. Peers are the `RemoteDb`
//! list `pull_all` uses (2-level leftovers and 3-level hosts); unrecognised
//! entries sit on top.

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::io::{self, Write};

use aw_client_rust::blocking::AwClient;
use chrono::{DateTime, Utc};

use crate::util::{
    inspect_sync_db, scan_sync_dir, DbInspect, SyncDirEntry, SyncEntryKind, SyncLayout,
};

pub fn run_status(
    client: &AwClient,
    host: &str,
    port: u16,
    profile: &str,
) -> Result<(), Box<dyn Error>> {
    let report = collect_status(client, host, port, profile)?;
    let mut stdout = io::stdout().lock();
    write!(stdout, "{report}")?;
    Ok(())
}

pub fn collect_status(
    client: &AwClient,
    host: &str,
    port: u16,
    profile: &str,
) -> Result<String, Box<dyn Error>> {
    let sync_dir = crate::dirs::get_sync_dir()?;
    let server_info = client.get_info().ok();
    let local_hostname = server_info
        .as_ref()
        .map(|i| i.hostname.clone())
        .unwrap_or_else(|| client.hostname.clone());
    let device_id = server_info.as_ref().map(|i| i.device_id.clone());
    let server_ok = server_info.is_some();

    let local_buckets = if server_ok {
        client.get_buckets().unwrap_or_default()
    } else {
        Default::default()
    };
    let imported_origins: HashSet<String> = local_buckets
        .keys()
        .filter_map(|id| {
            id.split("-synced-from-")
                .nth(1)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .chain(local_buckets.values().filter_map(|b| {
            b.data
                .get("$aw.sync.origin")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }))
        .collect();
    let local_newest = local_buckets
        .values()
        .filter_map(|b| b.metadata.end.or(b.last_updated))
        .max();

    let mut inspected: Vec<(SyncDirEntry, Option<Result<DbInspect, String>>)> = Vec::new();
    for entry in scan_sync_dir(&sync_dir, device_id.as_deref())? {
        let peek = entry.db_path.as_ref().map(|p| inspect_sync_db(p));
        inspected.push((entry, peek));
    }

    let mut out = String::new();
    out.push_str("aw-sync status\n");
    out.push_str("==============\n");
    out.push_str(&format!("sync dir: {}\n", sync_dir.display()));
    out.push_str(&format!("profile: {profile}\n"));
    out.push_str(&format!("local hostname: {local_hostname}\n"));
    match &device_id {
        Some(id) => out.push_str(&format!("device_id: {id}\n")),
        None => out.push_str("device_id: (unknown — could not reach local server)\n"),
    }
    out.push_str(&format!(
        "server: {host}:{port}  {}\n",
        if server_ok {
            "reachable"
        } else {
            "UNREACHABLE"
        }
    ));
    out.push('\n');

    if inspected.is_empty() {
        out.push_str("Entries: (none)\n");
    } else {
        out.push_str("Entries:\n");
        for (entry, peek) in &inspected {
            out.push_str(&entry.diagnostic_line());
            out.push('\n');
            if let Some(result) = peek {
                match result {
                    Ok(info) => out.push_str(&format_inspect(info, &imported_origins)),
                    Err(e) => out.push_str(&format!("    inspect: {e}\n")),
                }
            }
        }
    }

    let warnings = collect_warnings(&inspected, local_newest.as_ref(), &imported_origins);
    out.push('\n');
    if warnings.is_empty() {
        out.push_str("Warnings: none\n");
    } else {
        out.push_str("Warnings:\n");
        for w in warnings {
            out.push_str(&format!("  ! {w}\n"));
        }
    }

    Ok(out)
}

fn format_inspect(info: &DbInspect, imported_origins: &HashSet<String>) -> String {
    let mut s = String::new();
    if let Some(host) = &info.hostname {
        s.push_str(&format!("    hostname (buckets): {host}\n"));
    }
    s.push_str(&format!(
        "    buckets: {}  events: {}  newest: {}\n",
        info.bucket_count,
        info.event_count,
        info.newest_event
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "-".to_string())
    ));
    if let Some(host) = &info.hostname {
        let imported = imported_origins.contains(host);
        s.push_str(&format!(
            "    imported locally: {}\n",
            if imported { "yes" } else { "no" }
        ));
    }
    s
}

fn collect_warnings(
    inspected: &[(SyncDirEntry, Option<Result<DbInspect, String>>)],
    local_newest: Option<&DateTime<Utc>>,
    imported_origins: &HashSet<String>,
) -> Vec<String> {
    let mut warnings = Vec::new();

    let has_two = inspected
        .iter()
        .any(|(e, _)| e.layout == Some(SyncLayout::TwoLevel) && e.db_path.is_some());
    let has_three = inspected
        .iter()
        .any(|(e, _)| e.layout == Some(SyncLayout::ThreeLevel) && e.db_path.is_some());
    if has_two && has_three {
        warnings.push(
            "sync folder has both 2-level ({device_id}/*.db) and 3-level \
             ({hostname}/{device_id}/*.db) databases; new daemon pushes should use the \
             3-level host layout (ActivityWatch/aw-server-rust#682 / #685)"
                .to_string(),
        );
    }

    let mut by_device: BTreeMap<String, Vec<&SyncDirEntry>> = BTreeMap::new();
    for (entry, _) in inspected {
        if let (Some(did), Some(_)) = (&entry.device_id, &entry.db_path) {
            by_device.entry(did.clone()).or_default().push(entry);
        }
    }
    for (did, group) in &by_device {
        if group.len() > 1 {
            let folders: Vec<String> = group
                .iter()
                .map(|e| {
                    e.hostname_folder
                        .clone()
                        .unwrap_or_else(|| e.path.display().to_string())
                })
                .collect();
            warnings.push(format!(
                "device_id {did} appears under {} folders: {} — pulling both can truncate history (ActivityWatch/aw-server-rust#683)",
                group.len(),
                folders.join(", ")
            ));
        }
    }

    for (entry, peek) in inspected {
        let Some(Ok(info)) = peek else { continue };
        if let (Some(folder), Some(host)) = (&entry.hostname_folder, &info.hostname) {
            if folder != host {
                warnings.push(format!(
                    "folder name '{folder}' ≠ bucket hostname '{host}' ({})",
                    entry.path.display()
                ));
            }
        }
        if entry.kind == SyncEntryKind::Peer {
            if let Some(host) = &info.hostname {
                if !imported_origins.contains(host) {
                    warnings.push(format!(
                        "peer {} ({}) has not been imported locally",
                        entry.device_id.as_deref().unwrap_or("?"),
                        host
                    ));
                }
            }
        }
        if entry.kind == SyncEntryKind::OwnStaging {
            if let (Some(staging_newest), Some(local)) = (info.newest_event, local_newest) {
                if staging_newest < *local {
                    warnings.push(format!(
                        "own staging newest event ({}) is older than local server newest ({}) — this device may not be publishing",
                        staging_newest.to_rfc3339(),
                        local.to_rfc3339()
                    ));
                }
            }
        }
        if info.buckets.iter().any(|b| b.id.contains("-synced-from-")) {
            warnings.push(format!(
                "{} still contains re-exported …-synced-from-… buckets (leftover from before ActivityWatch/aw-server-rust#648)",
                entry.path.display()
            ));
        }
    }

    if inspected
        .iter()
        .any(|(e, _)| e.db_path.as_ref().is_some_and(|p| p.ends_with("test.db")))
    {
        warnings.push(
            "staging databases are still named test.db — leftover test scaffolding in a directory users are told to inspect".to_string(),
        );
    }

    warnings
}
