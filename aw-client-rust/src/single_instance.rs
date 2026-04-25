use fs4::fs_std::FileExt;
use log::{debug, error};
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

impl SingleInstance {
    pub fn new(client_name: &str) -> Result<SingleInstance, SingleInstanceError> {
        // On Android, use the app-specific cache directory with flock (Android is Unix-based).
        #[cfg(target_os = "android")]
        {
            use std::path::PathBuf;

            let cache_dir = PathBuf::from("/data/user/0/net.activitywatch.android/cache");
            let lock_dir = cache_dir.join("activitywatch").join("client_locks");
            std::fs::create_dir_all(&lock_dir).map_err(|_| SingleInstanceError::LockDirCreation)?;

            let lockfile = lock_dir.join(client_name);
            debug!("SingleInstance lockfile: {:?}", lockfile);

            return match OpenOptions::new().write(true).create(true).open(&lockfile) {
                Ok(file) => match file.try_lock_exclusive() {
                    Ok(true) => Ok(SingleInstance {
                        file: Some(file),
                        locked: Arc::new(AtomicBool::new(true)),
                    }),
                    Ok(false) => {
                        error!("Another instance is already running");
                        Err(SingleInstanceError::AlreadyRunning)
                    }
                    Err(e) => Err(SingleInstanceError::Io(e)),
                },
                Err(e) => Err(SingleInstanceError::Io(e)),
            };
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
                        error!("Another instance is already running");
                        Err(SingleInstanceError::AlreadyRunning)
                    }
                    Err(e) => Err(SingleInstanceError::Io(e)),
                }
            }

            #[cfg(unix)]
            {
                // On Unix-like systems (Linux, macOS), use flock.
                match OpenOptions::new().write(true).create(true).open(&lockfile) {
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
