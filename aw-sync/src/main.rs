// What needs to be done:
//  - [x] Setup local sync bucket
//  - [x] Import local buckets and sync events from aw-server (either through API or through creating a read-only Datastore)
//  - [x] Import buckets and sync events from remotes
//  - [ ] Add CLI arguments
//     - [x] For which local server to use
//     - [x] For which sync dir to use
//     - [ ] Date to start syncing from

#[macro_use]
extern crate log;
extern crate chrono;
extern crate serde;
extern crate serde_json;

use std::error::Error;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};

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
    Daemon {},

    /// Sync subcommand (basic)
    ///
    /// Pulls remote buckets then pushes local buckets.
    Sync {
        /// Host(s) to pull from, comma separated. Will pull from all hosts if not specified.
        #[clap(long, value_parser=parse_list)]
        host: Option<Vec<String>>,
    },

    /// Sync subcommand (advanced)
    ///
    /// Pulls remote buckets then pushes local buckets.
    /// First pulls remote buckets in the sync directory to the local aw-server.
    /// Then pushes local buckets from the aw-server to the local sync directory.
    #[clap(arg_required_else_help = true)]
    SyncAdvanced {
        /// Date to start syncing from.
        /// If not specified, start from beginning.
        /// NOTE: might be unstable, as count cannot be used to verify integrity of sync.
        /// Format: YYYY-MM-DD
        #[clap(long, value_parser=parse_start_date)]
        start_date: Option<DateTime<Utc>>,

        /// Specify buckets to sync using a comma-separated list.
        /// If not specified, all buckets will be synced.
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
    Ok(arg.split(',').map(|s| s.to_string()).collect())
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

    // if opts.command is None, then we're using the default subcommand (Sync)
    match opts.command.unwrap_or(Commands::Daemon {}) {
        // Start daemon
        Commands::Daemon {} => {
            info!("Starting daemon...");
            daemon(&client)?;
        }
        // Perform basic sync
        Commands::Sync { host } => {
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
        // Perform two-way sync
        Commands::SyncAdvanced {
            start_date,
            buckets,
            mode,
            sync_db,
        } => {
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
                buckets,
                start: start_date,
            };

            sync::sync_run(&client, &sync_spec, mode)?
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

fn daemon(client: &AwClient) -> Result<(), Box<dyn Error>> {
    loop {
        if let Err(e) = daemon_sync_cycle(client) {
            error!("Error during sync cycle: {}", e);
            // Re-throw the error
            return Err(e);
        }

        info!("Sync pass done, sleeping for 5 minutes");
        std::thread::sleep(std::time::Duration::from_secs(300));
    }
}

fn daemon_sync_cycle(client: &AwClient) -> Result<(), Box<dyn Error>> {
    info!("Pulling from all hosts");
    sync_wrapper::pull_all(client)?;

    info!("Pushing local data");
    sync_wrapper::push(client)?;

    Ok(())
}
