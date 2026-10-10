// The public dir helpers use `Result<_, ()>` and are part of the crate API.
#![allow(clippy::result_unit_err)]

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite;

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
    let roots = testing_detection_roots();
    let roots: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
    appname_for_roots(profile, &roots)
}

/// Pure appname resolution against explicit XDG-style parent dirs (testable).
pub fn appname_for_in(profile: &str, data: &Path, config: &Path, cache: &Path) -> String {
    appname_for_roots(profile, &[data, config, cache])
}

fn appname_for_roots(profile: &str, roots: &[&Path]) -> String {
    if profile.is_empty() || profile == "default" {
        return DEFAULT_APPNAME.to_string();
    }
    if profile == TESTING_PROFILE && using_legacy_testing_root_in_roots(profile, roots) {
        return DEFAULT_APPNAME.to_string();
    }
    format!("{DEFAULT_APPNAME}-{profile}")
}

fn platform_roots() -> Option<(PathBuf, PathBuf, PathBuf)> {
    Some((user_data_root()?, user_config_root()?, dirs::cache_dir()?))
}

/// Parents whose on-disk state decides the testing layout: the platform
/// roots plus, on Windows, the Roaming root v0.14.0 wrote to (so its testing
/// files still count before [`module_dir`] has migrated them).
fn testing_detection_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some((data, config, cache)) = platform_roots() {
        roots.extend([data, config, cache]);
    }
    roots.extend(v0140_misplaced_root());
    roots
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

/// Where v0.14.0 put `<appname>/<module>` dirs on Windows: Roaming
/// `%APPDATA%` (ActivityWatch/aw-server-rust#562). `None` elsewhere: other
/// platforms were not affected.
fn v0140_misplaced_root() -> Option<PathBuf> {
    if cfg!(windows) {
        dirs::data_dir()
    } else {
        None
    }
}

/// `<root>/<appname>/<module>` for data/config, creating it.
///
/// On Windows, first recovers anything v0.14.0 wrote to Roaming `%APPDATA%`
/// instead (see [`resolve_or_migrate`]). If that cannot be done safely the
/// Roaming dir is returned instead, so existing data is never shadowed by a
/// new empty database.
#[cfg(not(target_os = "android"))]
pub fn module_dir(root: PathBuf, appname: &str, module: &str) -> PathBuf {
    let target = root.join(appname).join(module);
    let dir = match v0140_misplaced_root() {
        Some(roaming) => {
            let misplaced = roaming.join(appname).join(module);
            resolve_or_migrate_cached(&target, &misplaced, appname == DEFAULT_APPNAME)
        }
        None => target,
    };
    fs::create_dir_all(&dir).expect("Unable to create dir");
    dir
}

/// Read-only counterpart of [`module_dir`]: the dir that currently holds the
/// data for `<appname>/<module>`, without migrating or creating anything.
///
/// Uses the same rule ([`choose_module_dir`]) as the migrating path, so
/// read-only callers (aw-sync `status`, aw-sync reading the server's config)
/// see the same files as the process that owns the dir: before migration
/// that is where the data still is, after a successful migration it is the
/// new location, and after a failed one it is the old location, which the
/// owner then also keeps using.
pub fn module_dir_readonly(root: PathBuf, appname: &str, module: &str) -> PathBuf {
    let target = root.join(appname).join(module);
    match v0140_misplaced_root() {
        Some(roaming) => {
            let misplaced = roaming.join(appname).join(module);
            match choose_module_dir(&target, &misplaced) {
                DirChoice::Target => target,
                DirChoice::Misplaced => misplaced,
            }
        }
        None => target,
    }
}

/// Which of two candidate dirs holds the data in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirChoice {
    /// The correct location (`%LOCALAPPDATA%` on Windows).
    Target,
    /// The v0.14.0 location (Roaming `%APPDATA%`), still to be migrated.
    Misplaced,
}

fn is_marker_name(name: &str) -> bool {
    name.contains(MIGRATING_TAG) || name.contains(MIGRATED_TAG) || name.contains(PRE_MIGRATION_TAG)
}

