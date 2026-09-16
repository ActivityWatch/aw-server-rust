use std::collections::HashMap;
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

    fn temp_sync_root() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "aw-sync-remotes-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_remote_db(
        root: &std::path::Path,
        host: &str,
        device_id: &str,
        size: usize,
    ) -> std::path::PathBuf {
        let dir = root.join(host).join(device_id);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        fs::write(&path, vec![0u8; size]).unwrap();
        path
    }

    #[test]
    fn list_remote_dbs_walks_hostname_device_layout() {
        let root = temp_sync_root();
        let large = write_remote_db(&root, "poco_f8_ultra", "device-1", 64);
        let small = write_remote_db(&root, "POCO F8 Ultra", "device-1", 8);
        write_remote_db(&root, "other-host", "device-2", 16);

        let mut listed = super::list_remote_dbs(&root).unwrap();
        listed.sort_by(|a, b| a.path.cmp(&b.path));
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(listed.len(), 3);
        assert!(listed.iter().any(|d| d.path == large && d.size == 64));
        assert!(listed.iter().any(|d| d.path == small && d.size == 8));
        assert!(listed.iter().any(|d| d.device_id == "device-2"));
    }

    #[test]
    fn list_remote_dbs_skips_legacy_two_level_root_dbs() {
        // `{sync_root}/{device_id}/test.db` is the leftover daemon layout
        // from #682. pull_all must not import it.
        let root = temp_sync_root();
        let three = write_remote_db(&root, "poco_f8_ultra", "device-1", 64);
        let two_level_dir = root.join("device-orphan");
        fs::create_dir_all(&two_level_dir).unwrap();
        let orphan = two_level_dir.join("test.db");
        fs::write(&orphan, vec![0u8; 128]).unwrap();

        let listed = super::list_remote_dbs(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, three);
        assert!(listed.iter().all(|d| d.path != orphan));
    }

    #[test]
    fn select_remote_dbs_keeps_largest_per_device_id() {
        let root = temp_sync_root();
        let large = write_remote_db(&root, "poco_f8_ultra", "device-1", 64);
        write_remote_db(&root, "POCO F8 Ultra", "device-1", 8);
        let other = write_remote_db(&root, "other-host", "device-2", 16);

        let listed = super::list_remote_dbs(&root).unwrap();
        // Reverse to prove selection does not depend on discovery order.
        let mut reversed = listed.clone();
        reversed.reverse();
        let selected = super::select_remote_dbs_by_device_id(reversed);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(selected.len(), 2);
        let device_1 = selected.iter().find(|d| d.device_id == "device-1").unwrap();
        assert_eq!(device_1.path, large);
        assert_eq!(device_1.size, 64);
        let device_2 = selected.iter().find(|d| d.device_id == "device-2").unwrap();
        assert_eq!(device_2.path, other);
    }

    #[test]
    fn select_remote_dbs_keeps_unique_device_ids() {
        let a = super::RemoteDb {
            hostname: "host-a".into(),
            device_id: "aaa".into(),
            path: std::path::PathBuf::from("/sync/host-a/aaa/test.db"),
            size: 10,
        };
        let b = super::RemoteDb {
            hostname: "host-b".into(),
            device_id: "bbb".into(),
            path: std::path::PathBuf::from("/sync/host-b/bbb/test.db"),
            size: 1,
        };
        let selected = super::select_remote_dbs_by_device_id(vec![a.clone(), b.clone()]);
        assert_eq!(selected, vec![a, b]);
    }

    #[test]
    fn select_db_paths_keeps_largest_per_device_id() {
        let root = temp_sync_root();
        let large = write_remote_db(&root, "poco_f8_ultra", "device-1", 64);
        write_remote_db(&root, "POCO F8 Ultra", "device-1", 8);
        let other = write_remote_db(&root, "other-host", "device-2", 16);
        let listed = super::list_remote_dbs(&root)
            .unwrap()
            .into_iter()
            .map(|d| d.path)
            .collect();
        let selected = super::select_db_paths_by_device_id(listed);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&large));
        assert!(selected.contains(&other));
    }
}

