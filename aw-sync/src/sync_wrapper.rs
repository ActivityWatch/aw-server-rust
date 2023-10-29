use std::boxed::Box;
use std::error::Error;
use std::fs;
use std::net::TcpStream;

use crate::sync::{sync_run, SyncMode, SyncSpec};
use crate::util::{get_hostname, get_remotes, get_server_port};
use aw_client_rust::blocking::AwClient;

pub fn pull_all(testing: bool) -> Result<(), Box<dyn Error>> {
    let hostnames = get_remotes()?;
    for host in hostnames {
        pull(&host, testing)?
    }
    Ok(())
}

pub fn pull(host: &str, testing: bool) -> Result<(), Box<dyn Error>> {
    info!("Pulling data from sync server {}", host);

    // Port of the main server
    let port = get_server_port(testing)?;

    // Check if server is running
    if TcpStream::connect(("localhost", port)).is_err() {
        return Err(format!("Local server {}:{} not running", host, port).into());
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
        let client = AwClient::new("localhost", port.to_string().as_str(), "aw-sync");
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

pub fn push(testing: bool) -> Result<(), Box<dyn Error>> {
    let hostname = get_hostname()?;
    let port = get_server_port(testing)?.to_string();

    let sync_dir = crate::dirs::get_sync_dir()
        .map_err(|_| "Could not get sync dir")?
        .join(&hostname);

    let client = AwClient::new("localhost", port.as_str(), "aw-sync");
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
