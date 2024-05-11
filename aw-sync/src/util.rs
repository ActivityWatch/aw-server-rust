use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Returns the port of the local aw-server instance
pub fn get_server_port(testing: bool) -> Result<u16, Box<dyn Error>> {
    // TODO: get aw-server config more reliably
    let aw_server_conf = crate::dirs::get_server_config_path(testing)
        .map_err(|_| "Could not get aw-server config path")?;
    let fallback: u16 = if testing { 5666 } else { 5600 };
    let port = if aw_server_conf.exists() {
        let mut file = File::open(&aw_server_conf)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        let value: toml::Value = toml::from_str(&contents)?;
        value
            .get("port")
            .and_then(|v| v.as_integer())
            .map(|v| v as u16)
            .unwrap_or(fallback)
    } else {
        fallback
    };
    Ok(port)
}

/// Check if a directory contains a .db file
fn contains_db_file(dir: &std::path::Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry
                    .path()
                    .extension()
                    .map(|ext| ext == "db")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Check if a directory contains a subdirectory that contains a .db file
fn contains_subdir_with_db_file(dir: &std::path::Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().is_dir() && contains_db_file(&entry.path()))
        })
        .unwrap_or(false)
}

/// Return all remotes in the sync folder
/// Only returns folders that match ./{host}/{device_id}/*.db
// TODO: share logic with find_remotes and find_remotes_nonlocal
pub fn get_remotes() -> Result<Vec<String>, Box<dyn Error>> {
    let sync_root_dir = crate::dirs::get_sync_dir()?;
    fs::create_dir_all(&sync_root_dir)?;
    let hostnames = fs::read_dir(sync_root_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir() && contains_subdir_with_db_file(&entry.path()))
        .filter_map(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|os_str| os_str.to_str().map(String::from))
        })
        .collect();
    info!("Found remotes: {:?}", hostnames);
    Ok(hostnames)
}

/// Returns a list of all remote dbs
fn find_remotes(sync_directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    let dbs = fs::read_dir(sync_directory)?
        .map(|res| res.ok().unwrap().path())
        .filter(|p| p.is_dir())
        .flat_map(|d| fs::read_dir(d).unwrap())
        .map(|res| res.ok().unwrap().path())
        .filter(|path| path.extension().unwrap_or_else(|| OsStr::new("")) == "db")
        .collect();
    Ok(dbs)
}

/// Returns a list of all remotes, excluding local ones
pub fn find_remotes_nonlocal(
    sync_directory: &Path,
    device_id: &str,
    sync_db: Option<&PathBuf>,
) -> Vec<PathBuf> {
    let remotes_all = find_remotes(sync_directory).unwrap();
    remotes_all
        .into_iter()
        // Filter out own remote
        .filter(|path| {
            !(path
                .clone()
                .into_os_string()
                .into_string()
                .unwrap()
                .contains(device_id))
        })
        // If sync_db is Some, return only remotes in that path
        .filter(|path| {
            if let Some(sync_db) = sync_db {
                path.starts_with(sync_db)
            } else {
                true
            }
        })
        .collect()
}
