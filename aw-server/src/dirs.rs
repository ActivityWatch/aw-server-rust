use std::fs;
use std::path::{Path, PathBuf};

#[cfg(target_os = "android")]
use std::sync::Mutex;

#[cfg(target_os = "android")]
lazy_static! {
    static ref ANDROID_DATA_DIR: Mutex<PathBuf> = Mutex::new(PathBuf::from(
        "/data/user/0/net.activitywatch.android/files"
    ));
}

const DEFAULT_APPNAME: &str = "activitywatch";
const TESTING_PROFILE: &str = "testing";
const TESTING_APPNAME: &str = "activitywatch-testing";

/// Filenames that mark a machine as still using the pre-profile shared-root
/// testing layout (ActivityWatch/activitywatch#1399). Keep this list specific:
/// a false positive would pin a fresh install to the legacy layout forever.
/// Identical to the python list in aw-core so both sides agree on disk state.
const LEGACY_TESTING_FILENAME_MARKERS: &[&str] = &[
    "peewee-sqlite-testing",
    "sqlite-testing",
    "settings-testing",
    "config-testing",
    "-testing.db",
    "-testing.toml",
    "-testing.json",
    "_testing_",
];

/// Platform "appname" root for the current profile.
///
/// Named profiles use a sibling root (`activitywatch-research`, …). `default`
/// keeps the bare `activitywatch` root. `testing` follows the new-root-plus
/// legacy-fallback rule from ActivityWatch/activitywatch#1399 — see
/// [`using_legacy_testing_root`].
#[cfg(not(target_os = "android"))]
pub fn appname() -> String {
    appname_for(crate::config::get_profile())
}

/// Platform appname for a given profile, observing on-disk state for `testing`.
#[cfg(not(target_os = "android"))]
pub fn appname_for(profile: &str) -> String {
    match platform_roots() {
        Some((data, config, cache)) => appname_for_in(profile, &data, &config, &cache),
        None => appname_for_in(profile, Path::new(""), Path::new(""), Path::new("")),
    }
}

/// Pure appname resolution against explicit XDG-style parent dirs (testable).
pub fn appname_for_in(profile: &str, data: &Path, config: &Path, cache: &Path) -> String {
    if profile.is_empty() || profile == "default" {
        return DEFAULT_APPNAME.to_string();
    }
    if profile == TESTING_PROFILE && using_legacy_testing_root_in(profile, data, config, cache) {
        return DEFAULT_APPNAME.to_string();
    }
    format!("{DEFAULT_APPNAME}-{profile}")
}

fn platform_roots() -> Option<(PathBuf, PathBuf, PathBuf)> {
    Some((dirs::data_dir()?, dirs::config_dir()?, dirs::cache_dir()?))
}

fn is_legacy_testing_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    LEGACY_TESTING_FILENAME_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

fn dir_has_legacy_testing_file(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            && is_legacy_testing_filename(&entry.file_name().to_string_lossy())
    })
}

/// Walk `activitywatch/` plus one extra level (`activitywatch/aw-server-rust/`).
fn legacy_testing_artifacts_in_app_root(app_root: &Path) -> bool {
    if !app_root.is_dir() {
        return false;
    }
    if dir_has_legacy_testing_file(app_root) {
        return true;
    }
    let Ok(entries) = fs::read_dir(app_root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_dir() && dir_has_legacy_testing_file(&path)
    })
}

fn new_testing_root_exists_in(data: &Path, config: &Path, cache: &Path) -> bool {
    [data, config, cache]
        .iter()
        .any(|root| root.join(TESTING_APPNAME).is_dir())
}

fn legacy_testing_artifacts_exist_in(data: &Path, config: &Path, cache: &Path) -> bool {
    [data, config, cache]
        .iter()
        .any(|root| legacy_testing_artifacts_in_app_root(&root.join(DEFAULT_APPNAME)))
}

/// Testing-root resolution against explicit parent dirs (testable).
///
/// Rule (ActivityWatch/activitywatch#1399), identical on python and rust:
///
/// 1. If `activitywatch-testing/` already exists: use it (new layout).
/// 2. Else if legacy testing artifacts exist in the bare `activitywatch/`
///    root: stay in legacy mode (old paths, old filenames).
/// 3. Else (fresh setup): create and use `activitywatch-testing/`.
pub fn using_legacy_testing_root_in(
    profile: &str,
    data: &Path,
    config: &Path,
    cache: &Path,
) -> bool {
    if profile != TESTING_PROFILE {
        return false;
    }
    if new_testing_root_exists_in(data, config, cache) {
        return false;
    }
    legacy_testing_artifacts_exist_in(data, config, cache)
}

