use dirs::home_dir;
use std::error::Error;
use std::fs;
#[cfg(any(target_os = "android", test))]
use std::path::Path;
use std::path::PathBuf;

/// Resolve the instance profile.
/// `--profile` wins, then `AW_PROFILE`, then `--testing` → `"testing"`, else `"default"`.
#[allow(dead_code)] // used by the aw-sync binary; the lib copy is unused
pub fn resolve_profile(
    cli_profile: Option<&str>,
    testing: bool,
    env_profile: Option<&str>,
) -> String {
    if let Some(p) = cli_profile {
        if !p.is_empty() {
            return p.to_string();
        }
    }
    if let Some(p) = env_profile {
        if !p.is_empty() {
            return p.to_string();
        }
    }
    if testing {
        "testing".to_string()
    } else {
        "default".to_string()
    }
}

/// aw-sync's own config dir: `{appname}/aw-sync`.
///
/// Uses the same profile appname as aw-server so a named profile (e.g.
/// `research`) does not share prod's sync config. `testing` follows the
/// same new-root-plus-legacy-fallback rule as aw-server.
// TODO: add proper config support
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub fn get_config_dir() -> Result<PathBuf, Box<dyn Error>> {
    let dir = sync_config_dir(&aw_server::dirs::appname())?;
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Path construction only — does not create directories (so tests stay off-disk).
#[cfg(not(target_os = "android"))]
fn sync_config_dir(appname: &str) -> Result<PathBuf, Box<dyn Error>> {
    Ok(dirs::config_dir()
        .ok_or("Unable to read user config dir")?
        .join(appname)
        .join("aw-sync"))
}

/// Path to the embedded/local aw-server config. On Android this is
/// `filesDir/config.toml` (same file ConfigManager and the server use).
#[allow(dead_code)]
pub fn get_server_config_path(testing: bool) -> Result<PathBuf, ()> {
    let profile = aw_server::config::get_profile();
    // If set_profile hasn't run yet, honour the legacy testing bool so
    // `--testing` still finds the testing config (legacy or new root).
    let effective = if profile == "default" && testing {
        "testing"
    } else {
        profile
    };
    let filename = aw_server::config::config_filename(effective);
    // Desktop: honour profile isolation (`activitywatch-<profile>/aw-server-rust`).
    // Android: stay on filesDir — sibling XDG roots do not exist there, and
    // master already reads this path for the embedded server's api_key (#666).
    #[cfg(not(target_os = "android"))]
    {
        let dir = dirs::config_dir()
            .ok_or(())?
            .join(aw_server::dirs::appname_for(effective))
            .join("aw-server-rust");
        Ok(dir.join(filename))
    }
    #[cfg(target_os = "android")]
    {
        let dir = aw_server::dirs::get_config_dir()?;
        Ok(dir.join(filename))
    }
}

/// The documented default sync data location: `data_dir()/activitywatch/aw-sync`,
/// mirroring aw-server's data-dir convention (`data_dir()/activitywatch/<component>`)
/// and aw-sync's own config dir (`config_dir()/activitywatch/aw-sync`).
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
fn default_sync_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(dirs::data_dir()
        .ok_or("Unable to read user data dir")?
        .join("activitywatch")
        .join("aw-sync"))
}

pub fn get_sync_dir() -> Result<PathBuf, Box<dyn Error>> {
    // if AW_SYNC_DIR is set, use that
    if let Ok(dir) = std::env::var("AW_SYNC_DIR") {
        return Ok(PathBuf::from(dir));
    }
    // Desktop: keep sync data out of the user's home directory and follow the
    // documented data-dir convention (ActivityWatch/activitywatch#1418).
    #[cfg(not(target_os = "android"))]
    {
        default_sync_dir()
    }
    // Android is already app-scoped; keep the historical location there.
    #[cfg(target_os = "android")]
    {
        let home_dir = home_dir().ok_or("Unable to read home_dir")?;
        Ok(home_dir.join("ActivityWatchSync"))
    }
}

/// One-time migration from the legacy `~/ActivityWatchSync` location to the
/// documented data dir, so existing synced data is preserved while new installs
/// stop writing into the home directory (ActivityWatch/activitywatch#1418).
///
/// No-op when an explicit location is in effect (`AW_SYNC_DIR` / `--sync-dir`),
/// when no legacy dir exists, or after the migration has already run. If both the
/// legacy and the new location already contain data, it refuses to merge and
/// leaves both in place (a manual merge is safer than an automated one).
#[cfg(not(target_os = "android"))]
#[allow(dead_code)] // called by the aw-sync binary; unused in the lib copy
pub fn migrate_legacy_sync_dir() -> Result<(), Box<dyn Error>> {
    // Respect an explicit override: the user chose a location, don't relocate data.
    if std::env::var("AW_SYNC_DIR").is_ok() {
        return Ok(());
    }
    let legacy = home_dir()
        .ok_or("Unable to read home_dir")?
        .join("ActivityWatchSync");
    let new = default_sync_dir()?;
    migrate_legacy_sync_dir_to(&legacy, &new)
}

