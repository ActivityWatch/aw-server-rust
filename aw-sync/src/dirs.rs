use dirs::home_dir;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

// TODO: This could be refactored to share logic with aw-server/src/dirs.rs
// TODO: add proper config support
#[allow(dead_code)]
pub fn get_config_dir() -> Result<PathBuf, Box<dyn Error>> {
    let mut dir = appdirs::user_config_dir(Some("activitywatch"), None, false)
        .map_err(|_| "Unable to read user config dir")?;
    dir.push("aw-sync");
    fs::create_dir_all(dir.clone())?;
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

pub fn get_sync_dir() -> Result<PathBuf, Box<dyn Error>> {
    // if AW_SYNC_DIR is set, use that
    if let Ok(dir) = std::env::var("AW_SYNC_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home_dir = home_dir().ok_or("Unable to read home_dir")?;
    Ok(home_dir.join("ActivityWatchSync"))
}
