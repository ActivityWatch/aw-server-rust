use std::error::Error;
use std::fs;

use crate::sync::{sync_run, SyncMode, SyncSpec};
use aw_client_rust::blocking::AwClient;

pub fn pull_all(client: &AwClient) -> Result<(), Box<dyn Error>> {
    let hostnames = crate::util::get_remotes()?;
    for host in hostnames {
        pull(&host, client)?
    }
    Ok(())
}

pub fn pull(host: &str, client: &AwClient) -> Result<(), Box<dyn Error>> {
    client.wait_for_start()?;

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

    let sync_spec = SyncSpec {
        path: sync_dir.clone(),
        path_db: Some(db.path().clone()),
        buckets: Some(vec![
            format!("aw-watcher-window_{}", host),
            format!("aw-watcher-afk_{}", host),
        ]),
        start: None,
    };
    sync_run(client, &sync_spec, SyncMode::Pull)?;

    Ok(())
}

pub fn push(client: &AwClient) -> Result<(), Box<dyn Error>> {
    let sync_dir = crate::dirs::get_sync_dir()
        .map_err(|_| "Could not get sync dir")?
        .join(&client.hostname);

    let sync_spec = SyncSpec {
        path: sync_dir,
        path_db: None,
        buckets: Some(vec![
            format!("aw-watcher-window_{}", client.hostname),
            format!("aw-watcher-afk_{}", client.hostname),
        ]),
        start: None,
    };
    sync_run(client, &sync_spec, SyncMode::Push)?;

    Ok(())
}
