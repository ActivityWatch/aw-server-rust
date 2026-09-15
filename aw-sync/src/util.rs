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

/// Return all remotes in the sync folder
/// Only returns folders that match ./{host}/{device_id}/*.db
// TODO: share logic with find_remotes and find_remotes_nonlocal
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

/// Returns a list of all remote dbs
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

/// Walk both known sync-folder layouts and classify every entry.
///
/// Does not open sqlite files and does not create directories. `local_device_id`
/// is used to tell own staging copies from peers; pass `None` when unknown.
pub fn scan_sync_dir(
    sync_directory: &Path,
    local_device_id: Option<&str>,
) -> std::io::Result<Vec<SyncDirEntry>> {
    let mut entries = Vec::new();
    if !sync_directory.exists() {
        return Ok(entries);
    }

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
        classify_top_dir(&path, local_device_id, &mut entries)?;
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
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
        assert!(three_level_peers
            .iter()
            .all(|e| e.not_visible_to_daemon.is_none()));

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
