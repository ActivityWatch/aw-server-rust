// The public dir helpers use `Result<_, ()>` and are part of the crate API.
#![allow(clippy::result_unit_err)]

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
    Some((user_data_root()?, user_config_root()?, dirs::cache_dir()?))
}

/// Parent dir for per-user data (`<root>/<appname>/<module>`).
///
/// On Windows this is `%LOCALAPPDATA%`, not the Roaming `%APPDATA%` that
/// `dirs::data_dir()` returns. That matches the `appdirs` crate used before
/// #562 (`roaming = false`) and the python modules (platformdirs), so
/// existing installs keep finding their data.
pub fn user_data_root() -> Option<PathBuf> {
    if cfg!(windows) {
        dirs::data_local_dir()
    } else {
        dirs::data_dir()
    }
}

/// Parent dir for per-user config. `%LOCALAPPDATA%` on Windows, see
/// [`user_data_root`].
pub fn user_config_root() -> Option<PathBuf> {
    if cfg!(windows) {
        dirs::data_local_dir()
    } else {
        dirs::config_dir()
    }
}

/// `<root>/<appname>/<module>` for data/config, creating it.
///
/// On Windows, first moves over anything v0.14.0 wrote to the Roaming
/// `%APPDATA%` equivalent (see [`migrate_misplaced_dir`]).
#[cfg(not(target_os = "android"))]
pub fn module_dir(root: PathBuf, appname: &str, module: &str) -> PathBuf {
    let dir = root.join(appname).join(module);
    #[cfg(target_os = "windows")]
    if let Some(roaming) = dirs::data_dir() {
        migrate_misplaced_dir(&dir, &roaming.join(appname).join(module));
    }
    fs::create_dir_all(&dir).expect("Unable to create dir");
    dir
}

/// Group key for files that must move together: a SQLite database and its
/// `-wal`/`-shm`/`-journal` siblings share the name up to `.db`.
fn migration_group(name: &str) -> &str {
    match name.find(".db") {
        Some(i) => &name[..i + 3],
        None => name,
    }
}

