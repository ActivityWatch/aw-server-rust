#[cfg(not(target_os="android"))]
use appdirs;

use std::fs;
use std::path::PathBuf;

#[cfg(target_os="android")]
static mut ANDROID_DATA_DIR: Option<PathBuf> = None;

pub fn get_config_dir() -> Result<PathBuf, ()> {
    #[cfg(not(target_os="android"))]
    {
        let mut dir;
        dir = appdirs::user_config_dir(Some("activitywatch"), None, false)?;
        dir.push("aw-server-rust");
        return Ok(dir);
    }

    #[cfg(target_os="android")]
    {
        return Err(());
    }
}

pub fn get_data_dir() -> Result<PathBuf, ()> {
    #[cfg(not(target_os="android"))]
    {
        let mut dir = appdirs::user_data_dir(Some("activitywatch"), None, false)?;
        dir.push("aw-server-rust");
        return Ok(dir);
    }

    #[cfg(target_os="android")]
    unsafe {
         return match ANDROID_DATA_DIR {
            Some(ref path) => Ok(path.to_path_buf()),
            None => Err(())
        }
    }
}

pub fn get_cache_dir() -> Result<PathBuf, ()> {
    #[cfg(not(target_os="android"))]
    {
        let mut dir = appdirs::user_cache_dir(Some("activitywatch"), None)?;
        dir.push("aw-server-rust");
        return Ok(dir);
    }

    #[cfg(target_os="android")]
    {
        return Err(());
    }
}

pub fn db_path() -> PathBuf {
    let mut db_path = get_data_dir().unwrap();
    fs::create_dir_all(db_path.clone()).unwrap();
    #[cfg(debug_assertions)]
    db_path.push("sqlite-testing.db");
    #[cfg(not(debug_assertions))]
    db_path.push("sqlite.db");
    return db_path;
}

#[cfg(target_os="android")]
pub fn set_android_data_dir(path: &str) {
    unsafe {
        ANDROID_DATA_DIR = Some(PathBuf::from(path));
    }
}
