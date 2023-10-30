use dirs::home_dir;
use std::fs;
use std::path::PathBuf;

// TODO: This could be refactored to share logic with aw-server/src/dirs.rs
// TODO: add proper config support
#[allow(dead_code)]
pub fn get_config_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_config_dir(Some("activitywatch"), None, false)?;
    dir.push("aw-sync");
    fs::create_dir_all(dir.clone()).expect("Unable to create config dir");
    Ok(dir)
}

pub fn get_server_config_path(testing: bool) -> Result<PathBuf, ()> {
    let dir = aw_server::dirs::get_config_dir()?;
    Ok(dir.join(if testing {
        "config-testing.toml"
    } else {
        "config.toml"
    }))
}

pub fn get_sync_dir() -> Result<PathBuf, ()> {
    // TODO: make this configurable
    home_dir().map(|p| p.join("ActivityWatchSync")).ok_or(())
}
