use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

// Only used by the binary (main.rs) and the Android entrypoint, so these are
// dead code in the plain library build.
#[allow(dead_code)]
pub struct ServerConfig {
    pub port: u16,
    pub api_key: Option<String>,
}

#[allow(dead_code)]
impl ServerConfig {
    pub fn default_for(testing: bool) -> Self {
        Self {
            port: if testing { 5666 } else { 5600 },
            api_key: None,
        }
    }
}

/// Returns the settings aw-sync needs from the selected aw-server config.
///
/// Also used on Android: the embedded server writes `config.toml` under
/// `filesDir`, and `get_client()` in `android.rs` must send the same
/// `[auth].api_key` or `/api/0/buckets` returns 401 (aw-android#247).
#[allow(dead_code)]
pub fn get_server_config(
    testing: bool,
    config_override: Option<&Path>,
) -> Result<ServerConfig, Box<dyn Error>> {
    let path = match config_override {
        Some(path) => path.to_path_buf(),
        None => crate::dirs::get_server_config_path(testing)
            .map_err(|_| "Could not get aw-server config path")?,
    };
    let default = ServerConfig::default_for(testing);
    if !path.exists() {
        return Ok(default);
    }

    let mut contents = String::new();
    File::open(path)?.read_to_string(&mut contents)?;
    let value: toml::Value = toml::from_str(&contents)?;
    let port = value
        .get("port")
        .and_then(|v| v.as_integer())
        .and_then(|v| u16::try_from(v).ok())
        .unwrap_or(default.port);
    let api_key = value
        .get("auth")
        .and_then(|a| a.get("api_key"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    Ok(ServerConfig { port, api_key })
}

/// Local config must never be read for a caller-selected remote target.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Add URL brackets around bare IPv6 literals.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub fn host_for_url(host: &str) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]"),
        _ => host.to_string(),
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::{get_server_config, host_for_url, is_loopback_host};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reads_port_and_api_key_from_config_override() {
        let config_path = std::env::temp_dir().join(format!(
            "aw-sync-config-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(
            &config_path,
            "port = 5611\n[auth]\napi_key = \"custom-key\"\n",
        )
        .unwrap();

        let config = get_server_config(false, Some(&config_path)).unwrap();

        fs::remove_file(config_path).unwrap();
        assert_eq!(config.port, 5611);
        assert_eq!(config.api_key.as_deref(), Some("custom-key"));
    }

    #[test]
    fn missing_config_override_uses_defaults() {
        let config_path = std::env::temp_dir().join(format!(
            "missing-aw-sync-config-{}.toml",
            std::process::id()
        ));
        let _ = fs::remove_file(&config_path);

        let production = get_server_config(false, Some(&config_path)).unwrap();
        let testing = get_server_config(true, Some(&config_path)).unwrap();

        assert_eq!(production.port, 5600);
        assert!(production.api_key.is_none());
        assert_eq!(testing.port, 5666);
        assert!(testing.api_key.is_none());
    }

    #[test]
    fn commented_or_empty_api_key_is_absent() {
        let config_path = std::env::temp_dir().join(format!(
            "aw-sync-config-commented-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(
            &config_path,
            "port = 5600\n[auth]\n#api_key = \"secret\"\napi_key = \"\"\n",
        )
        .unwrap();

        let config = get_server_config(false, Some(&config_path)).unwrap();
        fs::remove_file(config_path).unwrap();
        assert!(config.api_key.is_none());
    }

    #[test]
    fn recognizes_only_loopback_hosts() {
        for host in ["127.0.0.1", "127.0.0.2", "::1", "localhost", "LOCALHOST"] {
            assert!(is_loopback_host(host));
        }
        for host in ["example.com", "192.0.2.1", "localhost.example.com"] {
            assert!(!is_loopback_host(host));
        }
    }

    #[test]
    fn brackets_bare_ipv6_hosts_for_urls() {
        assert_eq!(host_for_url("::1"), "[::1]");
        assert_eq!(host_for_url("2001:db8::1"), "[2001:db8::1]");
        assert_eq!(host_for_url("127.0.0.1"), "127.0.0.1");
        assert_eq!(host_for_url("localhost"), "localhost");
    }
}

/// Return hostnames that have a 3-level `{host}/{device_id}/*.db` tree.
///
/// Shares the walker with `find_remotes` so daemon/`sync_run` and
/// `aw-sync sync`/Android agree on what a remote looks like
/// (ActivityWatch/aw-server-rust#682). Legacy 2-level
/// `{device_id}/test.db` entries at the sync root have no hostname and
/// are not returned — `sync_wrapper` cannot pull them.
pub fn get_remotes() -> Result<Vec<String>, Box<dyn Error>> {
    let sync_root_dir = crate::dirs::get_sync_dir()?;
    fs::create_dir_all(&sync_root_dir)?;
    let mut hostnames: Vec<String> = find_remotes(&sync_root_dir)?
        .into_iter()
        .filter_map(|db| hostname_from_db(&sync_root_dir, &db))
        .collect();
    hostnames.sort();
    hostnames.dedup();
    info!("Found remotes: {:?}", hostnames);
    Ok(hostnames)
}

/// `{sync_root}/{hostname}/{device_id}/file.db` → Some(hostname).
/// `{sync_root}/{device_id}/file.db` (legacy 2-level) → None.
fn hostname_from_db(sync_root: &Path, db: &Path) -> Option<String> {
    let rel = db.strip_prefix(sync_root).ok()?;
    let mut comps = rel.components();
    let host = comps.next()?.as_os_str().to_str()?.to_string();
    let _device_id = comps.next()?;
    let _file = comps.next()?;
    if comps.next().is_some() {
        return None;
    }
    Some(host)
}

/// Returns a list of all remote dbs two or three levels below `sync_directory`.
///
/// Two layouts exist in the wild:
/// - `{dir}/{device_id}/test.db` (legacy `sync_run` against the sync root)
/// - `{dir}/{hostname}/{device_id}/test.db` (`sync_wrapper` / Android)
///
/// I/O errors are propagated rather than unwrapped (a panic here aborts the app
/// on Android, ActivityWatch/aw-android#220) and rather than skipped: silently
/// dropping a host directory we failed to read would report a successful sync
/// that quietly omitted that host's data.
fn find_remotes(sync_directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut dbs = Vec::new();
    collect_db_files(sync_directory, 0, &mut dbs)?;
    Ok(dbs)
}

/// Collect `.db` files in directories at depth 1 or 2 (file paths at 2 or 3
/// components relative to `dir`).
fn collect_db_files(dir: &Path, dir_depth: u32, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    const MAX_DIR_DEPTH: u32 = 2;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            if dir_depth < MAX_DIR_DEPTH {
                collect_db_files(&path, dir_depth + 1, out)?;
            }
        } else if dir_depth >= 1 && path.extension().unwrap_or_else(|| OsStr::new("")) == "db" {
            out.push(path);
        }
    }
    Ok(())
}

/// Returns a list of all remotes, excluding local ones
pub fn find_remotes_nonlocal(
    sync_directory: &Path,
    device_id: &str,
    sync_db: Option<&PathBuf>,
) -> std::io::Result<Vec<PathBuf>> {
    let remotes_all = find_remotes(sync_directory)?;
    Ok(remotes_all
        .into_iter()
        // Filter out own remote
        .filter(|path| !path.to_string_lossy().contains(device_id))
        // If sync_db is Some, return only remotes in that path
        .filter(|path| {
            if let Some(sync_db) = sync_db {
                path.starts_with(sync_db)
            } else {
                true
            }
        })
        .collect())
}

#[cfg(test)]
mod remote_layout_tests {
    use super::{find_remotes, hostname_from_db};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_sync_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aw-sync-find-remotes-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch_db(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn find_remotes_sees_wrapper_and_legacy_layouts() {
        let root = temp_sync_dir();
        let three_level = root.join("poco").join("device-android").join("test.db");
        let two_level = root.join("device-desktop").join("test.db");
        touch_db(&three_level);
        touch_db(&two_level);
        fs::write(root.join("orphan.db"), b"").unwrap();

        let mut found = find_remotes(&root).unwrap();
        found.sort();
        let mut expected = vec![two_level, three_level];
        expected.sort();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(found, expected);
    }

    #[test]
    fn hostname_from_db_only_for_three_level_layout() {
        let root = Path::new("/tmp/ActivityWatchSync");
        assert_eq!(
            hostname_from_db(
                root,
                &root.join("poco").join("device-android").join("test.db")
            )
            .as_deref(),
            Some("poco")
        );
        assert_eq!(
            hostname_from_db(root, &root.join("device-desktop").join("test.db")),
            None
        );
        assert_eq!(hostname_from_db(root, &root.join("orphan.db")), None);
    }

    #[test]
    fn find_remotes_from_host_dir_still_sees_device_dbs() {
        // `sync_wrapper::pull` calls find_remotes on `{sync_dir}/{host}`.
        let root = temp_sync_dir();
        let host_dir = root.join("erb-m2.localdomain");
        let db = host_dir.join("d7bc68e7").join("test.db");
        touch_db(&db);

        let found = find_remotes(&host_dir).unwrap();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(found, vec![db]);
    }
}
