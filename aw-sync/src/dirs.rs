use dirs::home_dir;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
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
#[allow(dead_code)] // used by the aw-sync binary; the lib copy is unused (status.rs uses config_dir_path)
#[cfg(not(target_os = "android"))]
pub fn get_config_dir() -> Result<PathBuf, Box<dyn Error>> {
    let dir = config_dir_path()?;
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Path to aw-sync's own config dir — construction only, does not create it.
/// For read-only callers (e.g. `status`) that must never mutate the
/// filesystem just to look at it; `get_config_dir` is for the daemon path,
/// which is about to write `config.toml` there anyway.
#[cfg(not(target_os = "android"))]
pub fn config_dir_path() -> Result<PathBuf, Box<dyn Error>> {
    sync_config_dir(&aw_server::dirs::appname())
}

/// aw-sync's own settings, read from `{config_dir}/config.toml`.
///
/// `pull` controls whether the **daemon** imports peers on each pass —
/// desktop only; Android stays push-only by design
/// (ActivityWatch/aw-android#291). The one-shot `aw-sync sync` command
/// always pulls+pushes regardless of this file, and an explicit `--mode`
/// on the daemon always wins over it (ActivityWatch/aw-server-rust#714).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub pull: bool,
}

const DEFAULT_SYNC_CONFIG_TOML: &str = "\
# aw-sync config
pull = false   # default; set true to import peers from the sync folder every pass
";

/// Load `config.toml` from `dir`, writing the commented default if it does
/// not exist yet (so users find the switch on first daemon start). Returns
/// the parsed config and the path it was read from.
// dirs.rs is compiled by both lib.rs and main.rs (dual-include); the lib
// does not call this directly (status.rs uses read_sync_config), but the
// daemon binary does via its own mod dirs copy.
#[cfg_attr(not(test), allow(dead_code))]
#[cfg(not(target_os = "android"))]
pub fn load_or_create_sync_config(dir: &Path) -> Result<(SyncConfig, PathBuf), Box<dyn Error>> {
    fs::create_dir_all(dir)?;
    let path = dir.join("config.toml");
    if !path.is_file() {
        fs::write(&path, DEFAULT_SYNC_CONFIG_TOML)?;
    }
    let content = fs::read_to_string(&path)?;
    let config: SyncConfig = toml::from_str(&content)?;
    Ok((config, path))
}

/// Read-only variant for `status`: returns `(None, path)` when the file is
/// absent rather than writing the default. The daemon's `load_or_create`
/// writes on first start; the doctor should never create files.
#[cfg(not(target_os = "android"))]
pub fn read_sync_config(dir: &Path) -> Result<(Option<SyncConfig>, PathBuf), Box<dyn Error>> {
    let path = dir.join("config.toml");
    if !path.is_file() {
        return Ok((None, path));
    }
    let content = fs::read_to_string(&path)?;
    let config: SyncConfig = toml::from_str(&content)?;
    Ok((Some(config), path))
}

/// Which `SyncMode` a daemon pass should use: an explicit `--mode` always
/// wins; otherwise the config's `pull` flag picks push-only vs both.
pub fn effective_daemon_mode(
    cli_mode: Option<crate::report::SyncMode>,
    pull: bool,
) -> crate::report::SyncMode {
    cli_mode.unwrap_or(if pull {
        crate::report::SyncMode::Both
    } else {
        crate::report::SyncMode::Push
    })
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
    use super::*;

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

    // ActivityWatch/aw-server-rust#714: daemon pull is opt-in.
    #[test]
    fn effective_daemon_mode_explicit_cli_always_wins() {
        use crate::report::SyncMode;
        assert_eq!(
            effective_daemon_mode(Some(SyncMode::Pull), false),
            SyncMode::Pull
        );
        assert_eq!(
            effective_daemon_mode(Some(SyncMode::Pull), true),
            SyncMode::Pull
        );
        assert_eq!(
            effective_daemon_mode(Some(SyncMode::Both), false),
            SyncMode::Both
        );
    }

    #[test]
    fn effective_daemon_mode_no_cli_follows_config_pull_flag() {
        use crate::report::SyncMode;
        // No config.toml (or pull = false): push-only by default.
        assert_eq!(effective_daemon_mode(None, false), SyncMode::Push);
        // pull = true: daemon imports too.
        assert_eq!(effective_daemon_mode(None, true), SyncMode::Both);
    }

    #[cfg(not(target_os = "android"))]
    fn temp_sync_config_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aw-sync-config-tests-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn load_or_create_sync_config_writes_commented_default_when_missing() {
        let dir = temp_sync_config_dir("missing");
        let (config, path) = load_or_create_sync_config(&dir).unwrap();
        assert!(!config.pull, "default config must be pull = false");
        assert!(path.is_file());
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("pull = false"),
            "commented default should mention pull = false, got: {content}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn load_or_create_sync_config_respects_existing_pull_true() {
        let dir = temp_sync_config_dir("pull-true");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), "pull = true\n").unwrap();
        let (config, _path) = load_or_create_sync_config(&dir).unwrap();
        assert!(config.pull);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn read_sync_config_returns_none_when_missing_and_does_not_create_file() {
        let dir = temp_sync_config_dir("read-missing");
        let (config, path) = read_sync_config(&dir).unwrap();
        assert!(config.is_none(), "should return None when file is absent");
        assert!(!path.exists(), "read_sync_config must not create the file");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn read_sync_config_reads_existing_config() {
        let dir = temp_sync_config_dir("read-existing");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), "pull = true\n").unwrap();
        let (config, _path) = read_sync_config(&dir).unwrap();
        assert!(config.unwrap().pull, "should read pull = true");
        let _ = fs::remove_dir_all(&dir);
    }
}
