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

use chrono::{DateTime, Datelike, TimeZone, Utc};
use clap::{Parser, Subcommand};

use aw_client_rust::AwClient;

mod accessmethod;
mod sync;

const DEFAULT_PORT: &str = "5600";

#[derive(Parser)]
#[clap(version = "0.1", author = "Erik Bjäreholt")]
struct Opts {
    #[clap(subcommand)]
    command: Commands,

    /// Host of instance to connect to.
    #[clap(long, default_value = "127.0.0.1")]
    host: String,
    /// Port of instance to connect to.
    #[clap(long, default_value = DEFAULT_PORT)]
    port: String,
    /// Convenience option for using the default testing host and port.
    #[clap(long)]
    testing: bool,
    /// Path to sync directory.
    /// If not specified, exit.
    #[clap(long)]
    sync_dir: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync subcommand.
    ///
    /// Pulls remote buckets then pushes local buckets.
    /// First pulls remote buckets in the sync directory to the local aw-server.
    /// Then pushes local buckets from the aw-server to the local sync directory.
    #[clap(arg_required_else_help = true)]
    Sync {
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
    },
    /// List buckets and their sync status.
    List {},
}

fn main() -> Result<(), Box<dyn Error>> {
    let opts: Opts = Opts::parse();

    info!("Started aw-sync...");

    aw_server::logging::setup_logger(true).expect("Failed to setup logging");

    let sync_directory = if opts.sync_dir.is_empty() {
        println!("No sync directory specified, exiting...");
        std::process::exit(1);
    } else {
        Path::new(&opts.sync_dir)
    };
    info!("Using sync dir: {}", sync_directory.display());

    let port = if opts.testing && opts.port == DEFAULT_PORT {
        "5666"
    } else {
        &opts.port
    };

    let client = AwClient::new(opts.host.as_str(), port, "aw-sync");

    match &opts.command {
        // Perform two-way sync
        Commands::Sync {
            start_date,
            buckets,
            mode,
        } => {
            let start: Option<DateTime<Utc>> = start_date.as_ref().map(|date| {
                println!("{}", date.clone());
                chrono::NaiveDate::parse_from_str(&date.clone(), "%Y-%m-%d")
                    .map(|nd| {
                        let dt = Utc.ymd(nd.year(), nd.month(), nd.day());
                        dt.and_hms(0, 0, 0)
                    })
                    .expect("Date was not on the format YYYY-MM-DD")
            });

            // Parse comma-separated list
            let buckets_vec: Option<Vec<String>> = buckets
                .as_ref()
                .map(|b| b.split(',').map(|s| s.to_string()).collect());

            let sync_spec = sync::SyncSpec {
                path: sync_directory.to_path_buf(),
                buckets: buckets_vec,
                start,
            };

            let mode_enum = match mode.as_str() {
                "push" => sync::SyncMode::Push,
                "pull" => sync::SyncMode::Pull,
                "both" => sync::SyncMode::Both,
                _ => panic!("Invalid mode"),
            };

            sync::sync_run(client, &sync_spec, mode_enum)
        }

        // List all buckets
        Commands::List {} => sync::list_buckets(&client, sync_directory),
    }?;

    // Needed to give the datastores some time to commit before program is shut down.
    // 100ms isn't actually needed, seemed to work fine with as little as 10ms, but I'd rather give
    // it some wiggleroom.
    std::thread::sleep(std::time::Duration::from_millis(100));

    Ok(())
}
