use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

    fn write_two_level_db(
        root: &std::path::Path,
        device_id: &str,
        size: usize,
    ) -> std::path::PathBuf {
        let dir = root.join(device_id);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        fs::write(&path, vec![0u8; size]).unwrap();
        path
    }

    fn set_mtime(path: &std::path::Path, secs_ago: u64) {
        let t = SystemTime::now() - std::time::Duration::from_secs(secs_ago);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
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
    fn list_remote_dbs_includes_legacy_two_level_root_dbs() {
        // `{sync_root}/{device_id}/test.db` is the leftover daemon layout
        // from #682. Not-yet-upgraded peers still write it, so pull_all
        // must see it (hostname unknown) and dedupe against any 3-level
        // entry for the same device_id.
        let root = temp_sync_root();
        let three = write_remote_db(&root, "poco_f8_ultra", "device-1", 64);
        let orphan = write_two_level_db(&root, "device-orphan", 128);

        let listed = super::list_remote_dbs(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(listed.len(), 2);
        assert!(listed
            .iter()
            .any(|d| d.path == three && !d.hostname.is_empty()));
        let two = listed.iter().find(|d| d.path == orphan).unwrap();
        assert!(two.hostname.is_empty());
        assert_eq!(two.device_id, "device-orphan");
        assert_eq!(two.size, 128);
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
            mtime: SystemTime::UNIX_EPOCH,
        };
        let b = super::RemoteDb {
            hostname: "host-b".into(),
            device_id: "bbb".into(),
            path: std::path::PathBuf::from("/sync/host-b/bbb/test.db"),
            size: 1,
            mtime: SystemTime::UNIX_EPOCH,
        };
        let selected = super::select_remote_dbs_by_device_id(vec![a.clone(), b.clone()]);
        assert_eq!(selected, vec![a, b]);
    }

    #[test]
    fn select_remote_dbs_mixed_layout_prefers_newest_not_largest() {
        // Same device_id at 2-level (the leftover, large) and 3-level
        // (current, small). Size would pick the leftover forever; newest
        // data (mtime) picks the 3-level file that is still being written.
        let root = temp_sync_root();
        let two = write_two_level_db(&root, "device-1", 128);
        let three = write_remote_db(&root, "poco_f8_ultra", "device-1", 8);
        set_mtime(&two, 3600);
        set_mtime(&three, 1);

        let selected =
            super::select_remote_dbs_by_device_id(super::list_remote_dbs(&root).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].path, three);
        assert_eq!(selected[0].hostname, "poco_f8_ultra");
    }

    #[test]
    fn select_remote_dbs_two_level_only_peer_is_kept() {
        // A daemon peer that has not upgraded still writes 2-level only.
        let root = temp_sync_root();
        let two = write_two_level_db(&root, "device-peer", 64);
        let other = write_remote_db(&root, "host-b", "device-other", 8);

        let selected =
            super::select_remote_dbs_by_device_id(super::list_remote_dbs(&root).unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(selected.len(), 2);
        assert!(selected
            .iter()
            .any(|d| d.path == two && d.hostname.is_empty()));
        assert!(selected.iter().any(|d| d.path == other));
    }

    #[test]
    fn promote_legacy_own_db_renames_into_host_layout() {
        let root = temp_sync_root();
        let src = write_two_level_db(&root, "device-1", 32);
        let wal = {
            let mut p = src.as_os_str().to_os_string();
            p.push("-wal");
            let p = std::path::PathBuf::from(p);
            fs::write(&p, b"wal").unwrap();
            p
        };

        let promoted = super::promote_legacy_own_db(&root, "erb-m2", "device-1").unwrap();
        let dest = root.join("erb-m2").join("device-1").join("test.db");
        let dest_wal = {
            let mut p = dest.as_os_str().to_os_string();
            p.push("-wal");
            std::path::PathBuf::from(p)
        };

        assert!(promoted);
        assert!(dest.exists());
        assert!(!src.exists());
        assert!(dest_wal.exists());
        assert!(!wal.exists());
        // Empty leftover directory is kept — nothing is deleted.
        assert!(root.join("device-1").exists());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn promote_legacy_own_db_is_noop_when_dest_exists() {
        let root = temp_sync_root();
        let src = write_two_level_db(&root, "device-1", 32);
        let dest = write_remote_db(&root, "erb-m2", "device-1", 8);

        let promoted = super::promote_legacy_own_db(&root, "erb-m2", "device-1").unwrap();
        assert!(!promoted);
        assert!(src.exists());
        assert!(dest.exists());
        assert_eq!(fs::read(&src).unwrap().len(), 32);
        assert_eq!(fs::read(&dest).unwrap().len(), 8);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn promote_legacy_own_db_is_noop_when_src_missing() {
        let root = temp_sync_root();
        let promoted = super::promote_legacy_own_db(&root, "erb-m2", "device-1").unwrap();
        assert!(!promoted);
        assert!(!root
            .join("erb-m2")
            .join("device-1")
            .join("test.db")
            .exists());
        fs::remove_dir_all(&root).unwrap();
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

/// A peer database discovered under the sync root.
///
/// - 3-level: `{sync_root}/{hostname}/{device_id}/*.db` (`hostname` is the folder)
/// - 2-level leftover: `{sync_root}/{device_id}/*.db` (`hostname` is empty —
///   the folder name is the device_id, so the hostname is unknown)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteDb {
    pub hostname: String,
    pub device_id: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: SystemTime,
}

impl RemoteDb {
    fn is_two_level(&self) -> bool {
        self.hostname.is_empty()
    }
}

fn file_size_and_mtime(meta: Option<fs::Metadata>) -> (u64, SystemTime) {
    match meta {
        Some(m) => (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
        None => (0, SystemTime::UNIX_EPOCH),
    }
}

/// List peer databases under `sync_root`.
///
/// Walks both layouts:
/// - 3-level `{hostname}/{device_id}/*.db` (Android / `aw-sync sync` / upgraded daemon)
/// - 2-level `{device_id}/*.db` (legacy daemon, still written by not-yet-upgraded peers)
///
/// 2-level entries have an empty `hostname`. Callers collapse by `device_id`
/// via [`select_remote_dbs_by_device_id`]. I/O errors are propagated rather
/// than skipped: dropping a host directory we failed to read would report a
/// successful sync that quietly omitted that host's data.
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
        let Some(first_name) = host_ent.file_name().to_str().map(str::to_string) else {
            continue;
        };
        for child_ent in fs::read_dir(&host_path)? {
            let child_ent = child_ent?;
            let child_path = child_ent.path();
            if child_path.is_file() && child_path.extension().and_then(|e| e.to_str()) == Some("db")
            {
                // 2-level: `{sync_root}/{device_id}/*.db`
                let (size, mtime) = file_size_and_mtime(child_ent.metadata().ok());
                dbs.push(RemoteDb {
                    hostname: String::new(),
                    device_id: first_name.clone(),
                    path: child_path,
                    size,
                    mtime,
                });
                continue;
            }
            if !child_path.is_dir() {
                continue;
            }
            let Some(device_id) = child_ent.file_name().to_str().map(str::to_string) else {
                continue;
            };
            for file_ent in fs::read_dir(&child_path)? {
                let file_ent = file_ent?;
                let path = file_ent.path();
                if !(path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("db")) {
                    continue;
                }
                let (size, mtime) = file_size_and_mtime(file_ent.metadata().ok());
                dbs.push(RemoteDb {
                    hostname: first_name.clone(),
                    device_id: device_id.clone(),
                    path,
                    size,
                    mtime,
                });
            }
        }
    }
    Ok(dbs)
}

/// Keep one database per `device_id`.
///
/// Same-layout duplicates (two 3-level hostname folders after sanitization)
/// still pick the largest file so resume-from-newest cannot drop history
/// (ActivityWatch/aw-server-rust#683).
///
/// Mixed 2-level + 3-level for the same device pick **newest data** (file
/// mtime), not size. The leftover 2-level file is the one with months of
/// history and would win on size forever, even after the 3-level file is
/// the one being written.
pub(crate) fn select_remote_dbs_by_device_id(dbs: Vec<RemoteDb>) -> Vec<RemoteDb> {
    let mut by_device: HashMap<String, Vec<RemoteDb>> = HashMap::new();
    for db in dbs {
        by_device.entry(db.device_id.clone()).or_default().push(db);
    }

    let mut selected = Vec::with_capacity(by_device.len());
    for (device_id, mut group) in by_device {
        let mixed =
            group.iter().any(RemoteDb::is_two_level) && group.iter().any(|d| !d.is_two_level());
        if mixed {
            group.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
        } else {
            group.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
        }
        let mut group = group.into_iter();
        let winner = group.next().expect("device_id group is non-empty");
        let skipped: Vec<String> = group
            .map(|d| format!("{} ({} bytes)", d.path.display(), d.size))
            .collect();
        if !skipped.is_empty() {
            if mixed {
                warn!(
                    "device_id {device_id} has both 2-level and 3-level dbs; using newest {} ({} bytes), skipping: {:?}",
                    winner.path.display(),
                    winner.size,
                    skipped
                );
            } else {
                warn!(
                    "device_id {device_id} appears under {} folders; using largest {} ({} bytes), skipping: {:?}",
                    skipped.len() + 1,
                    winner.path.display(),
                    winner.size,
                    skipped
                );
            }
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

const SQLITE_SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

fn sidecar_path(db: &Path, suffix: &str) -> PathBuf {
    let mut s = db.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn rename_sqlite_db(src: &Path, dest: &Path) -> std::io::Result<()> {
    fs::rename(src, dest)?;
    for suffix in SQLITE_SIDECARS {
        let src_side = sidecar_path(src, suffix);
        if src_side.exists() {
            fs::rename(&src_side, sidecar_path(dest, suffix))?;
        }
    }
    Ok(())
}

/// If `{sync_root}/{device_id}/test.db` exists and the 3-level destination
/// `{sync_root}/{hostname}/{device_id}/test.db` does not, rename the leftover
/// own db (plus sqlite sidecars) into place.
///
/// Creating a fresh 3-level file would re-export the entire history into a
/// new Syncthing object. A rename is one metadata op and, because the
/// content is identical, a cheap move on the syncer side. Nothing is deleted:
/// an empty leftover `{device_id}/` directory is left behind.
///
/// Returns `true` if a rename happened.
pub(crate) fn promote_legacy_own_db(
    sync_root: &Path,
    hostname: &str,
    device_id: &str,
) -> std::io::Result<bool> {
    let dest_dir = sync_root.join(hostname).join(device_id);
    let dest = dest_dir.join("test.db");
    let src = sync_root.join(device_id).join("test.db");
    if dest.exists() || !src.exists() || src == dest {
        return Ok(false);
    }
    fs::create_dir_all(&dest_dir)?;
    info!(
        "Promoting leftover 2-level own db {} → {} (avoid re-exporting history into a new file)",
        src.display(),
        dest.display()
    );
    rename_sqlite_db(&src, &dest)?;
    Ok(true)
}

/// 2-level walker: `{sync_directory}/{x}/*.db`.
///
/// Callers pass different roots:
/// - `sync_wrapper::pull` passes a host folder, so this finds
///   `{host}/{device_id}/*.db`
/// - advanced `sync_run` against the sync root finds the legacy
///   `{device_id}/*.db` layout
///
/// Default-daemon pull is [`list_remote_dbs`] (both layouts). This walker
/// stays 2-level relative to the directory it is given: a host folder
/// yields `{host}/{device_id}/*.db`; the sync root yields leftover
/// `{device_id}/*.db`.
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
            let (size, mtime) = file_size_and_mtime(fs::metadata(&path).ok());
            Some(RemoteDb {
                hostname,
                device_id,
                path,
                size,
                mtime,
            })
        })
        .collect();
    select_remote_dbs_by_device_id(dbs)
        .into_iter()
        .map(|d| d.path)
        .collect()
}

/// How a database sits in the sync folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncLayout {
    TwoLevel,
    ThreeLevel,
}

impl SyncLayout {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncLayout::TwoLevel => "2-level",
            SyncLayout::ThreeLevel => "3-level",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncEntryKind {
    Peer,
    OwnStaging,
    Unrecognised,
}

impl SyncEntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncEntryKind::Peer => "peer",
            SyncEntryKind::OwnStaging => "own-staging",
            SyncEntryKind::Unrecognised => "unrecognised",
        }
    }
}

