use super::dirs::get_server_config_path;
use std::boxed::Box;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::{fs, path::PathBuf};

pub fn get_hostname() -> Result<String, Box<dyn Error>> {
    let hostname = gethostname::gethostname()
        .into_string()
        .map_err(|_| "Failed to convert hostname to string")?;
    Ok(hostname)
}

/// Returns the port of the local aw-server instance
pub fn get_server_port(testing: bool) -> Result<u16, Box<dyn Error>> {
    // TODO: get aw-server config more reliably
    let aw_server_conf =
        get_server_config_path(testing).map_err(|_| "Could not get aw-server config path")?;
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

/// Return all remotes in the sync folder
pub fn get_remotes() -> Result<Vec<String>, Box<dyn Error>> {
    let sync_root_dir = crate::dirs::get_sync_dir().map_err(|_| "Could not get sync dir")?;
    let hostnames = fs::read_dir(&sync_root_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
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