/// Move entries from `misplaced` into `target`, never overwriting.
///
/// Entries are moved per group (see [`migration_group`]): if `target`
/// already has any member of a group, the whole group stays put and a
/// warning is logged, so a database is never mixed with another one's WAL.
/// A group that fails to move part-way is rolled back. `misplaced` is
/// removed if it ends up empty.
///
/// Used on Windows to recover data that v0.14.0 put under Roaming
/// `%APPDATA%` instead of `%LOCALAPPDATA%` (ActivityWatch/aw-server-rust#562).
pub fn migrate_misplaced_dir(target: &Path, misplaced: &Path) {
    let Ok(entries) = fs::read_dir(misplaced) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if names.is_empty() {
        let _ = fs::remove_dir(misplaced);
        return;
    }
    names.sort();
    if let Err(e) = fs::create_dir_all(target) {
        warn!("Could not create {target:?}, leaving {misplaced:?} in place: {e}");
        return;
    }
    let existing: Vec<String> = fs::read_dir(target)
        .map(|it| {
            it.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();

    let mut groups: Vec<(&str, Vec<&String>)> = Vec::new();
    for name in &names {
        let key = migration_group(name);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(name),
            None => groups.push((key, vec![name])),
        }
    }

    for (key, members) in groups {
        if existing.iter().any(|e| migration_group(e) == key) {
            warn!(
                "Not migrating {:?}: {target:?} already has {key}. \
                 Both copies are kept; merge them manually if needed.",
                misplaced.join(key)
            );
            continue;
        }
        let mut moved: Vec<&String> = Vec::new();
        let mut failed = None;
        for name in &members {
            match fs::rename(misplaced.join(name), target.join(name)) {
                Ok(()) => moved.push(name),
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        match failed {
            None => info!("Migrated {:?} to {target:?}", misplaced.join(key)),
            Some(e) => {
                for name in moved {
                    let _ = fs::rename(target.join(name), misplaced.join(name));
                }
                warn!(
                    "Could not migrate {:?} to {target:?}: {e}",
                    misplaced.join(key)
                );
            }
        }
    }
    // Only succeeds if everything moved.
    let _ = fs::remove_dir(misplaced);
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
    Ok(module_dir(
        user_config_root().ok_or(())?,
        &appname(),
        "aw-server-rust",
    ))
}

#[cfg(target_os = "android")]
pub fn get_config_dir() -> Result<PathBuf, ()> {
    Ok(ANDROID_DATA_DIR.lock().unwrap().to_path_buf())
}

#[cfg(not(target_os = "android"))]
pub fn get_data_dir() -> Result<PathBuf, ()> {
    Ok(module_dir(
        user_data_root().ok_or(())?,
        &appname(),
        "aw-server-rust",
    ))
}

#[cfg(target_os = "android")]
pub fn get_data_dir() -> Result<PathBuf, ()> {
    return Ok(ANDROID_DATA_DIR.lock().unwrap().to_path_buf());
}

#[cfg(not(target_os = "android"))]
pub fn get_cache_dir() -> Result<PathBuf, ()> {
    // Windows: %LOCALAPPDATA%\<appname>\Cache\<module>, as with `appdirs`
    // before #562 (`dirs::cache_dir()` is the bare %LOCALAPPDATA%, which would
    // put the cache in the data dir).
    #[cfg(windows)]
    let root = dirs::data_local_dir()
        .ok_or(())?
        .join(appname())
        .join("Cache");
    #[cfg(not(windows))]
    let root = dirs::cache_dir().ok_or(())?.join(appname());
    let dir = root.join("aw-server-rust");
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
pub fn get_log_dir(_module: &str) -> Result<PathBuf, ()> {
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
    Ok(module_dir(
        user_data_root().ok_or(())?,
        &appname_for(profile),
        "aw-server-rust",
    ))
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

#[cfg(test)]
fn migration_dirs() -> (PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir()
        .join("aw-windows-dir-migration")
        .join(uuid::Uuid::new_v4().to_string());
    let target = root
        .join("local")
        .join("activitywatch")
        .join("aw-server-rust");
    let misplaced = root
        .join("roaming")
        .join("activitywatch")
        .join("aw-server-rust");
    fs::create_dir_all(&misplaced).unwrap();
    (root, target, misplaced)
}

#[test]
fn test_migration_group_keeps_sqlite_siblings_together() {
    assert_eq!(migration_group("sqlite.db"), "sqlite.db");
    assert_eq!(migration_group("sqlite.db-wal"), "sqlite.db");
    assert_eq!(migration_group("sqlite.db-shm"), "sqlite.db");
    assert_eq!(
        migration_group("sqlite-testing.db-wal"),
        "sqlite-testing.db"
    );
    assert_eq!(migration_group("config.toml"), "config.toml");
}

#[test]
fn test_migrate_moves_everything_into_missing_target() {
    let (root, target, misplaced) = migration_dirs();
    fs::write(misplaced.join("sqlite.db"), b"db").unwrap();
    fs::write(misplaced.join("sqlite.db-wal"), b"wal").unwrap();
    fs::write(misplaced.join("config.toml"), b"cfg").unwrap();
    migrate_misplaced_dir(&target, &misplaced);
    assert_eq!(fs::read(target.join("sqlite.db")).unwrap(), b"db");
    assert_eq!(fs::read(target.join("sqlite.db-wal")).unwrap(), b"wal");
    assert_eq!(fs::read(target.join("config.toml")).unwrap(), b"cfg");
    assert!(!misplaced.exists(), "emptied source dir should be removed");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_migrate_never_overwrites_or_mixes_databases() {
    let (root, target, misplaced) = migration_dirs();
    fs::create_dir_all(&target).unwrap();
    // Target has an older database (no WAL); misplaced has a newer one + WAL.
    fs::write(target.join("sqlite.db"), b"old").unwrap();
    fs::write(misplaced.join("sqlite.db"), b"new").unwrap();
    fs::write(misplaced.join("sqlite.db-wal"), b"new-wal").unwrap();
    fs::write(misplaced.join("device_id"), b"id").unwrap();
    migrate_misplaced_dir(&target, &misplaced);
    assert_eq!(fs::read(target.join("sqlite.db")).unwrap(), b"old");
    assert!(
        !target.join("sqlite.db-wal").exists(),
        "a WAL must never be moved next to another database"
    );
    assert_eq!(fs::read(misplaced.join("sqlite.db")).unwrap(), b"new");
    assert_eq!(
        fs::read(misplaced.join("sqlite.db-wal")).unwrap(),
        b"new-wal"
    );
    // Non-conflicting entries still move.
    assert_eq!(fs::read(target.join("device_id")).unwrap(), b"id");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_migrate_noop_without_misplaced_dir() {
    let (root, target, misplaced) = migration_dirs();
    fs::remove_dir_all(&misplaced).unwrap();
    migrate_misplaced_dir(&target, &misplaced);
    assert!(!target.exists());
    let _ = fs::remove_dir_all(root);
}

/// Expected per-user roots, derived from the environment rather than the
/// `dirs` crate, so the pins below fail if a dependency swap or bump moves
/// them (as happened in #562: `appdirs` → `dirs` silently moved Windows from
/// `%LOCALAPPDATA%` to Roaming `%APPDATA%`).
///
/// Returns (data, config, log, cache) parents for the default profile.
/// These are the paths documented at
/// <https://docs.activitywatch.net/en/latest/directories.html>. Keep in sync
/// with that page and the sibling pins (change them together):
/// - aw-core `tests/test_dirs_pinned.py` (Python modules; on Windows one
///   extra `activitywatch` level, since platformdirs uses appname as author)
/// - aw-tauri `src-tauri/src/dirs.rs` `test_default_paths_are_pinned`
/// - `aw-datastore/src/legacy_import.rs` `test_legacy_dbfile_path_is_pinned`
#[cfg(all(test, not(target_os = "android")))]
fn expected_default_dirs() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    #[cfg(target_os = "windows")]
    {
        let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap());
        let app = local.join("activitywatch");
        (
            app.clone(),
            app.clone(),
            app.join("Logs"),
            app.join("Cache"),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let support = home.join("Library/Application Support/activitywatch");
        (
            support.clone(),
            support,
            home.join("Library/Logs/activitywatch"),
            home.join("Library/Caches/activitywatch"),
        )
    }
    #[cfg(target_os = "linux")]
    {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let xdg = |var: &str, fallback: &str| {
            std::env::var(var)
                .ok()
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        (
            xdg("XDG_DATA_HOME", ".local/share").join("activitywatch"),
            xdg("XDG_CONFIG_HOME", ".config").join("activitywatch"),
            xdg("XDG_CACHE_HOME", ".cache").join("activitywatch/log"),
            xdg("XDG_CACHE_HOME", ".cache").join("activitywatch"),
        )
    }
}

/// Pins the on-disk locations of existing installs. If this fails, users'
/// data is about to be orphaned: do not update the expectations without a
/// migration (see [`migrate_misplaced_dir`]).
#[cfg(not(target_os = "android"))]
#[test]
fn test_default_paths_are_pinned() {
    let (data, config, log, cache) = expected_default_dirs();
    assert_eq!(get_data_dir().unwrap(), data.join("aw-server-rust"));
    assert_eq!(get_config_dir().unwrap(), config.join("aw-server-rust"));
    assert_eq!(
        get_log_dir("aw-server-rust").unwrap(),
        log.join("aw-server-rust")
    );
    assert_eq!(get_cache_dir().unwrap(), cache.join("aw-server-rust"));
    assert_eq!(
        db_path("default").unwrap(),
        data.join("aw-server-rust").join("sqlite.db")
    );
}
