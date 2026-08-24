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

/// Platform "appname" root for the current profile.
///
/// `default` and `testing` keep the bare `activitywatch` root: the legacy
/// per-file suffixes (`sqlite-testing.db`, `config-testing.toml`, port 5666)
/// already separate those two, and changing their root would orphan existing
/// installs. Any other profile gets its own sibling root, which isolates
/// config, data, cache and logs — and everything nested under them — in one
/// place, with no per-module changes.
#[cfg(not(target_os = "android"))]
fn appname() -> String {
    appname_for(crate::config::get_profile())
}

#[cfg(not(target_os = "android"))]
fn appname_for(profile: &str) -> String {
    if profile == "default" || profile == "testing" {
        "activitywatch".to_string()
    } else {
        format!("activitywatch-{profile}")
    }
}

#[cfg(not(target_os = "android"))]
pub fn get_config_dir() -> Result<PathBuf, ()> {
    let dir = dirs::config_dir()
        .ok_or(())?
        .join(appname())
        .join("aw-server-rust");
    fs::create_dir_all(&dir).expect("Unable to create config dir");
    Ok(dir)
}

#[cfg(target_os = "android")]
pub fn get_config_dir() -> Result<PathBuf, ()> {
    Ok(ANDROID_DATA_DIR.lock().unwrap().to_path_buf())
}

#[cfg(not(target_os = "android"))]
pub fn get_data_dir() -> Result<PathBuf, ()> {
    let dir = dirs::data_dir()
        .ok_or(())?
        .join(appname())
        .join("aw-server-rust");
    fs::create_dir_all(&dir).expect("Unable to create data dir");
    Ok(dir)
}

#[cfg(target_os = "android")]
pub fn get_data_dir() -> Result<PathBuf, ()> {
    return Ok(ANDROID_DATA_DIR.lock().unwrap().to_path_buf());
}

#[cfg(not(target_os = "android"))]
pub fn get_cache_dir() -> Result<PathBuf, ()> {
    let dir = dirs::cache_dir()
        .ok_or(())?
        .join(appname())
        .join("aw-server-rust");
    fs::create_dir_all(&dir).expect("Unable to create cache dir");
    Ok(dir)
}

#[cfg(target_os = "android")]
pub fn get_cache_dir() -> Result<PathBuf, ()> {
    panic!("not implemented on Android");
}

#[cfg(not(target_os = "android"))]
pub fn get_log_dir(module: &str) -> Result<PathBuf, ()> {
    let dir = get_user_log_dir()?.join(module);
    fs::create_dir_all(&dir).expect("Unable to create log dir");
    Ok(dir)
}

/// Returns the platform-appropriate log directory for ActivityWatch.
///
/// Replicates the behavior of the old `appdirs::user_log_dir("activitywatch")`:
/// - Linux:   ~/.cache/activitywatch/log/
/// - macOS:   ~/Library/Logs/activitywatch/
/// - Windows: {LOCALAPPDATA}\activitywatch\Logs\
#[cfg(target_os = "linux")]
fn get_user_log_dir() -> Result<PathBuf, ()> {
    Ok(dirs::cache_dir().ok_or(())?.join(appname()).join("log"))
}

#[cfg(target_os = "macos")]
fn get_user_log_dir() -> Result<PathBuf, ()> {
    Ok(dirs::home_dir()
        .ok_or(())?
        .join("Library")
        .join("Logs")
        .join(appname()))
}

#[cfg(target_os = "windows")]
fn get_user_log_dir() -> Result<PathBuf, ()> {
    Ok(dirs::data_local_dir()
        .ok_or(())?
        .join(appname())
        .join("Logs"))
}

#[cfg(target_os = "android")]
pub fn get_log_dir(module: &str) -> Result<PathBuf, ()> {
    panic!("not implemented on Android");
}

