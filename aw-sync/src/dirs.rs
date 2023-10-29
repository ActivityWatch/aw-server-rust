use dirs::home_dir;
use std::fs;
use std::path::PathBuf;

// TODO: This could be refactored to share logic with aw-server/src/dirs.rs
pub fn get_config_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_config_dir(Some("activitywatch"), None, false)?;
    dir.push("aw-sync");
    fs::create_dir_all(dir.clone()).expect("Unable to create config dir");
    Ok(dir)
}

pub fn get_server_config_path(testing: bool) -> Result<PathBuf, ()> {
    home_dir()
        .map(|dir| {
            dir.join(".config/activitywatch/aw-server-rust")
                .join(if testing {
                    "config-testing.toml"
                } else {
                    "config.toml"
                })
        })
        .ok_or(())
}

pub fn get_sync_dir() -> Result<PathBuf, ()> {
    home_dir().map(|p| p.join("ActivityWatchSync")).ok_or(())
}