fn dir_entries(dir: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .filter(|e| !is_marker_name(&e.file_name().to_string_lossy()))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `dir` holds a SQLite database (`sqlite.db`, `sqlite-testing.db`, …).
fn has_database(dir: &Path) -> bool {
    dir_entries(dir).iter().any(|e| {
        e.file_type().map(|t| t.is_file()).unwrap_or(false)
            && e.file_name().to_string_lossy().ends_with(".db")
    })
}

fn has_entries(dir: &Path) -> bool {
    !dir_entries(dir).is_empty()
}

/// Newest event `starttime` (nanoseconds) in any database inside `dir`, or
/// `None` when the directory has no database or the query fails.
///
/// Opens with `SQLITE_OPEN_READ_ONLY` so the WAL is visible but never written
/// or checkpointed.  A missing table (empty / schema-only database) is treated
/// as None.  On any error the database is simply excluded from ranking.
fn db_newest_starttime(dir: &Path) -> Option<i64> {
    let db_path = dir_entries(dir)
        .into_iter()
        .find(|e| {
            e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && e.file_name().to_string_lossy().ends_with(".db")
        })?
        .path();
    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.query_row("SELECT MAX(starttime) FROM events", [], |row| {
        row.get::<_, Option<i64>>(0)
    })
    .ok()
    .flatten()
}

/// Event count across all buckets in `dir`, or 0 on any error.
fn db_event_count(dir: &Path) -> u64 {
    let db_path = dir_entries(dir)
        .into_iter()
        .find(|e| {
            e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && e.file_name().to_string_lossy().ends_with(".db")
        })
        .map(|e| e.path());
    let Some(path) = db_path else { return 0 };
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return 0;
    };
    conn.query_row("SELECT COUNT(*) FROM events", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|n| n as u64)
    .unwrap_or(0)
}

/// The rule for which dir is in use, shared by every reader and writer.
///
/// 1. Both dirs hold a database: inspect content and pick the one whose newest
///    event is most recent (fall back: higher event count, then prefer target).
///    Log the choice and both candidates' newest timestamps.
/// 2. Only target holds a database: it is authoritative.
/// 3. Only the misplaced dir holds a database: it is the one in use (to be
///    migrated as a whole, config included).
/// 4. No database anywhere (e.g. aw-sync's config dir): whichever is
///    non-empty, preferring the target.
pub fn choose_module_dir(target: &Path, misplaced: &Path) -> DirChoice {
    if has_database(target) && has_database(misplaced) {
        // Content-based selection: newest event wins.
        let target_ts = db_newest_starttime(target);
        let misplaced_ts = db_newest_starttime(misplaced);
        log::info!(
            "choose_module_dir: both dirs have databases — \
             target newest={:?}, misplaced newest={:?}",
            target_ts,
            misplaced_ts
        );
        match (target_ts, misplaced_ts) {
            (Some(t), Some(m)) if m > t => DirChoice::Misplaced,
            (None, Some(_)) => {
                // Target database is empty or unreadable; misplaced has data.
                let mc = db_event_count(misplaced);
                if mc > 0 {
                    DirChoice::Misplaced
                } else {
                    DirChoice::Target
                }
            }
            _ => {
                // Target wins on timestamp, or tie → prefer target.
                if target_ts.is_none() && misplaced_ts.is_none() {
                    // Both empty: fall back to count, then target.
                    let tc = db_event_count(target);
                    let mc = db_event_count(misplaced);
                    if mc > tc {
                        DirChoice::Misplaced
                    } else {
                        DirChoice::Target
                    }
                } else {
                    DirChoice::Target
                }
            }
        }
    } else if has_database(target) {
        DirChoice::Target
    } else if has_database(misplaced) {
        DirChoice::Misplaced
    } else if has_entries(target) || !has_entries(misplaced) {
        DirChoice::Target
    } else {
        DirChoice::Misplaced
    }
}

const MIGRATING_TAG: &str = ".migrating-";
const MIGRATED_TAG: &str = ".migrated-";
const PRE_MIGRATION_TAG: &str = ".pre-migration-";

/// Per-process memo of [`resolve_or_migrate`] (target → dir in use), so the
/// several `get_*_dir` calls during startup decide (and log) once.
#[cfg(not(target_os = "android"))]
static RESOLVED_DIRS: std::sync::Mutex<Vec<(PathBuf, PathBuf)>> = std::sync::Mutex::new(Vec::new());

static MIGRATED_DEFAULT_DATABASE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether this process migrated the default profile's database out of the
/// v0.14.0 Roaming location.
///
/// Such a database was created by v0.14.0, whose first-start Python import
/// looked in the wrong directory and so never ran against the real Python
/// aw-server database. aw-server uses this to run that import once.
pub fn migrated_v0140_default_database() -> bool {
    MIGRATED_DEFAULT_DATABASE.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(not(target_os = "android"))]