/// Check if a directory contains a .db file
fn contains_db_file(dir: &std::path::Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry
                    .path()
                    .extension()
                    .map(|ext| ext == "db")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Check if a directory contains a subdirectory that contains a .db file
fn contains_subdir_with_db_file(dir: &std::path::Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().is_dir() && contains_db_file(&entry.path()))
        })
        .unwrap_or(false)
}

/// A peer database discovered under `{sync_root}/{hostname}/{device_id}/*.db`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteDb {
    pub hostname: String,
    pub device_id: String,
    pub path: PathBuf,
    pub size: u64,
}

/// List every `{hostname}/{device_id}/*.db` under `sync_root`.
///
/// Unlike [`get_remotes`], this returns the device_id and file size so callers
/// can collapse duplicate folders for one device before importing. I/O errors
/// are propagated rather than skipped: dropping a host directory we failed to
/// read would report a successful sync that quietly omitted that host's data.
///
/// 3-level-only by design. `pull_all` used to go through [`get_remotes`] and
/// then `find_remotes` on each host folder; now it uses this walker, so a
/// leftover 2-level root db (`{sync_root}/{device_id}/test.db`, no hostname
/// folder) is **not** a pull candidate. That is the correct outcome: the
/// 1.19 GB root orphan from ActivityWatch/aw-server-rust#682 must never be
/// imported by a peer. Do not "fix" this by broadening the walk — the
/// advanced `sync_run` path still uses [`find_remotes`] (2-level, relative
/// to whatever directory it is given).
pub(crate) fn list_remote_dbs(sync_root: &Path) -> std::io::Result<Vec<RemoteDb>> {
    let mut dbs = Vec::new();
    if !sync_root.exists() {
        return Ok(dbs);
    }
    for host_ent in fs::read_dir(sync_root)? {
        let host_ent = host_ent?;
        let host_path = host_ent.path();
        if !host_path.is_dir() {
            continue;
        }
        let Some(hostname) = host_ent.file_name().to_str().map(str::to_string) else {
            continue;
        };
        for device_ent in fs::read_dir(&host_path)? {
            let device_ent = device_ent?;
            let device_path = device_ent.path();
            if !device_path.is_dir() {
                continue;
            }
            let Some(device_id) = device_ent.file_name().to_str().map(str::to_string) else {
                continue;
            };
            for file_ent in fs::read_dir(&device_path)? {
                let file_ent = file_ent?;
                let path = file_ent.path();
                if !(path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("db")) {
                    continue;
                }
                let size = file_ent.metadata().map(|m| m.len()).unwrap_or(0);
                dbs.push(RemoteDb {
                    hostname: hostname.clone(),
                    device_id: device_id.clone(),
                    path,
                    size,
                });
            }
        }
    }
    Ok(dbs)
}

/// Keep one database per `device_id`, preferring the largest file.
///
/// A hostname change (or sanitization) can leave the same device writing under
/// two folder names. Importing both is unsafe: provenance is derived from the
/// bucket hostname, so both land in the same destination bucket, and resume
/// then silently drops the older history. See ActivityWatch/aw-server-rust#683.
pub(crate) fn select_remote_dbs_by_device_id(dbs: Vec<RemoteDb>) -> Vec<RemoteDb> {
    let mut by_device: HashMap<String, Vec<RemoteDb>> = HashMap::new();
    for db in dbs {
        by_device.entry(db.device_id.clone()).or_default().push(db);
    }

    let mut selected = Vec::with_capacity(by_device.len());
    for (device_id, mut group) in by_device {
        group.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
        let mut group = group.into_iter();
        let winner = group.next().expect("device_id group is non-empty");
        let skipped: Vec<String> = group
            .map(|d| format!("{} ({} bytes)", d.path.display(), d.size))
            .collect();
        if !skipped.is_empty() {
            warn!(
                "device_id {device_id} appears under {} folders; using largest {} ({} bytes), skipping: {:?}",
                skipped.len() + 1,
                winner.path.display(),
                winner.size,
                skipped
            );
        }
        selected.push(winner);
    }
    selected.sort_by(|a, b| {
        a.hostname
            .cmp(&b.hostname)
            .then_with(|| a.device_id.cmp(&b.device_id))
            .then_with(|| a.path.cmp(&b.path))
    });
    selected
}

