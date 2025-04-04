use log::{debug, error};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(not(windows))]
use std::os::unix::io::AsRawFd;

#[derive(Debug)]
pub struct SingleInstance {
    #[cfg(windows)]
    handle: Option<File>,
    #[cfg(not(windows))]
    file: Option<File>,
    lockfile: PathBuf,
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
    pub fn new<P: AsRef<Path>>(
        cache_dir: P,
        client_name: &str,
    ) -> Result<Self, SingleInstanceError> {
        let lock_dir = cache_dir.as_ref().join("client_locks");
        std::fs::create_dir_all(&lock_dir).map_err(|_| SingleInstanceError::LockDirCreation)?;

        let lockfile = lock_dir.join(client_name);
        debug!("SingleInstance lockfile: {:?}", lockfile);

        #[cfg(windows)]
        {
            // On Windows, try to create an exclusive file
            // Remove existing file if it exists (in case of previous crash)
            let _ = std::fs::remove_file(&lockfile);

            match OpenOptions::new()
                .write(true)
                .create(true)
                .create_new(true)
                .open(&lockfile)
            {
                Ok(handle) => Ok(SingleInstance {
                    handle: Some(handle),
                    lockfile,
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
            // On Unix-like systems, use flock
            match OpenOptions::new().write(true).create(true).open(&lockfile) {
                Ok(file) => {
                    let fd = file.as_raw_fd();
                    match unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } {
                        0 => Ok(SingleInstance {
                            file: Some(file),
                            lockfile,
                            locked: Arc::new(AtomicBool::new(true)),
                        }),
                        _ => {
                            error!("Another instance is already running");
                            Err(SingleInstanceError::AlreadyRunning)
                        }
                    }
                }
                Err(e) => Err(SingleInstanceError::Io(e)),
            }
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if self.locked.load(Ordering::SeqCst) {
            #[cfg(windows)]
            {
                // On Windows, drop the handle and remove the file
                self.handle.take();
                let _ = std::fs::remove_file(&self.lockfile);
            }

            #[cfg(unix)]
            {
                // On Unix, the flock is automatically released when the file is closed
                self.file.take();
            }

            self.locked.store(false, Ordering::SeqCst);
        }
    }
}
