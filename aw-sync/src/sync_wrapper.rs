use std::boxed::Box;
use std::error::Error;
use std::fs;
use std::net::TcpStream;

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
    info!("Pulling data from sync server {}", host);

    // Check if server is running
    if TcpStream::connect(client.baseurl.clone()).is_err() {
        return Err(format!("Local server {} not running", &client.baseurl).into());
    }

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

    // filter out dbs that are smaller than 50kB (workaround for trying to sync empty database
    // files that are spuriously created somewhere)
    let dbs = dbs
        .into_iter()
        .filter(|entry| entry.metadata().map(|m| m.len() > 50_000).unwrap_or(false))
        .collect::<Vec<_>>();

    // if more than one db, error
    if dbs.len() > 1 {
        return Err("More than one db found in sync folder".into());
    }
    // if no db, error
    if dbs.is_empty() {
        return Err(format!("No db found in sync folder {:?}", sync_dir).into());
    }

    for db in dbs {
        let sync_spec = SyncSpec {
            path: sync_dir.clone(),
            path_db: Some(db.path().clone()),
            buckets: Some(vec![
                format!("aw-watcher-window_{}", host),
                format!("aw-watcher-afk_{}", host),
            ]),
            start: None,
        };
        info!("Pulling data with spec {:?}", sync_spec);
        sync_run(client, &sync_spec, SyncMode::Pull)?;
    }

    Ok(())
}

pub fn push(client: &AwClient) -> Result<(), Box<dyn Error>> {
    let hostname = crate::util::get_hostname()?;
    let sync_dir = crate::dirs::get_sync_dir()
        .map_err(|_| "Could not get sync dir")?
        .join(&hostname);

    let sync_spec = SyncSpec {
        path: sync_dir,
        path_db: None,
        buckets: Some(vec![
            format!("aw-watcher-window_{}", hostname),
            format!("aw-watcher-afk_{}", hostname),
        ]),
        start: None,
    };
    sync_run(client, &sync_spec, SyncMode::Push)?;

    Ok(())
}