/// Validate a profile name: lowercase alphanumerics plus `-` and `_`, max 32
/// chars, must start with a letter or digit. Returns Err(message) on failure.
pub fn validate_profile(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("profile name must not be empty".into());
    }
    if name.len() > 32 {
        return Err(format!(
            "profile name too long ({} chars, max 32)",
            name.len()
        ));
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err(format!(
            "profile name must start with a letter or digit, got '{first}'"
        ));
    }
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
            return Err(format!("invalid character '{c}' in profile name"));
        }
    }
    if name != name.to_lowercase() {
        return Err("profile name must be lowercase".into());
    }
    Ok(())
}

pub fn db_path(profile: &str) -> Result<PathBuf, ()> {
    let mut db_path = get_data_dir()?;
    if profile == "default" {
        db_path.push("sqlite.db");
    } else {
        db_path.push(format!("sqlite-{profile}.db"));
    }
    Ok(db_path)
}

#[cfg(target_os = "android")]
pub fn set_android_data_dir(path: &str) {
    let mut android_data_dir = ANDROID_DATA_DIR.lock().unwrap();
    *android_data_dir = PathBuf::from(path);
}

#[cfg(not(target_os = "android"))]
#[test]
fn test_appname_root_isolation() {
    // default and testing keep the legacy bare root — existing installs
    // must not be orphaned by this change.
    assert_eq!(appname_for("default"), "activitywatch");
    assert_eq!(appname_for("testing"), "activitywatch");

    // any other profile gets its own sibling root
    assert_eq!(appname_for("research"), "activitywatch-research");
    assert_eq!(appname_for("my-profile"), "activitywatch-my-profile");
}

#[test]
fn test_db_path_suffix_rule() {
    // default → no suffix (legacy unsuffixed path)
    let p = db_path("default").unwrap();
    assert!(
        p.ends_with("sqlite.db"),
        "default should be sqlite.db, got {p:?}"
    );

    // testing → -testing suffix (legacy path preserved)
    let p = db_path("testing").unwrap();
    assert!(
        p.ends_with("sqlite-testing.db"),
        "testing should be sqlite-testing.db, got {p:?}"
    );

    // custom profile → -<profile> suffix
    let p = db_path("research").unwrap();
    assert!(
        p.ends_with("sqlite-research.db"),
        "research should be sqlite-research.db, got {p:?}"
    );
}

#[test]
fn test_validate_profile() {
    assert!(validate_profile("default").is_ok());
    assert!(validate_profile("testing").is_ok());
    assert!(validate_profile("research").is_ok());
    assert!(validate_profile("my-profile").is_ok());
    assert!(validate_profile("profile_1").is_ok());

    assert!(validate_profile("").is_err());
    assert!(
        validate_profile("Research").is_err(),
        "uppercase should be rejected"
    );
    assert!(validate_profile("-bad").is_err(), "must start with alnum");
    assert!(validate_profile("bad name").is_err(), "spaces not allowed");
    assert!(
        validate_profile("a/b").is_err(),
        "path separator not allowed"
    );
    assert!(validate_profile(&"a".repeat(33)).is_err(), "too long");
}

#[test]
fn test_get_dirs() {
    #[cfg(target_os = "android")]
    set_android_data_dir("/test");

    get_cache_dir().unwrap();
    get_log_dir("aw-server-rust").unwrap();
    db_path("testing").unwrap();
    db_path("default").unwrap();
    db_path("research").unwrap();
}

#[test]
#[cfg(not(target_os = "android"))]
fn test_log_dir_has_log_component() {
    let log_dir = get_log_dir("aw-server-rust").unwrap();
    let path_str = log_dir.to_string_lossy();

    // The log path must contain a log-specific subdirectory, not just the cache dir.
    // This guards against the regression from PR #562 where /log was dropped.
    #[cfg(target_os = "linux")]
    assert!(
        path_str.contains("activitywatch/log/"),
        "Linux log path should contain activitywatch/log/, got: {}",
        path_str
    );

    #[cfg(target_os = "macos")]
    assert!(
        path_str.contains("Library/Logs/activitywatch"),
        "macOS log path should use Library/Logs, got: {}",
        path_str
    );

    #[cfg(target_os = "windows")]
    assert!(
        path_str.contains("activitywatch\\Logs\\") || path_str.contains("activitywatch/Logs/"),
        "Windows log path should contain activitywatch/Logs, got: {}",
        path_str
    );
}
