use dirs::home_dir;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

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
fn default_sync_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(dirs::data_dir()
        .ok_or("Unable to read user data dir")?
        .join("activitywatch")
        .join("aw-sync"))
}

fn legacy_sync_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(home_dir()
        .ok_or("Unable to read home_dir")?
        .join("ActivityWatchSync"))
}

#[cfg(not(target_os = "android"))]
fn dir_has_entries(path: &Path) -> bool {
    fs::read_dir(path)
        .ok()
        .and_then(|mut it| it.next())
        .is_some()
}

/// Prefer an existing `~/ActivityWatchSync` so folder-sync setups
/// (Syncthing/Dropbox/etc watching that path) keep working. New installs
/// with no legacy dir use the documented data-dir location.
///
/// Do not auto-rename: a failed cross-device `rename` would leave the daemon
/// writing a fresh empty tree, and a successful one would disconnect any
/// external transport still pointed at the old path.
///
/// An empty leftover `~/ActivityWatchSync` must not displace live data
/// already in the documented directory (backup restore of an empty folder,
/// old docs creating the path after a new install has started syncing).
#[cfg(not(target_os = "android"))]
fn resolve_sync_dir(legacy: &Path, documented: &Path) -> PathBuf {
    if !legacy.exists() {
        return documented.to_path_buf();
    }
    if dir_has_entries(legacy) || !dir_has_entries(documented) {
        legacy.to_path_buf()
    } else {
        documented.to_path_buf()
    }
}

pub fn get_sync_dir() -> Result<PathBuf, Box<dyn Error>> {
    // if AW_SYNC_DIR is set, use that
    if let Ok(dir) = std::env::var("AW_SYNC_DIR") {
        return Ok(PathBuf::from(dir));
    }
    // Desktop: keep sync data out of the user's home directory and follow the
    // documented data-dir convention (ActivityWatch/activitywatch#1418).
    // If the previous default already has content, keep using it — aw-sync's
    // transport is an external folder synchronizer watching that path.
    #[cfg(not(target_os = "android"))]
    {
        Ok(resolve_sync_dir(&legacy_sync_dir()?, &default_sync_dir()?))
    }
    // Android is already app-scoped; keep the historical location there.
    #[cfg(target_os = "android")]
    {
        legacy_sync_dir()
    }
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
            "aw-sync-resolve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn prefers_existing_legacy_sync_dir() {
        let root = unique_root();
        let legacy = root.join("ActivityWatchSync");
        let documented = root.join("data").join("activitywatch").join("aw-sync");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("test.db"), b"sqlite").unwrap();

        assert_eq!(resolve_sync_dir(&legacy, &documented), legacy);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn uses_documented_dir_when_no_legacy() {
        let root = unique_root();
        let legacy = root.join("ActivityWatchSync");
        let documented = root.join("data").join("activitywatch").join("aw-sync");

        assert_eq!(resolve_sync_dir(&legacy, &documented), documented);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn prefers_legacy_even_if_documented_also_exists() {
        // Don't silently switch away from a live folder-sync path.
        let root = unique_root();
        let legacy = root.join("ActivityWatchSync");
        let documented = root.join("data").join("activitywatch").join("aw-sync");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("legacy.db"), b"1").unwrap();
        fs::create_dir_all(&documented).unwrap();
        fs::write(documented.join("new.db"), b"2").unwrap();

        assert_eq!(resolve_sync_dir(&legacy, &documented), legacy);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn empty_legacy_does_not_displace_populated_documented() {
        // A restored/recreated empty ~/ActivityWatchSync must not hide
        // remote databases already in the documented data dir.
        let root = unique_root();
        let legacy = root.join("ActivityWatchSync");
        let documented = root.join("data").join("activitywatch").join("aw-sync");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&documented).unwrap();
        fs::write(documented.join("test.db"), b"sqlite").unwrap();

        assert_eq!(resolve_sync_dir(&legacy, &documented), documented);
        let _ = fs::remove_dir_all(&root);
    }
}