/// Return hostnames that have a `{host}/{device_id}/*.db` tree.
///
/// `pull_all` no longer calls this — it uses [`list_remote_dbs`] +
/// [`select_remote_dbs_by_device_id`]. Kept only so hostname-only callers
/// (if any remain) do not break; do not put pull back on this path, it
/// cannot distinguish duplicate `device_id` folders.
// TODO: share logic with find_remotes and find_remotes_nonlocal
#[allow(dead_code)] // pull_all now uses list_remote_dbs; no remaining in-crate caller
pub fn get_remotes() -> Result<Vec<String>, Box<dyn Error>> {
    let sync_root_dir = crate::dirs::get_sync_dir()?;
    fs::create_dir_all(&sync_root_dir)?;
    let hostnames = fs::read_dir(sync_root_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir() && contains_subdir_with_db_file(&entry.path()))
        .filter_map(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|os_str| os_str.to_str().map(String::from))
        })
        .collect();
    info!("Found remotes: {:?}", hostnames);
    Ok(hostnames)
}

/// 2-level walker: `{sync_directory}/{x}/*.db`.
///
/// Callers pass different roots:
/// - `sync_wrapper::pull` passes a host folder, so this finds
///   `{host}/{device_id}/*.db`
/// - advanced `sync_run` against the sync root finds the legacy
///   `{device_id}/*.db` layout
///
/// Do not broaden this to 3-level at the sync root. That would make a
/// pull import the leftover root orphan from #682. Default-daemon pull
/// is [`list_remote_dbs`] (3-level-only), not this function.
///
/// I/O errors are propagated rather than unwrapped (a panic here aborts the app
/// on Android, ActivityWatch/aw-android#220) and rather than skipped: silently
/// dropping a host directory we failed to read would report a successful sync
/// that quietly omitted that host's data.
fn find_remotes(sync_directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut dbs = Vec::new();
    for entry in fs::read_dir(sync_directory)? {
        let hostdir = entry?.path();
        if !hostdir.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&hostdir)? {
            let path = entry?.path();
            if path.extension().unwrap_or_else(|| OsStr::new("")) == "db" {
                dbs.push(path);
            }
        }
    }
    Ok(dbs)
}

/// Returns a list of all remotes, excluding local ones.
///
/// Duplicate folders for one `device_id` are collapsed to the largest db so
/// resume-from-newest cannot silently drop history (aw-server-rust#683).
pub fn find_remotes_nonlocal(
    sync_directory: &Path,
    device_id: &str,
    sync_db: Option<&PathBuf>,
) -> std::io::Result<Vec<PathBuf>> {
    let remotes_all = find_remotes(sync_directory)?;
    let filtered: Vec<PathBuf> = remotes_all
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
        .collect();
    Ok(select_db_paths_by_device_id(filtered))
}

/// Collapse `{…}/{device_id}/*.db` paths to the largest file per device_id.
fn select_db_paths_by_device_id(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let dbs: Vec<RemoteDb> = paths
        .into_iter()
        .filter_map(|path| {
            let device_id = path.parent()?.file_name()?.to_str()?.to_string();
            let hostname = path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Some(RemoteDb {
                hostname,
                device_id,
                path,
                size,
            })
        })
        .collect();
    select_remote_dbs_by_device_id(dbs)
        .into_iter()
        .map(|d| d.path)
        .collect()
}