/// Core migration: move `legacy` to `new` when `legacy` exists and `new` is absent
/// or empty. Refuses to merge two non-empty locations and never destroys data:
/// a failed rename leaves the source in place.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
fn migrate_legacy_sync_dir_to(legacy: &PathBuf, new: &PathBuf) -> Result<(), Box<dyn Error>> {
    if !legacy.exists() {
        return Ok(());
    }
    if new.exists() && is_non_empty(new)? {
        warn!(
            "Sync data exists in both legacy {:?} and documented {:?}; not auto-merging. \
             Move what you need and delete the rest manually.",
            legacy, new
        );
        return Ok(());
    }
    // `fs::rename` replaces an empty dir target on Unix but fails on Windows,
    // so drop an empty target first.
    if new.exists() {
        fs::remove_dir(new)?;
    }
    if let Some(parent) = new.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(legacy, new) {
        Ok(()) => {
            info!("Migrated legacy sync dir {:?} -> {:?}", legacy, new);
            Ok(())
        }
        // Cross-device rename (EXDEV) or other failure: leave the data where it
        // is rather than risk losing it.
        Err(e) => {
            warn!(
                "Could not migrate legacy sync dir {:?} to {:?} ({e}); move it manually \
                 if you want the documented location.",
                legacy, new
            );
            Ok(())
        }
    }
}

#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
fn is_non_empty(dir: &PathBuf) -> Result<bool, Box<dyn Error>> {
    Ok(fs::read_dir(dir)?.next().is_some())
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
    use super::*;
    use std::path::Path;

    #[test]
    fn resolve_profile_cli_wins_over_env_and_testing() {
        assert_eq!(
            resolve_profile(Some("research"), true, Some("testing")),
            "research"
        );
    }

    #[test]
    fn resolve_profile_env_wins_over_testing() {
        assert_eq!(resolve_profile(None, true, Some("research")), "research");
    }

    #[test]
    fn resolve_profile_testing_alias_and_default() {
        assert_eq!(resolve_profile(None, true, None), "testing");
        assert_eq!(resolve_profile(None, false, None), "default");
        assert_eq!(resolve_profile(Some(""), false, Some("")), "default");
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn sync_config_dir_is_isolated_per_named_profile() {
        use aw_server::dirs::appname_for_in;
        use std::fs;
        let root = std::env::temp_dir().join(format!(
            "aw-sync-profile-tests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let data = root.join("data");
        let config = root.join("config");
        let cache = root.join("cache");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&cache).unwrap();

        let default_app = appname_for_in("default", &data, &config, &cache);
        let testing_app = appname_for_in("testing", &data, &config, &cache);
        let research_app = appname_for_in("research", &data, &config, &cache);

        assert_eq!(default_app, "activitywatch");
        // Fresh setup: testing is a sibling root, not the shared one.
        assert_eq!(testing_app, "activitywatch-testing");
        assert_eq!(research_app, "activitywatch-research");
        assert_ne!(testing_app, default_app);
        assert_ne!(research_app, default_app);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn server_config_filename_default_and_research_are_bare() {
        // Isolated roots use config.toml. Suffixed config-testing.toml is
        // only for the legacy shared-root layout (dirs.rs tests).
        assert_eq!(aw_server::config::config_filename("default"), "config.toml");
        assert_eq!(
            aw_server::config::config_filename("research"),
            "config.toml"
        );
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn server_config_path_default_is_config_toml() {
        let production = get_server_config_path(false).unwrap();
        assert!(
            production.ends_with("config.toml"),
            "default should read config.toml, got {production:?}"
        );
    }

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

    #[cfg(not(target_os = "android"))]
    fn unique_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "aw-sync-migrate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn migrates_legacy_sync_dir_into_documented_location() {
        let root = unique_root();
        let legacy = root.join("ActivityWatchSync");
        let new = root.join("data").join("activitywatch").join("aw-sync");
        fs::create_dir_all(legacy.join("host1").join("device1")).unwrap();
        fs::write(
            legacy.join("host1").join("device1").join("test.db"),
            b"sqlite",
        )
        .unwrap();

        migrate_legacy_sync_dir_to(&legacy, &new).unwrap();

        assert!(new.join("host1").join("device1").join("test.db").exists());
        assert!(!legacy.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn migration_is_noop_without_legacy_dir() {
        let root = unique_root();
        let legacy = root.join("ActivityWatchSync");
        let new = root.join("data").join("activitywatch").join("aw-sync");

        migrate_legacy_sync_dir_to(&legacy, &new).unwrap();

        assert!(!new.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn migration_refuses_to_merge_two_non_empty_dirs() {
        let root = unique_root();
        let legacy = root.join("ActivityWatchSync");
        let new = root.join("data").join("activitywatch").join("aw-sync");
        fs::create_dir_all(legacy.join("a")).unwrap();
        fs::write(legacy.join("a").join("x.db"), b"1").unwrap();
        fs::create_dir_all(new.join("b")).unwrap();
        fs::write(new.join("b").join("y.db"), b"2").unwrap();

        migrate_legacy_sync_dir_to(&legacy, &new).unwrap();

        assert!(legacy.join("a").join("x.db").exists());
        assert!(new.join("b").join("y.db").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