fn resolve_or_migrate_cached(target: &Path, misplaced: &Path, default_appname: bool) -> PathBuf {
    let mut memo = RESOLVED_DIRS.lock().unwrap();
    if let Some((_, dir)) = memo.iter().find(|(t, _)| t == target) {
        return dir.clone();
    }
    let outcome = resolve_or_migrate(target, misplaced, &|from, to| fs::copy(from, to));
    if outcome.migrated_database && default_appname {
        MIGRATED_DEFAULT_DATABASE.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    memo.push((target.to_path_buf(), outcome.dir.clone()));
    outcome.dir
}

/// Result of [`resolve_or_migrate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    /// The dir to use.
    pub dir: PathBuf,
    /// A database was migrated into `target` by this call.
    pub migrated_database: bool,
}

type CopyFn<'a> = dyn Fn(&Path, &Path) -> std::io::Result<u64> + 'a;

/// Recover a module dir that v0.14.0 wrote to the wrong place (Windows:
/// Roaming `%APPDATA%` instead of `%LOCALAPPDATA%`, see #562).
///
/// Decides with [`choose_module_dir`]. If the misplaced dir is the one in
/// use, it is migrated with [`migrate_dir`]; if that fails, the misplaced dir
/// keeps being used (and migration is retried next start) rather than
/// starting from an empty target. Nothing is ever deleted or overwritten.
pub fn resolve_or_migrate(target: &Path, misplaced: &Path, copy: &CopyFn<'_>) -> MigrationOutcome {
    match choose_module_dir(target, misplaced) {
        DirChoice::Target => {
            if has_entries(misplaced) {
                let what = if has_database(misplaced) {
                    "a database (data recorded while running v0.14.0)"
                } else {
                    "files"
                };
                warn!(
                    "{misplaced:?} holds {what}, likely from ActivityWatch v0.14.0, which wrote \
                     to Roaming %APPDATA% by mistake. It is not used and was not migrated, \
                     because {target:?} already has data of its own, which stays in use. \
                     Nothing was changed; see https://docs.activitywatch.net/en/latest/directories.html"
                );
            }
            MigrationOutcome {
                dir: target.to_path_buf(),
                migrated_database: false,
            }
        }
        DirChoice::Misplaced => match MigrationLock::acquire(target).and_then(|_lock| {
            // Re-check under the lock: another process may have completed
            // the migration between our first look and taking the lock.
            if choose_module_dir(target, misplaced) == DirChoice::Target {
                return Err(io_err("already migrated by another process".into()));
            }
            migrate_dir(target, misplaced, copy)
        }) {
            Ok(()) => MigrationOutcome {
                dir: target.to_path_buf(),
                migrated_database: has_database(target),
            },
            Err(e) => {
                // Re-apply the shared rule rather than assume: normally this
                // is still `misplaced`, but another process (e.g. a second
                // server sharing a legacy testing dir) may have finished the
                // same migration meanwhile.
                let dir = match choose_module_dir(target, misplaced) {
                    DirChoice::Target => target,
                    DirChoice::Misplaced => misplaced,
                };
                warn!(
                    "Could not migrate {misplaced:?} to {target:?} ({e}). Nothing was changed; \
                     using {dir:?} and retrying on next start if needed."
                );
                MigrationOutcome {
                    dir: dir.to_path_buf(),
                    migrated_database: false,
                }
            }
        },
    }
}

