use dirs::home_dir;
use std::error::Error;
use std::fs;
#[cfg(any(target_os = "android", test))]
use std::path::Path;
use std::path::PathBuf;

// TODO: This could be refactored to share logic with aw-server/src/dirs.rs
// TODO: add proper config support
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub fn get_config_dir() -> Result<PathBuf, Box<dyn Error>> {
    let dir = dirs::config_dir()
        .ok_or("Unable to read user config dir")?
        .join("activitywatch")
        .join("aw-sync");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Path to the embedded/local aw-server config. On Android this is
/// `filesDir/config.toml` (same file ConfigManager and the server use).
#[allow(dead_code)]
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

/// SyncInterface.kt sets `XDG_DATA_HOME=$filesDir/data` before `loadLibrary`.
/// `libaw_sync.so` is a separate cdylib from `libaw_server.so`, so
/// `RustInterface.setDataDir` does not update this library's `ANDROID_DATA_DIR`.
/// Parent of that env var is the app filesDir (release, `.debug`, work profile).
#[cfg(any(target_os = "android", test))]
pub(crate) fn files_dir_from_xdg_data_home(xdg_data_home: &Path) -> Option<PathBuf> {
    if xdg_data_home.file_name() != Some(std::ffi::OsStr::new("data")) {
        return None;
    }
    let parent = xdg_data_home.parent()?;
    if parent.as_os_str().is_empty() || parent == Path::new("/") {
        return None;
    }
    Some(parent.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::files_dir_from_xdg_data_home;
    use std::path::Path;

    #[test]
    fn xdg_data_home_parent_is_files_dir() {
        let debug = Path::new("/data/user/0/net.activitywatch.android.debug/files/data");
        assert_eq!(
            files_dir_from_xdg_data_home(debug).as_deref(),
            Some(Path::new(
                "/data/user/0/net.activitywatch.android.debug/files"
            ))
        );

        let work_profile = Path::new("/data/user/10/net.activitywatch.android/files/data");
        assert_eq!(
            files_dir_from_xdg_data_home(work_profile).as_deref(),
            Some(Path::new("/data/user/10/net.activitywatch.android/files"))
        );
    }

    #[test]
    fn rejects_non_data_leaf_and_root() {
        assert!(files_dir_from_xdg_data_home(Path::new("/data")).is_none());
        assert!(files_dir_from_xdg_data_home(Path::new("/")).is_none());
        assert!(files_dir_from_xdg_data_home(Path::new("data")).is_none());
        assert!(files_dir_from_xdg_data_home(Path::new("/tmp/config")).is_none());
    }
}
