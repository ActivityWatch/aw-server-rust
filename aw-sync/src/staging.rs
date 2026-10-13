//! Reset of this device's own push staging database.
//!
//! The staging db under `{sync_root}/{hostname}/{device_id}/` is derived data:
//! the local datastore is the source of truth and the next push rebuilds it.
//! Dropping it is how a device discards copies written by older pushes (the
//! pre-ActivityWatch/aw-server-rust#713 boundary duplicates, see
//! ActivityWatch/aw-android#296) without deduping a peer-visible file in place.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Staging db names a push may have written: `test.db` historically, `sync.db`
/// once the staging rename lands. Both are removed so the reset stays correct
/// on either side of that rename.
const STAGING_DB_NAMES: [&str; 2] = ["test.db", "sync.db"];

/// SQLite sidecars, removed before the main file so a failure part-way never
/// leaves a WAL that a freshly created db would pick up.
const SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// `name` must be exactly one ordinary path component: no separators, `.`,
/// `..`, root, or prefix. Anything else could steer the reset out of the
/// device's own directory.
fn single_component(name: &str, what: &str) -> Result<(), String> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(c)), None) if c == name => Ok(()),
        _ => Err(format!(
            "refusing to reset staging: invalid {what} {name:?}"
        )),
    }
}

fn remove_if_present(path: &Path, removed: &mut Vec<PathBuf>) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => {
            removed.push(path.to_path_buf());
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("failed to remove {}: {e}", path.display())),
    }
}

/// Delete the staging db files in `{sync_root}/{hostname}/{device_id}/`.
///
/// Only this device's own directory is touched: peer directories, other device
/// ids under the same hostname, and any non-staging file are left alone. The
/// resolved directory must lie under `sync_root` after symlinks are resolved.
/// A missing directory or file is not an error, so repeating the reset is a
/// no-op. Returns the paths that were removed.
pub(crate) fn reset_own_staging(
    sync_root: &Path,
    hostname: &str,
    device_id: &str,
) -> Result<Vec<PathBuf>, String> {
    single_component(hostname, "hostname")?;
    single_component(device_id, "device id")?;

    let device_dir = sync_root.join(hostname).join(device_id);
    if !device_dir.exists() {
        return Ok(Vec::new());
    }

    let root = sync_root
        .canonicalize()
        .map_err(|e| format!("failed to resolve sync dir {}: {e}", sync_root.display()))?;
    let dir = device_dir
        .canonicalize()
        .map_err(|e| format!("failed to resolve {}: {e}", device_dir.display()))?;
    if !dir.starts_with(&root) || dir == root {
        return Err(format!(
            "refusing to reset staging: {} resolves outside sync dir {}",
            device_dir.display(),
            root.display()
        ));
    }

    let mut removed = Vec::new();
    for name in STAGING_DB_NAMES {
        for suffix in SIDECAR_SUFFIXES {
            remove_if_present(&dir.join(format!("{name}{suffix}")), &mut removed)?;
        }
        remove_if_present(&dir.join(name), &mut removed)?;
    }
    Ok(removed)
}

/// Read this device's id without creating one.
///
/// `aw_server::device_id::get_device_id` writes a fresh id when the file is
/// missing, which here would point the reset at a directory no push ever
/// wrote. A missing or empty id is therefore an error, not a new id.
pub(crate) fn read_device_id(data_dir: &Path) -> Result<String, String> {
    let path = data_dir.join("device_id");
    let id = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read device id from {}: {e}", path.display()))?;
    let id = id.trim();
    if id.is_empty() {
        return Err(format!("device id file {} is empty", path.display()));
    }
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn removes_own_staging_and_sidecars_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let own = root.join("phone").join("dev-a");
        for f in [
            "test.db",
            "test.db-wal",
            "test.db-shm",
            "sync.db",
            "sync.db-wal",
        ] {
            touch(&own.join(f));
        }
        let unrelated = own.join("notes.txt");
        touch(&unrelated);
        // Same hostname, different device id (e.g. a second phone of the same model).
        let sibling = root.join("phone").join("dev-b").join("test.db");
        touch(&sibling);
        let peer = root.join("desktop").join("dev-c").join("test.db");
        touch(&peer);

        let removed = reset_own_staging(root, "phone", "dev-a").unwrap();

        assert_eq!(removed.len(), 5);
        for f in [
            "test.db",
            "test.db-wal",
            "test.db-shm",
            "sync.db",
            "sync.db-wal",
        ] {
            assert!(!own.join(f).exists(), "{f} should be removed");
        }
        assert!(unrelated.exists());
        assert!(sibling.exists());
        assert!(peer.exists());
    }

    #[test]
    fn repeat_and_missing_dir_are_noops() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(reset_own_staging(root, "phone", "dev-a")
            .unwrap()
            .is_empty());

        touch(&root.join("phone").join("dev-a").join("test.db"));
        assert_eq!(reset_own_staging(root, "phone", "dev-a").unwrap().len(), 1);
        assert!(reset_own_staging(root, "phone", "dev-a")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_path_components_that_escape_the_device_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sync");
        let outside = tmp.path().join("test.db");
        touch(&outside);
        fs::create_dir_all(&root).unwrap();

        for (host, dev) in [
            ("..", "x"),
            ("phone", ".."),
            (".", "."),
            ("phone/dev-a", "x"),
            ("", "dev-a"),
            ("phone", ""),
            ("/etc", "x"),
        ] {
            assert!(
                reset_own_staging(&root, host, dev).is_err(),
                "{host:?}/{dev:?} should be refused"
            );
        }
        assert!(outside.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_device_dir_symlinked_outside_sync_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sync");
        let elsewhere = tmp.path().join("elsewhere");
        touch(&elsewhere.join("test.db"));
        fs::create_dir_all(root.join("phone")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("phone").join("dev-a")).unwrap();

        let err = reset_own_staging(&root, "phone", "dev-a").unwrap_err();
        assert!(err.contains("outside sync dir"), "{err}");
        assert!(elsewhere.join("test.db").exists());
    }

    #[test]
    fn device_id_is_read_not_created() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_device_id(tmp.path()).is_err());
        assert!(!tmp.path().join("device_id").exists());

        fs::write(tmp.path().join("device_id"), "  ").unwrap();
        assert!(read_device_id(tmp.path()).is_err());

        fs::write(tmp.path().join("device_id"), "dev-a\n").unwrap();
        assert_eq!(read_device_id(tmp.path()).unwrap(), "dev-a");
    }
}