/// Whether `profile=testing` should stay on the shared `activitywatch` root.
pub fn using_legacy_testing_root(profile: &str) -> bool {
    match platform_roots() {
        Some((data, config, cache)) => {
            using_legacy_testing_root_in(profile, &data, &config, &cache)
        }
        None => false,
    }
}

/// `"-testing"` only when testing data still shares the default root.
///
/// Isolated profile roots (including new-style `activitywatch-testing/`) use
/// bare filenames: the directory already isolates. Suffixed names remain only
/// in legacy mode so existing `sqlite-testing.db` files keep working.
pub fn legacy_testing_suffix(profile: &str) -> &'static str {
    if using_legacy_testing_root(profile) {
        "-testing"
    } else {
        ""
    }
}

fn db_filename_for_legacy(legacy: bool) -> &'static str {
    if legacy {
        "sqlite-testing.db"
    } else {
        "sqlite.db"
    }
}

fn config_filename_for_legacy(legacy: bool) -> &'static str {
    if legacy {
        "config-testing.toml"
    } else {
        "config.toml"
    }
}

/// Database filename for a profile. Isolated roots use `sqlite.db`; legacy
/// testing keeps `sqlite-testing.db`.
pub fn db_filename(profile: &str) -> String {
    db_filename_for_legacy(using_legacy_testing_root(profile)).to_string()
}

/// Config filename for a profile. Isolated roots use `config.toml`; legacy
/// testing keeps `config-testing.toml`.
pub fn config_filename(profile: &str) -> String {
    config_filename_for_legacy(using_legacy_testing_root(profile)).to_string()
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

/// Data dir for an explicit profile (does not depend on the process-global
/// `OnceLock`). Creates the directory. Used by `db_path` so a caller asking
/// for `research` cannot land in the default root just because `set_profile`
/// has not run yet.
#[cfg(not(target_os = "android"))]
fn get_data_dir_for(profile: &str) -> Result<PathBuf, ()> {
    let dir = dirs::data_dir()
        .ok_or(())?
        .join(appname_for(profile))
        .join("aw-server-rust");
    fs::create_dir_all(&dir).expect("Unable to create data dir");
    Ok(dir)
}

#[cfg(target_os = "android")]
fn get_data_dir_for(_profile: &str) -> Result<PathBuf, ()> {
    get_data_dir()
}

pub fn db_path(profile: &str) -> Result<PathBuf, ()> {
    let mut db_path = get_data_dir_for(profile)?;
    db_path.push(db_filename(profile));
    Ok(db_path)
}

#[cfg(target_os = "android")]
pub fn set_android_data_dir(path: &str) {
    let mut android_data_dir = ANDROID_DATA_DIR.lock().unwrap();
    *android_data_dir = PathBuf::from(path);
}

#[cfg(test)]
fn fake_roots() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir()
        .join("aw-testing-root-fallback")
        .join(uuid::Uuid::new_v4().to_string());
    let data = root.join("data");
    let config = root.join("config");
    let cache = root.join("cache");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&cache).unwrap();
    (root, data, config, cache)
}

#[cfg(test)]
fn plant_legacy_testing_db(data: &Path) -> PathBuf {
    let aw_server = data.join("activitywatch").join("aw-server-rust");
    fs::create_dir_all(&aw_server).unwrap();
    let marker = aw_server.join("sqlite-testing.db");
    fs::write(&marker, b"").unwrap();
    marker
}

#[cfg(not(target_os = "android"))]
#[test]
fn test_appname_root_isolation() {
    let (_root, data, config, cache) = fake_roots();
    assert_eq!(
        appname_for_in("default", &data, &config, &cache),
        "activitywatch"
    );
    // Fresh setup: testing uses the isolated sibling root.
    assert_eq!(
        appname_for_in("testing", &data, &config, &cache),
        "activitywatch-testing"
    );
    assert_eq!(
        appname_for_in("research", &data, &config, &cache),
        "activitywatch-research"
    );
    assert_eq!(
        appname_for_in("my-profile", &data, &config, &cache),
        "activitywatch-my-profile"
    );
    let _ = fs::remove_dir_all(_root);
}

#[test]
fn test_testing_root_fresh_setup_uses_new_root() {
    let (_root, data, config, cache) = fake_roots();
    assert!(!using_legacy_testing_root_in(
        "testing", &data, &config, &cache
    ));
    assert_eq!(
        appname_for_in("testing", &data, &config, &cache),
        "activitywatch-testing"
    );
    let _ = fs::remove_dir_all(_root);
}