/// Cross-process lock around choosing and migrating one dir: an exclusive
/// OS file lock (`File::lock`) on `<name>.migration-lock` next to `target`.
/// The OS releases it when the holder exits, also on a crash, so there is no
/// staleness to guess at; the file itself is never deleted (deleting a lock
/// file by path can let two processes hold "the" lock). A second process
/// starting meanwhile (e.g. default and legacy testing servers sharing a
/// dir) blocks until the migration is done and then re-applies the rule,
/// instead of falling back to the old dir while it is being copied.
struct MigrationLock(#[allow(dead_code)] fs::File);

impl MigrationLock {
    fn path_for(target: &Path) -> PathBuf {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        target.with_file_name(format!("{name}.migration-lock"))
    }

    fn acquire(target: &Path) -> std::io::Result<Self> {
        let path = Self::path_for(target);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                info!("Waiting for another process to finish migrating {target:?}");
                file.lock()?;
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
        Ok(MigrationLock(file))
    }
}

fn migration_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// `dir` with `tag` and a unique suffix appended to its name, in the same parent.
fn tagged_sibling(dir: &Path, tag: &str, suffix: &str) -> PathBuf {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut candidate = dir.with_file_name(format!("{name}{tag}{suffix}"));
    let mut n = 1;
    while candidate.exists() {
        n += 1;
        candidate = dir.with_file_name(format!("{name}{tag}{suffix}-{n}"));
    }
    candidate
}

fn files_equal(a: &Path, b: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let (mut fa, mut fb) = (fs::File::open(a)?, fs::File::open(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

fn io_err(msg: String) -> std::io::Error {
    std::io::Error::other(msg)
}

/// Recursively copy `from` into a new dir `to`, verifying every file
/// (size and content) after copying.
fn copy_tree_verified(from: &Path, to: &Path, copy: &CopyFn<'_>) -> std::io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = fs::symlink_metadata(&src)?.file_type();
        if kind.is_dir() {
            copy_tree_verified(&src, &dst, copy)?;
        } else if kind.is_file() {
            copy(&src, &dst)?;
            if !files_equal(&src, &dst)? {
                return Err(io_err(format!(
                    "{src:?} changed or was not copied faithfully"
                )));
            }
        } else {
            return Err(io_err(format!("{src:?} is not a regular file or dir")));
        }
    }
    Ok(())
}

/// Move `misplaced` to `target` by copy-then-verify, never deleting or
/// overwriting anything:
///
/// 1. Copy `misplaced` to a staging dir next to `target` (same volume, so
///    the final step is an atomic rename even when Roaming is redirected to
///    another drive or a network share). Every file is compared byte for
///    byte with its source, and each plain SQLite database is opened with
///    its `-wal`/`-shm` in place and must pass `PRAGMA quick_check`.
/// 2. If `target` exists and is not empty (it holds no database, or the
///    caller would not be migrating), rename it aside to
///    `<name>.pre-migration-<date>`.
/// 3. Rename the staging dir to `target`.
/// 4. Rename `misplaced` to `<name>.migrated-<date>`: kept as a backup, and
///    no longer seen by [`choose_module_dir`].
///
/// Any failure before step 4 undoes the earlier steps (removing only the
/// staging copy this call made) and returns the error, leaving both dirs
/// as they were. A failure in step 4 is only logged: the copy is complete
/// and verified, and the leftover original is reported on later starts.
pub fn migrate_dir(target: &Path, misplaced: &Path, copy: &CopyFn<'_>) -> std::io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| io_err(format!("{target:?} has no parent")))?;
    fs::create_dir_all(parent)?;
    let stamp = migration_stamp();
    let staging = tagged_sibling(target, MIGRATING_TAG, &uuid::Uuid::new_v4().to_string());

    let verified = copy_tree_verified(misplaced, &staging, copy).and_then(|()| {
        for entry in dir_entries(&staging) {
            let path = entry.path();
            if path.is_file() && path.to_string_lossy().ends_with(".db") {
                aw_datastore::sqlite_quick_check(&path).map_err(io_err)?;
            }
        }
        Ok(())
    });
    if let Err(e) = verified {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }

    if has_database(target) {
        // Someone else migrated (or created a database) meanwhile: never
        // move a database aside.
        let _ = fs::remove_dir_all(&staging);
        return Err(io_err(format!("{target:?} got a database meanwhile")));
    }
    let aside = if target.exists() {
        if fs::remove_dir(target).is_ok() {
            None // was empty
        } else {
            let aside = tagged_sibling(target, PRE_MIGRATION_TAG, &stamp);
            if let Err(e) = fs::rename(target, &aside) {
                let _ = fs::remove_dir_all(&staging);
                return Err(e);
            }
            warn!("Moved existing {target:?} (no database) aside to {aside:?}");
            Some(aside)
        }
    } else {
        None
    };

    if let Err(e) = fs::rename(&staging, target) {
        if let Some(aside) = &aside {
            let _ = fs::rename(aside, target);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }

    let backup = tagged_sibling(misplaced, MIGRATED_TAG, &stamp);
    match fs::rename(misplaced, &backup) {
        Ok(()) => info!(
            "Migrated {misplaced:?} (written by ActivityWatch v0.14.0) to {target:?}; \
             the original is kept at {backup:?}"
        ),
        Err(e) => warn!(
            "Migrated {misplaced:?} (written by ActivityWatch v0.14.0) to {target:?}, but \
             could not rename the original to {backup:?}: {e}. It is no longer used."
        ),
    }
    Ok(())
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
    using_legacy_testing_root_in_roots(profile, &[data, config, cache])
}

/// [`using_legacy_testing_root_in`] over any number of parent dirs. Each
/// step of the rule is checked across all roots before the next, so an
/// existing `activitywatch-testing/` in any root wins over legacy files in
/// any other (e.g. Local vs the v0.14.0 Roaming root on Windows).
fn using_legacy_testing_root_in_roots(profile: &str, roots: &[&Path]) -> bool {
    if profile != TESTING_PROFILE {
        return false;
    }
    if roots.iter().any(|root| root.join(TESTING_APPNAME).is_dir()) {
        return false;
    }
    roots
        .iter()
        .any(|root| legacy_testing_artifacts_in_app_root(&root.join(DEFAULT_APPNAME)))
}

/// Whether `profile=testing` should stay on the shared `activitywatch` root.
pub fn using_legacy_testing_root(profile: &str) -> bool {
    let roots = testing_detection_roots();
    let roots: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
    using_legacy_testing_root_in_roots(profile, &roots)
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

/// Read-only counterpart of [`get_data_dir`]: the server's data dir as
/// resolved by [`module_dir_readonly`], without creating or migrating it.
/// For other processes (aw-sync) that store files next to the server's data.
#[cfg(not(target_os = "android"))]
pub fn data_dir_path() -> Result<PathBuf, ()> {
    Ok(module_dir_readonly(
        user_data_root().ok_or(())?,
        &appname(),
        "aw-server-rust",
    ))
}

#[cfg(target_os = "android")]
pub fn data_dir_path() -> Result<PathBuf, ()> {
    get_data_dir()
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

/// (root, target, misplaced): a fake `%LOCALAPPDATA%` and Roaming
/// `%APPDATA%` pair for `activitywatch/aw-server-rust`. Only `misplaced`
/// is created.
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

#[cfg(test)]
fn real_copy(from: &Path, to: &Path) -> std::io::Result<u64> {
    fs::copy(from, to)
}

/// A real SQLite database in `dir` whose data is only in its `-wal` (as left
/// by a process that did not shut down cleanly), with no open handles.
#[cfg(test)]
fn plant_wal_db(dir: &Path) {
    let scratch = dir.with_file_name(format!("scratch-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&scratch).unwrap();
    let conn = rusqlite::Connection::open(scratch.join("sqlite.db")).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    conn.execute_batch("CREATE TABLE t (x TEXT); INSERT INTO t VALUES ('history');")
        .unwrap();
    // Snapshot while open, so the WAL is not checkpointed away on close.
    for name in ["sqlite.db", "sqlite.db-wal"] {
        fs::copy(scratch.join(name), dir.join(name)).unwrap();
    }
    drop(conn);
    let _ = fs::remove_dir_all(scratch);
}

#[cfg(test)]
fn sibling_with(dir: &Path, tag: &str) -> Option<PathBuf> {
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    fs::read_dir(dir.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{name}{tag}"))
        })
}

/// v0.14.0 wrote the only database to Roaming: it is copied (with its WAL
/// and subdirs), verified, and the original is kept as `.migrated-<date>`.
#[test]
fn test_migrate_roaming_only_database() {
    let (root, target, misplaced) = migration_dirs();
    plant_wal_db(&misplaced);
    assert!(misplaced.join("sqlite.db-wal").exists());
    fs::write(misplaced.join("config.toml"), b"port = 5666").unwrap();
    fs::create_dir_all(misplaced.join("aw-sync")).unwrap();
    fs::write(misplaced.join("aw-sync/last-sync-report.json"), b"{}").unwrap();

    assert_eq!(choose_module_dir(&target, &misplaced), DirChoice::Misplaced);
    let out = resolve_or_migrate(&target, &misplaced, &real_copy);
    assert_eq!(
        out,
        MigrationOutcome {
            dir: target.clone(),
            migrated_database: true
        }
    );
    let migrated = rusqlite::Connection::open(target.join("sqlite.db")).unwrap();
    let x: String = migrated
        .query_row("SELECT x FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(x, "history", "data only in the WAL must survive the copy");
    assert_eq!(
        fs::read(target.join("config.toml")).unwrap(),
        b"port = 5666"
    );
    assert!(target.join("aw-sync/last-sync-report.json").exists());

    assert!(!misplaced.exists(), "original is renamed, not left in use");
    let backup = sibling_with(&misplaced, MIGRATED_TAG).expect("original kept as backup");
    assert!(backup.join("sqlite.db").exists(), "nothing is deleted");
    assert!(
        sibling_with(&target, MIGRATING_TAG).is_none(),
        "no staging left"
    );

    // Second start: nothing left to do, and the backup is not re-migrated.
    assert_eq!(choose_module_dir(&target, &misplaced), DirChoice::Target);
    assert!(!resolve_or_migrate(&target, &misplaced, &real_copy).migrated_database);
    let _ = fs::remove_dir_all(root);
}

/// Local already has a database (pre-v0.14.0 install): it stays in use and
/// the Roaming dir is not touched at all, not even its non-db files.
#[test]
fn test_local_database_is_authoritative() {
    let (root, target, misplaced) = migration_dirs();
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("sqlite.db"), b"local").unwrap();
    fs::write(misplaced.join("sqlite.db"), b"roaming").unwrap();
    fs::write(misplaced.join("sqlite.db-wal"), b"roaming-wal").unwrap();
    fs::write(misplaced.join("config.toml"), b"roaming-cfg").unwrap();

    let out = resolve_or_migrate(&target, &misplaced, &real_copy);
    assert_eq!(out.dir, target);
    assert!(!out.migrated_database);
    assert_eq!(fs::read(target.join("sqlite.db")).unwrap(), b"local");
    assert!(!target.join("sqlite.db-wal").exists());
    assert!(!target.join("config.toml").exists());
    assert_eq!(fs::read(misplaced.join("sqlite.db")).unwrap(), b"roaming");
    assert_eq!(
        fs::read(misplaced.join("config.toml")).unwrap(),
        b"roaming-cfg"
    );
    let _ = fs::remove_dir_all(root);
}

/// Create a real SQLite database in `dir/sqlite.db` with one event at
/// `starttime` (nanoseconds since epoch).  Uses the same schema as aw-datastore.
#[cfg(test)]
fn plant_events_db(dir: &Path, starttime: i64) {
    fs::create_dir_all(dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("sqlite.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS buckets (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL);
         CREATE TABLE IF NOT EXISTS events (
             id INTEGER PRIMARY KEY,
             bucketrow INTEGER NOT NULL,
             starttime INTEGER NOT NULL,
             endtime INTEGER NOT NULL,
             datastr TEXT NOT NULL
         );
         INSERT OR IGNORE INTO buckets (name) VALUES ('test-bucket');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO events (bucketrow, starttime, endtime, datastr)
         VALUES ((SELECT id FROM buckets WHERE name='test-bucket'), ?1, ?1, '{}')",
        rusqlite::params![starttime],
    )
    .unwrap();
}

/// Roaming has newer events than a stale Local database (row 2/3 from the
/// issue): content-based selection must prefer Roaming even though Local
/// exists.
#[test]
fn test_content_selection_prefers_newer_roaming_over_stale_local() {
    let (root, target, misplaced) = migration_dirs();
    // Stale Local: last event months ago.
    plant_events_db(&target, 1_700_000_000_000_000_000i64);
    // Recent Roaming: months of v0.14.0 beta data.
    plant_events_db(&misplaced, 1_728_000_000_000_000_000i64);

    assert_eq!(
        choose_module_dir(&target, &misplaced),
        DirChoice::Misplaced,
        "Roaming has newer events; content-based selection must prefer it"
    );
    let _ = fs::remove_dir_all(root);
}

/// Local has newer events (normal upgrade path, pre-v0.14.0 install):
/// content-based selection must keep Local.
#[test]
fn test_content_selection_keeps_newer_local() {
    let (root, target, misplaced) = migration_dirs();
    // Recent Local: ongoing pre-v0.14.0 Python data.
    plant_events_db(&target, 1_728_000_000_000_000_000i64);
    // Older Roaming: only a few days of v0.14.0 beta.
    plant_events_db(&misplaced, 1_700_000_000_000_000_000i64);

    assert_eq!(
        choose_module_dir(&target, &misplaced),
        DirChoice::Target,
        "Local has newer events; must stay in use"
    );
    let _ = fs::remove_dir_all(root);
}

/// Equal newest timestamps: prefer Local (target) as the tiebreaker.
#[test]
fn test_content_selection_ties_prefer_target() {
    let (root, target, misplaced) = migration_dirs();
    let ts = 1_728_000_000_000_000_000i64;
    plant_events_db(&target, ts);
    plant_events_db(&misplaced, ts);

    assert_eq!(
        choose_module_dir(&target, &misplaced),
        DirChoice::Target,
        "Equal timestamps: Local (target) wins the tie"
    );
    let _ = fs::remove_dir_all(root);
}

/// Cross-drive / network-share Roaming, or any other copy failure: nothing
/// changes, the Roaming dir stays in use (no empty Local database is
/// created to shadow it later), and the attempt is retried next start.
#[test]
fn test_failed_copy_keeps_using_misplaced_dir() {
    let (root, target, misplaced) = migration_dirs();
    fs::write(misplaced.join("config.toml"), b"cfg").unwrap();
    fs::write(misplaced.join("sqlite.db"), b"db").unwrap();
    let failing = |from: &Path, to: &Path| {
        if from.ends_with("sqlite.db") {
            Err(std::io::Error::other("simulated: not same device"))
        } else {
            fs::copy(from, to)
        }
    };
    let out = resolve_or_migrate(&target, &misplaced, &failing);
    assert_eq!(out.dir, misplaced);
    assert!(!out.migrated_database);
    assert!(!target.exists(), "no (empty) target is created");
    assert!(
        sibling_with(&target, MIGRATING_TAG).is_none(),
        "staging cleaned up"
    );
    assert_eq!(fs::read(misplaced.join("sqlite.db")).unwrap(), b"db");
    assert_eq!(fs::read(misplaced.join("config.toml")).unwrap(), b"cfg");
    // Readers agree with the server while it keeps using Roaming.
    assert_eq!(choose_module_dir(&target, &misplaced), DirChoice::Misplaced);
    // The next start (copy works now) completes the migration.
    assert_eq!(
        resolve_or_migrate(&target, &misplaced, &real_copy).dir,
        target
    );
    assert_eq!(fs::read(target.join("sqlite.db")).unwrap(), b"db");
    let _ = fs::remove_dir_all(root);
}

/// Another process finishing the same migration meanwhile: its database is
/// never moved aside, and this process switches to it too.
#[test]
fn test_concurrent_migration_never_moves_a_database_aside() {
    let (root, target, misplaced) = migration_dirs();
    fs::write(misplaced.join("sqlite.db"), b"db").unwrap();
    let racing = |from: &Path, to: &Path| {
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("sqlite.db"), b"other").unwrap();
        fs::copy(from, to)
    };
    let out = resolve_or_migrate(&target, &misplaced, &racing);
    assert_eq!(out.dir, target);
    assert_eq!(fs::read(target.join("sqlite.db")).unwrap(), b"other");
    assert!(sibling_with(&target, PRE_MIGRATION_TAG).is_none());
    assert!(sibling_with(&target, MIGRATING_TAG).is_none());
    assert_eq!(fs::read(misplaced.join("sqlite.db")).unwrap(), b"db");
    let _ = fs::remove_dir_all(root);
}

/// A migration in progress elsewhere (lock held) is waited for, then the
/// rule is re-applied: no fallback to the dir being copied, no second copy.
#[test]
fn test_migration_lock_waits_for_concurrent_migration() {
    let (root, target, misplaced) = migration_dirs();
    fs::write(misplaced.join("sqlite.db"), b"db").unwrap();
    let held = MigrationLock::acquire(&target).unwrap();
    let (t2, m2) = (target.clone(), misplaced.clone());
    let waiter = std::thread::spawn(move || resolve_or_migrate(&t2, &m2, &real_copy));
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!waiter.is_finished(), "must wait while the lock is held");
    // The holder completes the migration, then releases the lock.
    migrate_dir(&target, &misplaced, &real_copy).unwrap();
    drop(held);
    let out = waiter.join().unwrap();
    assert_eq!(out.dir, target);
    assert_eq!(fs::read(target.join("sqlite.db")).unwrap(), b"db");
    assert!(sibling_with(&target, PRE_MIGRATION_TAG).is_none());
    let _ = fs::remove_dir_all(root);
}

/// A copy that does not match its source is detected and rejected.
#[test]
fn test_unfaithful_copy_is_rejected() {
    let (root, target, misplaced) = migration_dirs();
    fs::write(misplaced.join("sqlite.db"), b"db").unwrap();
    let truncating = |_: &Path, to: &Path| fs::write(to, b"d").map(|_| 1);
    let out = resolve_or_migrate(&target, &misplaced, &truncating);
    assert_eq!(out.dir, misplaced);
    assert!(!target.exists());
    let _ = fs::remove_dir_all(root);
}

/// A database that fails `PRAGMA quick_check` is not switched to.
#[test]
fn test_corrupt_database_is_not_migrated() {
    let (root, target, misplaced) = migration_dirs();
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend(std::iter::repeat_n(0xAB, 4096));
    fs::write(misplaced.join("sqlite.db"), &bytes).unwrap();
    let out = resolve_or_migrate(&target, &misplaced, &real_copy);
    assert_eq!(out.dir, misplaced);
    assert!(!target.exists());
    assert_eq!(fs::read(misplaced.join("sqlite.db")).unwrap(), bytes);
    let _ = fs::remove_dir_all(root);
}

/// Local has files but no database, Roaming has the database: the Roaming
/// dir (with its config) wins as a whole; Local's files are moved aside,
/// not overwritten or mixed in.
#[test]
fn test_local_files_without_database_are_moved_aside() {
    let (root, target, misplaced) = migration_dirs();
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("config.toml"), b"local-cfg").unwrap();
    fs::write(misplaced.join("config.toml"), b"roaming-cfg").unwrap();
    fs::write(misplaced.join("sqlite.db"), b"db").unwrap();
    // Before migrating, readers already see Roaming's config, like the server.
    assert_eq!(choose_module_dir(&target, &misplaced), DirChoice::Misplaced);
    let out = resolve_or_migrate(&target, &misplaced, &real_copy);
    assert_eq!(out.dir, target);
    assert_eq!(
        fs::read(target.join("config.toml")).unwrap(),
        b"roaming-cfg"
    );
    let aside = sibling_with(&target, PRE_MIGRATION_TAG).expect("local files kept");
    assert_eq!(fs::read(aside.join("config.toml")).unwrap(), b"local-cfg");
    let _ = fs::remove_dir_all(root);
}

/// Config-only dirs (aw-sync): an existing Local config wins and Roaming is
/// left alone; a Roaming-only config is migrated.
#[test]
fn test_config_only_dirs() {
    let (root, target, misplaced) = migration_dirs();
    fs::write(misplaced.join("config.toml"), b"[daemon]\npull = true\n").unwrap();
    assert_eq!(choose_module_dir(&target, &misplaced), DirChoice::Misplaced);
    let out = resolve_or_migrate(&target, &misplaced, &real_copy);
    assert_eq!(out.dir, target);
    assert!(!out.migrated_database);
    assert!(fs::read_to_string(target.join("config.toml"))
        .unwrap()
        .contains("pull = true"));

    let (root2, target2, misplaced2) = migration_dirs();
    fs::create_dir_all(&target2).unwrap();
    fs::write(target2.join("config.toml"), b"local").unwrap();
    fs::write(misplaced2.join("config.toml"), b"roaming").unwrap();
    assert_eq!(choose_module_dir(&target2, &misplaced2), DirChoice::Target);
    assert_eq!(
        resolve_or_migrate(&target2, &misplaced2, &real_copy).dir,
        target2
    );
    assert_eq!(
        fs::read(misplaced2.join("config.toml")).unwrap(),
        b"roaming"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(root2);
}

#[test]
fn test_migrate_noop_without_misplaced_dir() {
    let (root, target, misplaced) = migration_dirs();
    fs::remove_dir_all(&misplaced).unwrap();
    assert_eq!(choose_module_dir(&target, &misplaced), DirChoice::Target);
    let out = resolve_or_migrate(&target, &misplaced, &real_copy);
    assert_eq!(out.dir, target);
    assert!(!target.exists(), "resolution alone creates nothing");
    let _ = fs::remove_dir_all(root);
}

/// v0.14.0's legacy testing files under Roaming still select the legacy
/// layout before they are migrated (so the migration looks in the right
/// dir)…
#[test]
fn test_roaming_legacy_testing_files_select_legacy_layout() {
    let (_root, local, _config, _cache) = fake_roots();
    let (_root2, roaming, _, _) = fake_roots();
    plant_legacy_testing_db(&roaming);
    assert!(using_legacy_testing_root_in_roots(
        "testing",
        &[&local, &roaming]
    ));
    assert_eq!(
        appname_for_roots("testing", &[&local, &roaming]),
        "activitywatch"
    );
    let _ = fs::remove_dir_all(_root);
    let _ = fs::remove_dir_all(_root2);
}

/// …but an existing isolated `activitywatch-testing/` in Local keeps
/// priority over them, so appname, db and config filenames stay consistent.
#[test]
fn test_local_testing_root_wins_over_roaming_legacy_files() {
    let (_root, local, _config, _cache) = fake_roots();
    let (_root2, roaming, _, _) = fake_roots();
    plant_legacy_testing_db(&roaming);
    fs::create_dir_all(local.join("activitywatch-testing")).unwrap();
    assert!(!using_legacy_testing_root_in_roots(
        "testing",
        &[&local, &roaming]
    ));
    assert_eq!(
        appname_for_roots("testing", &[&local, &roaming]),
        "activitywatch-testing"
    );
    let _ = fs::remove_dir_all(_root);
    let _ = fs::remove_dir_all(_root2);
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
/// migration (see [`resolve_or_migrate`]).
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
