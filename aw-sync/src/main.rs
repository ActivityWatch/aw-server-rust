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
use std::path::Path;
use std::path::PathBuf;

use chrono::{DateTime, Datelike, TimeZone, Utc};
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
    command: Commands,

    /// Host of instance to connect to.
    #[clap(long, default_value = "127.0.0.1")]
    host: String,

    /// Port of instance to connect to.
    #[clap(long)]
    port: Option<String>,

    /// Convenience option for using the default testing host and port.
    #[clap(long)]
    testing: bool,

    /// Enable debug logging.
    #[clap(long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync subcommand (basic)
    ///
    /// Pulls remote buckets then pushes local buckets.
    Sync {
        /// Host(s) to pull from, comma separated. Will pull from all hosts if not specified.
        #[clap(long)]
        host: Option<String>,
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
        #[clap(long)]
        start_date: Option<String>,

        /// Specify buckets to sync using a comma-separated list.
        /// If not specified, all buckets will be synced.
        #[clap(long)]
        buckets: Option<String>,

        /// Mode to sync in. Can be "push", "pull", or "both".
        /// Defaults to "both".
        #[clap(long, default_value = "both")]
        mode: String,

        /// Full path to sync directory.
        /// If not specified, exit.
        #[clap(long)]
        sync_dir: String,

        /// Full path to sync db file
        /// Useful for syncing buckets from a specific db file in the sync directory.
        /// Must be a valid absolute path to a file in the sync directory.
        #[clap(long)]
        sync_db: Option<String>,
    },
    /// List buckets and their sync status.
    List {},
}

fn main() -> Result<(), Box<dyn Error>> {
    let opts: Opts = Opts::parse();
    let verbose = opts.verbose;

    info!("Started aw-sync...");

    aw_server::logging::setup_logger("aw-sync", opts.testing, verbose)
        .expect("Failed to setup logging");

    let port = opts
        .port
        .or_else(|| Some(crate::util::get_server_port(opts.testing).ok()?.to_string()))
        .unwrap();

    let client = AwClient::new(opts.host.as_str(), port.as_str(), "aw-sync");

    match &opts.command {
        // Perform basic sync
        Commands::Sync { host } => {
            // Pull
            match host {
                Some(host) => {
                    let hosts: Vec<&str> = host.split(',').collect();
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
            sync_wrapper::push(&client)?;
            Ok(())
        }
        // Perform two-way sync
        Commands::SyncAdvanced {
            start_date,
            buckets,
            mode,
            sync_dir,
            sync_db,
        } => {
            let sync_directory = if sync_dir.is_empty() {
                error!("No sync directory specified, exiting...");
                std::process::exit(1);
            } else {
                Path::new(&sync_dir)
            };
            info!("Using sync dir: {}", sync_directory.display());

            if let Some(sync_db) = &sync_db {
                info!("Using sync db: {}", sync_db);
            }

            let start: Option<DateTime<Utc>> = start_date.as_ref().map(|date| {
                println!("{}", date.clone());
                chrono::NaiveDate::parse_from_str(&date.clone(), "%Y-%m-%d")
                    .map(|nd| {
                        Utc.with_ymd_and_hms(nd.year(), nd.month(), nd.day(), 0, 0, 0)
                            .single()
                            .unwrap()
                    })
                    .expect("Date was not on the format YYYY-MM-DD")
            });

            // Parse comma-separated list
            let buckets_vec: Option<Vec<String>> = buckets
                .as_ref()
                .map(|b| b.split(',').map(|s| s.to_string()).collect());

            let sync_db: Option<PathBuf> = sync_db.as_ref().map(|db| {
                let db_path = Path::new(db);
                if !db_path.is_absolute() {
                    panic!("Sync db path must be absolute");
                }
                if !db_path.starts_with(sync_directory) {
                    panic!("Sync db path must be in sync directory");
                }
                db_path.to_path_buf()
            });

            let sync_spec = sync::SyncSpec {
                path: sync_directory.to_path_buf(),
                path_db: sync_db,
                buckets: buckets_vec,
                start,
            };

            let mode_enum = match mode.as_str() {
                "push" => sync::SyncMode::Push,
                "pull" => sync::SyncMode::Pull,
                "both" => sync::SyncMode::Both,
                _ => panic!("Invalid mode"),
            };

            sync::sync_run(&client, &sync_spec, mode_enum)
        }

        // List all buckets
        Commands::List {} => sync::list_buckets(&client),
    }?;

    // Needed to give the datastores some time to commit before program is shut down.
    // 100ms isn't actually needed, seemed to work fine with as little as 10ms, but I'd rather give
    // it some wiggleroom.
    std::thread::sleep(std::time::Duration::from_millis(100));

    Ok(())
}
