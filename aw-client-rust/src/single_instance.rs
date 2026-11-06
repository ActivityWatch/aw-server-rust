use fs4::fs_std::FileExt;
use log::debug;
use std::fs::{File, OpenOptions};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug)]
pub struct SingleInstance {
    file: Option<File>,
    locked: Arc<AtomicBool>,
}

#[derive(Debug, thiserror::Error)]
pub enum SingleInstanceError {
    #[error("Another instance is already running")]
    AlreadyRunning,
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Failed to create lock directory")]
    LockDirCreation,
}

/// The running app's cache dir, as set by [`set_android_cache_dir`].
#[cfg(target_os = "android")]
static ANDROID_CACHE_DIR: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// Set the Android app's cache dir (`Context.getCacheDir()`), where lock files
/// are kept. The app's private dirs depend on its package (debug builds add a
/// `.debug` suffix) and on the Android user (work profiles are not user 0), so
/// they can't be hardcoded.
#[cfg(target_os = "android")]
pub fn set_android_cache_dir(path: &std::path::Path) {
    *ANDROID_CACHE_DIR.lock().unwrap() = Some(path.to_path_buf());
}

/// The configured cache dir, else `/data/data/<package>/cache` for the running
/// package (the package name is the process name in `/proc/self/cmdline`).
#[cfg(target_os = "android")]
fn android_cache_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = ANDROID_CACHE_DIR.lock().unwrap().clone() {
        return Some(dir);
    }
    let cmdline = std::fs::read("/proc/self/cmdline").ok()?;
    let process = cmdline.split(|b| *b == 0).next()?;
    let process = std::str::from_utf8(process).ok()?;
    // Service processes are named "<package>:<process>".
    let package = process.split(':').next()?;
    if package.is_empty() || package.contains('/') {
        return None;
    }
    Some(
        std::path::Path::new("/data/data")
            .join(package)
            .join("cache"),
    )
}

impl SingleInstance {
    pub fn new(client_name: &str) -> Result<SingleInstance, SingleInstanceError> {
        // On Android, use the app-specific cache directory with flock (Android is Unix-based).
        #[cfg(target_os = "android")]
        {
            let cache_dir = android_cache_dir().ok_or(SingleInstanceError::LockDirCreation)?;
            let lock_dir = cache_dir.join("activitywatch").join("client_locks");
            std::fs::create_dir_all(&lock_dir).map_err(|_| SingleInstanceError::LockDirCreation)?;

            let lockfile = lock_dir.join(client_name);
            debug!("SingleInstance lockfile: {:?}", lockfile);

            match OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&lockfile)
            {
                Ok(file) => match file.try_lock_exclusive() {
                    Ok(true) => Ok(SingleInstance {
                        file: Some(file),
                        locked: Arc::new(AtomicBool::new(true)),
                    }),
                    Ok(false) => {
                        // Qualified so non-Android unix builds don't carry an unused `error` import
                        log::error!("Another instance is already running");
                        Err(SingleInstanceError::AlreadyRunning)
                    }
                    Err(e) => Err(SingleInstanceError::Io(e)),
                },
                Err(e) => Err(SingleInstanceError::Io(e)),
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            use dirs::cache_dir;

            let cache_dir = cache_dir().ok_or(SingleInstanceError::LockDirCreation)?;
            let lock_dir = cache_dir.join("activitywatch").join("client_locks");
            std::fs::create_dir_all(&lock_dir).map_err(|_| SingleInstanceError::LockDirCreation)?;

            let lockfile = lock_dir.join(client_name);
            debug!("SingleInstance lockfile: {:?}", lockfile);

            #[cfg(windows)]
            {
                // On Windows, try to create an exclusive file.
                // Remove existing file if it exists (in case of previous crash).
                let _ = std::fs::remove_file(&lockfile);

                match OpenOptions::new()
                    .write(true)
                    .create(true)
                    .create_new(true)
                    .open(&lockfile)
                {
                    Ok(file) => Ok(SingleInstance {
                        file: Some(file),
                        locked: Arc::new(AtomicBool::new(true)),
                    }),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        log::error!("Another instance is already running");
                        Err(SingleInstanceError::AlreadyRunning)
                    }
                    Err(e) => Err(SingleInstanceError::Io(e)),
                }
            }

            #[cfg(unix)]
            {
                // On Unix-like systems (Linux, macOS), use flock.
                match OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&lockfile)
                {
                    Ok(file) => match file.try_lock_exclusive() {
                        Ok(true) => Ok(SingleInstance {
                            file: Some(file),
                            locked: Arc::new(AtomicBool::new(true)),
                        }),
                        Ok(false) => Err(SingleInstanceError::AlreadyRunning),
                        Err(e) => Err(SingleInstanceError::Io(e)),
                    },
                    Err(e) => Err(SingleInstanceError::Io(e)),
                }
            }
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if self.locked.load(Ordering::SeqCst) {
            // Drop the file handle, releasing the lock on Unix and Windows.
            self.file.take();
            self.locked.store(false, Ordering::SeqCst);
        }
    }
}
