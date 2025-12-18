// What needs to be done:
//  - [x] Setup local sync bucket
//  - [x] Import local buckets and sync events from aw-server (either through API or through creating a read-only Datastore)
//  - [x] Import buckets and sync events from remotes
//  - [x] Add CLI arguments
//     - [x] For which local server to use
//     - [x] For which sync dir to use
//     - [x] Date to start syncing from

#[macro_use]
extern crate log;
extern crate chrono;
extern crate serde;
extern crate serde_json;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use std::error::Error;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

use aw_client_rust::blocking::AwClient;

mod accessmethod;
mod dirs;
mod sync;
mod sync_wrapper;
mod util;

#[derive(Parser)]
#[clap(version = "0.1", author = "Erik Bjäreholt")]
struct Opts {
    #[clap(subcommand)]
    command: Option<Commands>,

    /// Host of instance to connect to.
    #[clap(long, default_value = "127.0.0.1")]
    host: String,

    /// Port of instance to connect to.
    #[clap(long)]
    port: Option<u16>,

    /// Convenience option for using the default testing host and port.
    #[clap(long)]
    testing: bool,

    /// Full path to sync directory.
    /// If not specified, use AW_SYNC_DIR env var, or default to ~/ActivityWatchSync
    #[clap(long)]
    sync_dir: Option<PathBuf>,

    /// Enable debug logging.
    #[clap(long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Daemon subcommand
    /// Starts aw-sync as a daemon, which will sync every 5 minutes.
    Daemon {
        /// Date to start syncing from.
        /// If not specified, start from beginning.
        /// Format: YYYY-MM-DD
        #[clap(long, value_parser=parse_start_date)]
        start_date: Option<DateTime<Utc>>,

        /// Specify buckets to sync using a comma-separated list.
        /// By default, all buckets are synced.
        #[clap(long, value_parser=parse_list)]
        buckets: Option<Vec<String>>,

        /// Full path to sync db file
        /// Useful for syncing buckets from a specific db file in the sync directory.
        /// Must be a valid absolute path to a file in the sync directory.
        #[clap(long)]
        sync_db: Option<PathBuf>,
    },

    /// Sync subcommand
    ///
    /// Syncs data between local aw-server and sync directory.
    /// First pulls remote buckets from the sync directory to the local aw-server.
    /// Then pushes local buckets from the aw-server to the local sync directory.
    Sync {
        /// Host(s) to pull from, comma separated. Will pull from all hosts if not specified.
        #[clap(long, value_parser=parse_list)]
        host: Option<Vec<String>>,

        /// Date to start syncing from.
        /// If not specified, start from beginning.
        /// Format: YYYY-MM-DD
        #[clap(long, value_parser=parse_start_date)]
        start_date: Option<DateTime<Utc>>,

        /// Specify buckets to sync using a comma-separated list.
        /// By default, all buckets are synced.
        #[clap(long, value_parser=parse_list)]
        buckets: Option<Vec<String>>,

        /// Mode to sync in. Can be "push", "pull", or "both".
        /// Defaults to "both".
        #[clap(long, default_value = "both")]
        mode: sync::SyncMode,

        /// Full path to sync db file
        /// Useful for syncing buckets from a specific db file in the sync directory.
        /// Must be a valid absolute path to a file in the sync directory.
        #[clap(long)]
        sync_db: Option<PathBuf>,
    },
    /// List buckets and their sync status.
    List {},
}

fn parse_start_date(arg: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    chrono::NaiveDate::parse_from_str(arg, "%Y-%m-%d")
        .map(|nd| nd.and_time(chrono::NaiveTime::MIN).and_utc())
}