/// One filesystem entry in the sync directory, classified without opening sqlite.
#[derive(Debug, Clone)]
pub struct SyncDirEntry {
    pub path: PathBuf,
    pub db_path: Option<PathBuf>,
    pub db_size: Option<u64>,
    pub layout: Option<SyncLayout>,
    pub kind: SyncEntryKind,
    pub hostname_folder: Option<String>,
    pub device_id: Option<String>,
    /// Why this path is not a pull remote (own staging, junk, empty folder).
    pub not_visible_to_daemon: Option<String>,
}

impl SyncDirEntry {
    pub fn diagnostic_line(&self) -> String {
        let size = self
            .db_size
            .map(format_bytes)
            .unwrap_or_else(|| "-".to_string());
        let layout = self.layout.map(|l| l.as_str()).unwrap_or("-");
        let mut line = format!(
            "  [{:<12} {layout}] {}  {size}",
            self.kind.as_str(),
            self.path.display()
        );
        if let Some(reason) = &self.not_visible_to_daemon {
            line.push_str(&format!("\n    not pulled: {reason}"));
        }
        line
    }
}

/// Classify the sync folder for `aw-sync status` and empty-pull warnings.
///
/// Peer databases come from [`list_remote_dbs`] + [`select_remote_dbs_by_device_id`]
/// — the same pair `pull_all` uses — so duplicate-`device_id` "not pulled" reasons
/// match the pull path (both 2-level leftovers and 3-level hosts). Status-only
/// overlay on top of that list: unrecognised files/dirs, own-staging vs peer,
/// [`SyncLayout`].
///
/// Does not open sqlite files and does not create directories. `local_device_id`
/// is used to tell own staging copies from peers; pass `None` when unknown.
pub fn scan_sync_dir(
    sync_directory: &Path,
    local_device_id: Option<&str>,
) -> std::io::Result<Vec<SyncDirEntry>> {
    let remotes = list_remote_dbs(sync_directory)?;
    let selected = select_remote_dbs_by_device_id(remotes.clone());
    let selected_paths: HashSet<PathBuf> = selected.into_iter().map(|d| d.path).collect();
    let remote_paths: HashSet<PathBuf> = remotes.iter().map(|d| d.path.clone()).collect();
    let known_hosts: HashSet<String> = remotes
        .iter()
        .filter(|d| !d.hostname.is_empty())
        .map(|d| d.hostname.clone())
        .collect();
    let two_level_tops: HashSet<PathBuf> = remotes
        .iter()
        .filter(|d| d.is_two_level())
        .filter_map(|d| d.path.parent().map(Path::to_path_buf))
        .collect();

    let mut entries: Vec<SyncDirEntry> = remotes
        .into_iter()
        .map(|db| remote_db_to_entry(db, local_device_id, &selected_paths))
        .collect();

    if sync_directory.exists() {
        for child in fs::read_dir(sync_directory)? {
            let child = child?;
            let path = child.path();
            if path.is_file() {
                entries.push(file_at_root(path));
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            if two_level_tops.contains(&path) {
                // Already classified from list_remote_dbs as a 2-level db.
                continue;
            }
            let name = file_name_string(&path);
            if name.as_ref().is_some_and(|n| known_hosts.contains(n)) {
                // Host folder already walked by list_remote_dbs. Only pick
                // leftover 2-level *.db files sitting beside device_id dirs.
                for file in fs::read_dir(&path)? {
                    let fp = file?.path();
                    if fp.is_file()
                        && fp.extension().unwrap_or_else(|| OsStr::new("")) == "db"
                        && !remote_paths.contains(&fp)
                    {
                        entries.push(db_entry(
                            &fp,
                            SyncLayout::TwoLevel,
                            None,
                            name.clone(),
                            local_device_id,
                            None,
                        )?);
                    }
                }
                continue;
            }
            classify_top_dir(&path, local_device_id, &mut entries)?;
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn remote_db_to_entry(
    db: RemoteDb,
    local_device_id: Option<&str>,
    selected_paths: &HashSet<PathBuf>,
) -> SyncDirEntry {
    let own = local_device_id == Some(db.device_id.as_str());
    let two_level = db.is_two_level();
    let not_visible = if own {
        Some("own device_id, excluded from pull".to_string())
    } else if !selected_paths.contains(&db.path) {
        Some(format!(
            "duplicate device_id {}; pull keeps one db per device (ActivityWatch/aw-server-rust#683)",
            db.device_id
        ))
    } else {
        None
    };
    SyncDirEntry {
        path: db.path.clone(),
        db_path: Some(db.path),
        db_size: Some(db.size),
        layout: Some(if two_level {
            SyncLayout::TwoLevel
        } else {
            SyncLayout::ThreeLevel
        }),
        kind: if own {
            SyncEntryKind::OwnStaging
        } else {
            SyncEntryKind::Peer
        },
        hostname_folder: if two_level { None } else { Some(db.hostname) },
        device_id: Some(db.device_id),
        not_visible_to_daemon: not_visible,
    }
}

fn file_at_root(path: PathBuf) -> SyncDirEntry {
    let is_db = path.extension().unwrap_or_else(|| OsStr::new("")) == "db";
    let db_size = fs::metadata(&path).ok().map(|m| m.len());
    SyncDirEntry {
        db_path: is_db.then(|| path.clone()),
        db_size: is_db.then_some(db_size).flatten(),
        layout: None,
        kind: SyncEntryKind::Unrecognised,
        hostname_folder: None,
        device_id: None,
        not_visible_to_daemon: Some(if is_db {
            "database at sync root; expected {device_id}/*.db or {hostname}/{device_id}/*.db"
                .to_string()
        } else {
            "not a database".to_string()
        }),
        path,
    }
}

fn classify_top_dir(
    dir: &Path,
    local_device_id: Option<&str>,
    entries: &mut Vec<SyncDirEntry>,
) -> std::io::Result<()> {
    let top_name = file_name_string(dir);
    let mut db_files = Vec::new();
    let mut subdirs = Vec::new();
    for child in fs::read_dir(dir)? {
        let child = child?;
        let path = child.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.extension().unwrap_or_else(|| OsStr::new("")) == "db" {
            db_files.push(path);
        }
    }

    for db in &db_files {
        entries.push(db_entry(
            db,
            SyncLayout::TwoLevel,
            None,
            top_name.clone(),
            local_device_id,
            None,
        )?);
    }

    let mut found_three_level = false;
    for sub in &subdirs {
        let device_id = file_name_string(sub);
        let mut sub_dbs = Vec::new();
        for child in fs::read_dir(sub)? {
            let path = child?.path();
            if path.extension().unwrap_or_else(|| OsStr::new("")) == "db" {
                sub_dbs.push(path);
            }
        }
        if sub_dbs.is_empty() {
            entries.push(SyncDirEntry {
                path: sub.clone(),
                db_path: None,
                db_size: None,
                layout: Some(SyncLayout::ThreeLevel),
                kind: SyncEntryKind::Unrecognised,
                hostname_folder: top_name.clone(),
                device_id,
                not_visible_to_daemon: Some("hostname/device_id folder with no .db".to_string()),
            });
            continue;
        }
        found_three_level = true;
        for db in sub_dbs {
            entries.push(db_entry(
                &db,
                SyncLayout::ThreeLevel,
                top_name.clone(),
                device_id.clone(),
                local_device_id,
                None,
            )?);
        }
    }

    if db_files.is_empty() && !found_three_level && subdirs.is_empty() {
        entries.push(SyncDirEntry {
            path: dir.to_path_buf(),
            db_path: None,
            db_size: None,
            layout: None,
            kind: SyncEntryKind::Unrecognised,
            hostname_folder: None,
            device_id: top_name,
            not_visible_to_daemon: Some("directory with no database".to_string()),
        });
    }
    Ok(())
}

fn db_entry(
    db: &Path,
    layout: SyncLayout,
    hostname_folder: Option<String>,
    device_id: Option<String>,
    local_device_id: Option<&str>,
    not_visible_to_daemon: Option<String>,
) -> std::io::Result<SyncDirEntry> {
    let db_size = fs::metadata(db).ok().map(|m| m.len());
    let own = match (local_device_id, device_id.as_deref()) {
        (Some(local), Some(did)) => local == did,
        (Some(local), None) => db.to_string_lossy().contains(local),
        _ => false,
    };
    let mut not_visible = not_visible_to_daemon;
    if own && not_visible.is_none() {
        not_visible = Some("own device_id, excluded from pull".to_string());
    }
    Ok(SyncDirEntry {
        path: db.to_path_buf(),
        db_path: Some(db.to_path_buf()),
        db_size,
        layout: Some(layout),
        kind: if own {
            SyncEntryKind::OwnStaging
        } else {
            SyncEntryKind::Peer
        },
        hostname_folder,
        device_id,
        not_visible_to_daemon: not_visible,
    })
}

fn file_name_string(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
}

pub fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Warn-lines for a pull that found no remotes, or that missed classified peers.
pub fn pull_discovery_warnings(
    sync_directory: &Path,
    local_device_id: &str,
    found_remotes: &[PathBuf],
) -> Vec<String> {
    let scan = match scan_sync_dir(sync_directory, Some(local_device_id)) {
        Ok(entries) => entries,
        Err(e) => {
            return vec![format!(
                "Could not scan sync dir {}: {e}",
                sync_directory.display()
            )]
        }
    };

    let mut lines = Vec::new();
    let found: std::collections::HashSet<&PathBuf> = found_remotes.iter().collect();
    let missed: Vec<&SyncDirEntry> = scan
        .iter()
        .filter(|e| {
            e.kind == SyncEntryKind::Peer && e.db_path.as_ref().is_some_and(|p| !found.contains(p))
        })
        .collect();

    if found_remotes.is_empty() {
        lines.push(format!(
            "Found 0 remote db files to pull from {}. \
             Zero peers in a configured sync dir is usually a layout or setup problem, not a no-op.",
            sync_directory.display()
        ));
        if scan.is_empty() {
            lines.push(
                "  (sync directory is empty aside from what this process creates)".to_string(),
            );
        } else {
            for entry in &scan {
                lines.push(entry.diagnostic_line());
            }
        }
    } else if !missed.is_empty() {
        lines.push(format!(
            "Found {} peer db(s) that pull did not select:",
            missed.len()
        ));
        for entry in missed {
            lines.push(entry.diagnostic_line());
        }
    }

    let mut by_device: std::collections::BTreeMap<String, Vec<&SyncDirEntry>> =
        std::collections::BTreeMap::new();
    for entry in scan
        .iter()
        .filter(|e| e.device_id.is_some() && e.db_path.is_some())
    {
        if let Some(did) = &entry.device_id {
            by_device.entry(did.clone()).or_default().push(entry);
        }
    }
    for (did, group) in by_device {
        if group.len() > 1 {
            let folders: Vec<String> = group
                .iter()
                .map(|e| {
                    e.hostname_folder
                        .clone()
                        .unwrap_or_else(|| e.path.display().to_string())
                })
                .collect();
            lines.push(format!(
                "device_id {did} appears under {} folders: {} — pulling both can truncate history (see ActivityWatch/aw-server-rust#683)",
                group.len(),
                folders.join(", ")
            ));
        }
    }

    lines
}

/// Read-only peek at a peer sqlite file (bucket hostname, counts, newest event).
///
/// Opens with `SQLITE_OPEN_READ_ONLY` so a doctor command does not create
/// `-wal`/`-shm` sidecars in a Syncthing folder.
#[cfg(feature = "cli")]
#[derive(Debug, Clone)]
pub struct DbInspect {
    pub hostname: Option<String>,
    pub bucket_count: usize,
    pub event_count: i64,
    pub newest_event: Option<chrono::DateTime<chrono::Utc>>,
    pub buckets: Vec<BucketInspect>,
}

#[cfg(feature = "cli")]
#[derive(Debug, Clone)]
pub struct BucketInspect {
    pub id: String,
}

#[cfg(feature = "cli")]
pub fn inspect_sync_db(path: &Path) -> Result<DbInspect, String> {
    use rusqlite::{Connection, OpenFlags};

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("failed to open {} read-only: {e}", path.display()))?;

    let mut stmt = conn
        .prepare(
            "SELECT buckets.name, buckets.hostname,
                    COUNT(events.id),
                    MAX(events.endtime)
             FROM buckets
             LEFT JOIN events ON events.bucketrow = buckets.id
             GROUP BY buckets.id",
        )
        .map_err(|e| format!("schema query failed on {}: {e}", path.display()))?;

    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let hostname: String = row.get(1)?;
            let events: i64 = row.get(2)?;
            let last_ns: Option<i64> = row.get(3)?;
            Ok((id, hostname, events, last_ns))
        })
        .map_err(|e| e.to_string())?;

    let mut buckets = Vec::new();
    let mut hostname = None;
    let mut event_count = 0i64;
    let mut newest_ns: Option<i64> = None;
    for row in rows {
        let (id, host, events, last_ns) = row.map_err(|e| e.to_string())?;
        event_count += events;
        if hostname.is_none() && !host.is_empty() && host != "unknown" {
            hostname = Some(host.clone());
        }
        if let Some(ns) = last_ns {
            newest_ns = Some(newest_ns.map_or(ns, |cur| cur.max(ns)));
        }
        buckets.push(BucketInspect { id });
    }

    Ok(DbInspect {
        hostname,
        bucket_count: buckets.len(),
        event_count,
        newest_event: newest_ns.and_then(ns_to_datetime),
        buckets,
    })
}

#[cfg(feature = "cli")]
fn ns_to_datetime(ns: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    let seconds = ns / 1_000_000_000;
    let subnanos = (ns % 1_000_000_000) as u32;
    chrono::DateTime::from_timestamp(seconds, subnanos)
}

#[cfg(all(test, not(target_os = "android")))]
mod scan_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_sync_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aw-sync-scan-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn touch_db(path: &Path, size: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; size]).unwrap();
    }

    #[test]
    fn classifies_both_layouts_and_flags_duplicate_device_id() {
        let root = temp_sync_dir();
        let local = "d7bc68e7-aaaa-bbbb-cccc-dddddddddddd";
        let peer = "41662faa-aaaa-bbbb-cccc-dddddddddddd";

        touch_db(&root.join(local).join("test.db"), 64);
        touch_db(&root.join("poco_f8_ultra").join(peer).join("test.db"), 273);
        touch_db(&root.join("POCO F8 Ultra").join(peer).join("test.db"), 9);
        fs::write(root.join("readme.txt"), "noise").unwrap();

        let scan = scan_sync_dir(&root, Some(local)).unwrap();
        let kinds: Vec<_> = scan.iter().map(|e| (e.kind, e.layout)).collect();
        assert!(
            kinds
                .iter()
                .any(|(k, l)| *k == SyncEntryKind::OwnStaging && *l == Some(SyncLayout::TwoLevel)),
            "own 2-level staging: {kinds:?}"
        );
        let three_level_peers: Vec<_> = scan
            .iter()
            .filter(|e| e.kind == SyncEntryKind::Peer && e.layout == Some(SyncLayout::ThreeLevel))
            .collect();
        assert_eq!(three_level_peers.len(), 2);
        let kept: Vec<_> = three_level_peers
            .iter()
            .filter(|e| e.not_visible_to_daemon.is_none())
            .collect();
        let skipped: Vec<_> = three_level_peers
            .iter()
            .filter(|e| e.not_visible_to_daemon.is_some())
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "pull-equivalent winner: {three_level_peers:?}"
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(kept[0].db_size, Some(273));
        assert!(
            skipped[0]
                .not_visible_to_daemon
                .as_ref()
                .is_some_and(|s| s.contains("duplicate device_id")),
            "skipped duplicate must use the pull selector: {skipped:?}"
        );

        let warnings = pull_discovery_warnings(&root, local, &[]);
        let joined = warnings.join("\n");
        assert!(
            joined.contains("Found 0 remote db files"),
            "empty-pull warning: {joined}"
        );
        assert!(
            joined.contains("3-level"),
            "must list 3-level peers in the dump: {joined}"
        );
        assert!(
            joined.contains(peer),
            "must name the duplicated device_id: {joined}"
        );
        assert!(
            joined.contains("appears under 2 folders"),
            "duplicate device_id warning: {joined}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn status_keeps_the_same_duplicate_winner_as_pull() {
        let root = temp_sync_dir();
        let local = "local-device";
        let peer = "peer-device";
        touch_db(&root.join("poco_f8_ultra").join(peer).join("test.db"), 273);
        touch_db(&root.join("POCO F8 Ultra").join(peer).join("test.db"), 9);

        let remotes = list_remote_dbs(&root).unwrap();
        let selected = select_remote_dbs_by_device_id(remotes);
        let winner = selected
            .iter()
            .find(|d| d.device_id == peer)
            .expect("pull selector keeps one db for the peer");

        let scan = scan_sync_dir(&root, Some(local)).unwrap();
        let kept: Vec<_> = scan
            .iter()
            .filter(|e| {
                e.kind == SyncEntryKind::Peer
                    && e.layout == Some(SyncLayout::ThreeLevel)
                    && e.not_visible_to_daemon.is_none()
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].db_path.as_ref(), Some(&winner.path));
        assert_eq!(kept[0].db_size, Some(winner.size));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn two_level_peer_is_selected_when_found() {
        let root = temp_sync_dir();
        let local = "local-device";
        let peer = "peer-device";
        touch_db(&root.join(peer).join("test.db"), 8);

        let scan = scan_sync_dir(&root, Some(local)).unwrap();
        let peer_entry = scan
            .iter()
            .find(|e| e.kind == SyncEntryKind::Peer)
            .expect("peer");
        assert_eq!(peer_entry.layout, Some(SyncLayout::TwoLevel));
        assert!(peer_entry.not_visible_to_daemon.is_none());

        let found = vec![root.join(peer).join("test.db")];
        let warnings = pull_discovery_warnings(&root, local, &found);
        assert!(
            warnings
                .iter()
                .all(|l| !l.contains("Found 0 remote db files")),
            "should not warn about empty remotes when a 2-level peer was found: {warnings:?}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(feature = "cli")]
    #[test]
    fn inspect_sync_db_reads_hostname_and_newest_event() {
        let root = temp_sync_dir();
        let db_path = root.join("peer").join("test.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();

        {
            use rusqlite::Connection;
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE buckets (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT UNIQUE NOT NULL,
                    type TEXT NOT NULL,
                    client TEXT NOT NULL,
                    hostname TEXT NOT NULL,
                    created TEXT NOT NULL,
                    data TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    bucketrow INTEGER NOT NULL,
                    starttime INTEGER NOT NULL,
                    endtime INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
                INSERT INTO buckets (name, type, client, hostname, created, data)
                    VALUES ('aw-watcher-android', 'currentwindow', 'aw-android',
                            'POCO F8 Ultra', '2021-01-01T00:00:00Z', '{}');
                INSERT INTO events (bucketrow, starttime, endtime, data)
                    VALUES (1, 1000000000, 2000000000, '{}');",
            )
            .unwrap();
        }

        let info = inspect_sync_db(&db_path).unwrap();
        assert_eq!(info.hostname.as_deref(), Some("POCO F8 Ultra"));
        assert_eq!(info.bucket_count, 1);
        assert_eq!(info.event_count, 1);
        assert!(info.newest_event.is_some());

        let _ = fs::remove_dir_all(root);
    }
}