#[test]
fn test_testing_root_legacy_artifacts_keep_shared_root() {
    let (_root, data, config, cache) = fake_roots();
    plant_legacy_testing_db(&data);
    assert!(using_legacy_testing_root_in(
        "testing", &data, &config, &cache
    ));
    assert_eq!(
        appname_for_in("testing", &data, &config, &cache),
        "activitywatch"
    );
    assert!(!data.join("activitywatch-testing").exists());
    let _ = fs::remove_dir_all(_root);
}

#[test]
fn test_testing_root_new_root_wins_over_legacy_artifacts() {
    let (_root, data, config, cache) = fake_roots();
    plant_legacy_testing_db(&data);
    fs::create_dir_all(data.join("activitywatch-testing")).unwrap();
    assert!(!using_legacy_testing_root_in(
        "testing", &data, &config, &cache
    ));
    assert_eq!(
        appname_for_in("testing", &data, &config, &cache),
        "activitywatch-testing"
    );
    let _ = fs::remove_dir_all(_root);
}

#[test]
fn test_config_testing_toml_is_a_legacy_marker() {
    let (_root, data, config, cache) = fake_roots();
    let cfg = config.join("activitywatch");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(cfg.join("config-testing.toml"), b"").unwrap();
    assert!(using_legacy_testing_root_in(
        "testing", &data, &config, &cache
    ));
    assert_eq!(
        appname_for_in("testing", &data, &config, &cache),
        "activitywatch"
    );
    let _ = fs::remove_dir_all(_root);
}

#[test]
fn test_named_profile_never_uses_legacy_root() {
    let (_root, data, config, cache) = fake_roots();
    plant_legacy_testing_db(&data);
    assert!(!using_legacy_testing_root_in(
        "research", &data, &config, &cache
    ));
    assert_eq!(
        appname_for_in("research", &data, &config, &cache),
        "activitywatch-research"
    );
    let _ = fs::remove_dir_all(_root);
}

#[cfg(test)]
fn db_filename_in(profile: &str, data: &Path, config: &Path, cache: &Path) -> &'static str {
    db_filename_for_legacy(using_legacy_testing_root_in(profile, data, config, cache))
}

#[cfg(test)]
fn config_filename_in(profile: &str, data: &Path, config: &Path, cache: &Path) -> &'static str {
    config_filename_for_legacy(using_legacy_testing_root_in(profile, data, config, cache))
}

#[test]
fn test_filenames_bare_except_legacy_testing() {
    assert_eq!(db_filename_for_legacy(false), "sqlite.db");
    assert_eq!(db_filename_for_legacy(true), "sqlite-testing.db");
    assert_eq!(config_filename_for_legacy(false), "config.toml");
    assert_eq!(config_filename_for_legacy(true), "config-testing.toml");
}

#[test]
fn test_filenames_follow_disk_state_rule() {
    let (_root, data, config, cache) = fake_roots();
    assert_eq!(
        db_filename_in("testing", &data, &config, &cache),
        "sqlite.db"
    );
    assert_eq!(
        config_filename_in("testing", &data, &config, &cache),
        "config.toml"
    );
    assert_eq!(
        db_filename_in("research", &data, &config, &cache),
        "sqlite.db"
    );

    plant_legacy_testing_db(&data);
    assert_eq!(
        db_filename_in("testing", &data, &config, &cache),
        "sqlite-testing.db"
    );
    assert_eq!(
        config_filename_in("testing", &data, &config, &cache),
        "config-testing.toml"
    );
    // Named profiles stay bare even when legacy testing artifacts exist.
    assert_eq!(
        db_filename_in("research", &data, &config, &cache),
        "sqlite.db"
    );
    assert_eq!(
        config_filename_in("research", &data, &config, &cache),
        "config.toml"
    );

    fs::create_dir_all(data.join("activitywatch-testing")).unwrap();
    assert_eq!(
        db_filename_in("testing", &data, &config, &cache),
        "sqlite.db"
    );
    assert_eq!(
        config_filename_in("testing", &data, &config, &cache),
        "config.toml"
    );
    let _ = fs::remove_dir_all(_root);
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
    // Do not call db_path("testing"): on a fresh CI machine that would create
    // ~/.local/share/activitywatch-testing and pin later tests to the new root.
    db_path("default").unwrap();
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
