use appdirs;

use std::path::PathBuf;

pub fn get_config_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_config_dir(Some("activitywatch"), None, false)?;
    dir.push("aw-server-rust");
    return Ok(dir);
}

pub fn get_data_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_data_dir(Some("activitywatch"), None, false)?;
    dir.push("aw-server-rust");
    return Ok(dir);
}

pub fn get_cache_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_cache_dir(Some("activitywatch"), None)?;
    dir.push("aw-server-rust");
    return Ok(dir);
}
