use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(not(target_os = "android"))]
use std::fs;

#[cfg(target_os = "android")]
use std::sync::Mutex;

#[cfg(target_os = "android")]
lazy_static! {
    static ref ANDROID_DATA_DIR: Mutex<PathBuf> = Mutex::new(PathBuf::from(
        "/data/user/0/net.activitywatch.android/files"
    ));
}

#[cfg(not(target_os = "android"))]
pub fn get_config_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_config_dir(Some("activitywatch"), None, false)?;
    dir.push("aw-server-rust");
    fs::create_dir_all(dir.clone()).expect("Unable to create config dir");
    Ok(dir)
}

#[cfg(target_os = "android")]
pub fn get_config_dir() -> Result<PathBuf, ()> {
    panic!("not implemented on Android");
}

#[cfg(not(target_os = "android"))]
pub fn get_data_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_data_dir(Some("activitywatch"), None, false)?;
    dir.push("aw-server-rust");
    fs::create_dir_all(dir.clone()).expect("Unable to create data dir");
    Ok(dir)
}

#[cfg(target_os = "android")]
pub fn get_data_dir() -> Result<PathBuf, ()> {
    return Ok(ANDROID_DATA_DIR.lock().unwrap().to_path_buf());
}

#[cfg(not(target_os = "android"))]
pub fn get_cache_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_cache_dir(Some("activitywatch"), None)?;
    dir.push("aw-server-rust");
    fs::create_dir_all(dir.clone()).expect("Unable to create cache dir");
    Ok(dir)
}

#[cfg(target_os = "android")]
pub fn get_cache_dir() -> Result<PathBuf, ()> {
    panic!("not implemented on Android");
}

#[cfg(not(target_os = "android"))]
pub fn get_log_dir() -> Result<PathBuf, ()> {
    let mut dir = appdirs::user_log_dir(Some("activitywatch"), None)?;
    dir.push("aw-server-rust");
    fs::create_dir_all(dir.clone()).expect("Unable to create log dir");
    Ok(dir)
}

#[cfg(target_os = "android")]
pub fn get_log_dir() -> Result<PathBuf, ()> {
    panic!("not implemented on Android");
}

pub fn db_path(testing: bool) -> Result<PathBuf, ()> {
    let mut db_path = get_data_dir()?;
    if testing {
        db_path.push("sqlite-testing.db");
    } else {
        db_path.push("sqlite.db");
    }
    Ok(db_path)
}

#[cfg(target_os = "android")]
pub fn set_android_data_dir(path: &str) {
    let mut android_data_dir = ANDROID_DATA_DIR.lock().unwrap();
    *android_data_dir = PathBuf::from(path);
}

#[test]
fn test_get_dirs() {
    #[cfg(target_os = "android")]
    set_android_data_dir("/test");

    get_cache_dir().unwrap();
    get_log_dir().unwrap();
    db_path(true).unwrap();
    db_path(false).unwrap();
}

// The appdirs implementation of site_data_dir is broken on computers which has flatpak installed
// as flatpak adds its ~/.local/share/flatpak/exports/share directory first and then doesn't care
// about the rest of the paths.
// This is a rewrite of site_data_dir which takes the first folder which exists out of all folders
// in XDG_DATA_DIRS
// TODO: Should we talk to upstream about this? This changes the behavior quite a lot so maybe they
// don't want this change?
fn site_data_dir(app: Option<&str>, _: Option<&str>) -> Result<PathBuf, ()> {
    // Iterate over all XDG_DATA_DIRS and return first match that exists
    let joined = match env::var_os("XDG_DATA_DIRS") {
        // If $XDG_DATA_DIRS is either not set or empty, a value equal to /usr/local/share/:/usr/share/ should be used.
        // https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html
        Some(path) => {
            if path.is_empty() {
                OsString::from("/usr/local/share:/usr/share")
            } else {
                path
            }
        }
        None => OsString::from("/usr/local/share:/usr/share"),
    };

    for mut data_dir in env::split_paths(&joined) {
        if let Some(app) = app {
            data_dir.push(app);
        }
        if !data_dir.is_dir() {
            continue;
        }
        return Ok(data_dir);
    }
    // If no dirs exists in XDG_DATA_DIRS, fallback to /usr/local/share
    let default = "/usr/local/share";
    let mut data_dir = PathBuf::new();
    data_dir.push(default);
    if let Some(app) = app {
        data_dir.push(app);
    }

    if data_dir.is_dir() {
        Ok(data_dir)
    } else {
        Err(())
    }
}

pub fn get_asset_path() -> PathBuf {
    use std::env::current_exe;

    // Search order for asset path is:
    // 1. ./aw-webui/dist
    // 2. $current_exe_dir/aw_server_rust/static
    //    NOTE: Slightly different for .app bundles on macOS
    // 3. $XDG_DATA_DIR/aw_server_rust/static
    // 4. (fallback) ./aw-webui/dist

    // cargo_dev_path
    // (for running with cargo run)
    let cargo_dev_path = PathBuf::from("./aw-webui/dist/");
    if cargo_dev_path.as_path().exists() {
        return cargo_dev_path;
    }

    info!("Cannot find assets {:?}", cargo_dev_path.as_path());

    // current_exe_path
    // (for self-contained deployed binaries)
    if let Ok(mut current_exe_path) = current_exe() {
        current_exe_path.pop(); // remove name of executable
        current_exe_path.push("./static/");
        if current_exe_path.as_path().exists() {
            return current_exe_path;
        }
    }

    // For .app bundles on macOS
    //
    // On macOS, the executable location is ActivityWatch.app/Contents/MacOS/aw-server-rust,
    // and the webui location is ActivityWatch.app/Contents/Resources/aw_server_rust/static.
    if let Ok(mut current_exe_path) = current_exe() {
        current_exe_path.pop(); // remove name of executable
        current_exe_path.pop(); // step up into the Contents directory
        current_exe_path.push("Resources/aw_server_rust/static/");
        if current_exe_path.as_path().exists() {
            return current_exe_path;
        }
    }

    // usr_path
    // (for linux usr installs)
    if let Ok(mut usr_path) = site_data_dir(Some("aw-server"), None) {
        usr_path.push("static");
        if usr_path.as_path().exists() {
            return usr_path;
        }
    }

    warn!("Unable to find an aw-webui asset path which exists, falling back to ./aw-webui/dist");
    cargo_dev_path
}