fn parse_list(arg: &str) -> Result<Vec<String>, clap::Error> {
    // If the argument is empty or just whitespace, return an empty Vec
    // This handles the case when --buckets is used without a value
    if arg.trim().is_empty() {
        return Ok(vec![]);
    }

    // Otherwise, split by comma, trim each part, and filter out empty strings
    Ok(arg
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

fn main() -> Result<(), Box<dyn Error>> {
    let opts: Opts = Opts::parse();
    let verbose = opts.verbose;

    info!("Started aw-sync...");

    aw_server::logging::setup_logger("aw-sync", opts.testing, verbose)?;

    // if sync_dir, set env var
    if let Some(sync_dir) = opts.sync_dir {
        if !sync_dir.is_absolute() {
            Err("Sync dir must be absolute")?
        }

        info!("Using sync dir: {}", &sync_dir.display());
        std::env::set_var("AW_SYNC_DIR", sync_dir);
    }

    let port = opts
        .port
        .map(|a| Ok(a))
        .unwrap_or_else(|| util::get_server_port(opts.testing))?;

    let client = AwClient::new(&opts.host, port, "aw-sync")?;

    // if opts.command is None, then we're using the default subcommand (Daemon)
    match opts.command.unwrap_or(Commands::Daemon {
        start_date: None,
        buckets: None,
        sync_db: None,
    }) {
        // Start daemon
        Commands::Daemon {
            start_date,
            buckets,
            sync_db,
        } => {
            info!("Starting daemon...");

            let effective_buckets = buckets;

            daemon(&client, start_date, effective_buckets, sync_db)?;
        }
        // Perform sync
        Commands::Sync {
            host,
            start_date,
            buckets,
            mode,
            sync_db,
        } => {
            let effective_buckets = buckets;

            // If advanced options are provided, use advanced sync mode
            if start_date.is_some() || effective_buckets.is_some() || sync_db.is_some() {
                let sync_dir = dirs::get_sync_dir()?;
                if let Some(db_path) = &sync_db {
                    info!("Using sync db: {}", &db_path.display());

                    if !db_path.is_absolute() {
                        Err("Sync db path must be absolute")?
                    }
                    if !db_path.starts_with(&sync_dir) {
                        Err("Sync db path must be in sync directory")?
                    }
                }

                let sync_spec = sync::SyncSpec {
                    path: sync_dir,
                    path_db: sync_db,
                    buckets: effective_buckets,
                    start: start_date,
                };

                sync::sync_run(&client, &sync_spec, mode)?
            } else {
                // Simple host-based sync mode (backwards compatibility)
                // Pull
                match host {
                    Some(hosts) => {
                        for host in hosts.iter() {
                            info!("Pulling from host: {}", host);
                            sync_wrapper::pull(host, &client)?;
                        }
                    }
                    None => {
                        info!("Pulling from all hosts");
                        sync_wrapper::pull_all(&client)?;
                    }
                }

                // Push
                info!("Pushing local data");
                sync_wrapper::push(&client)?
            }
        }

        // List all buckets
        Commands::List {} => sync::list_buckets(&client)?,
    }

    // Needed to give the datastores some time to commit before program is shut down.
    // 100ms isn't actually needed, seemed to work fine with as little as 10ms, but I'd rather give
    // it some wiggleroom.
    std::thread::sleep(std::time::Duration::from_millis(100));

    Ok(())
}

fn daemon(
    client: &AwClient,
    start_date: Option<DateTime<Utc>>,
    buckets: Option<Vec<String>>,
    sync_db: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let (tx, rx) = channel();

    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })?;

    let sync_dir = dirs::get_sync_dir()?;
    if let Some(db_path) = &sync_db {
        info!("Using sync db: {}", &db_path.display());

        if !db_path.is_absolute() {
            Err("Sync db path must be absolute")?
        }
        if !db_path.starts_with(&sync_dir) {
            Err("Sync db path must be in sync directory")?
        }
    }

    let sync_spec = sync::SyncSpec {
        path: sync_dir,
        buckets,
        path_db: sync_db,
        start: start_date,
    };

    loop {
        if let Err(e) = sync::sync_run(client, &sync_spec, sync::SyncMode::Both) {
            error!("Error during sync cycle: {}", e);
            return Err(e);
        }

        info!("Sync pass done, sleeping for 5 minutes");

        match rx.recv_timeout(Duration::from_secs(300)) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => {
                info!("Termination signal received, shutting down.");
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                // Continue the loop if the timeout occurs
            }
        }
    }

    Ok(())
}
