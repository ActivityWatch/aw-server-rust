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

use std::path::Path;

use chrono::{DateTime, Utc};
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

    /// Host of instance to connect to
    #[clap(long, default_value = "127.0.0.1")]
    host: String,
    /// Port of instance to connect to
    #[clap(long, default_value = DEFAULT_PORT)]
    port: String,
    /// Convenience option for using the default testing host and port.
    #[clap(long)]
    testing: bool,
    /// Path to sync directory
    /// If not specified, exit
    #[clap(long)]
    sync_dir: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Clones repos
    #[clap(arg_required_else_help = true)]
    Sync {
        /// Date to start syncing from
        /// If not specified, start from beginning
        /// NOTE: might be unstable, as count cannot be used to verify integrity of sync.
        /// Format: YYYY-MM-DD
        #[clap(long)]
        start_date: Option<String>,
        /// Specify buckets to sync
        /// If not specified, all buckets will be synced
        #[clap(long)]
        buckets: Vec<String>,
    },
    /// List buckets and their sync status
    List {},
}

fn main() -> std::io::Result<()> {
    let opts: Opts = Opts::parse();

    println!("Started aw-sync-rust...");

    aw_server::logging::setup_logger(true).expect("Failed to setup logging");

    let sync_directory = if opts.sync_dir.is_empty() {
        println!("No sync directory specified, exiting...");
        std::process::exit(1);
    } else {
        Path::new(&opts.sync_dir)
    };

    let port = if opts.testing && opts.port == DEFAULT_PORT {
        "5666"
    } else {
        &opts.port
    };

    let client = AwClient::new(opts.host.as_str(), port, "aw-sync-rust");

    match &opts.command {
        // Perform two-way sync
        Commands::Sync {
            start_date,
            buckets,
        } => {
            let start: Option<DateTime<Utc>> = start_date.as_ref().map(|date| {
                let date_copy = date.clone();
                chrono::DateTime::parse_from_rfc3339(&date_copy)
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            });

            sync::sync_run(sync_directory, client, buckets, start).map_err(|e| {
                println!("Error: {}", e);
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })?;
            info!("Finished successfully, exiting...");
            Ok(())
        }

        // List all buckets
        Commands::List {} => {
            sync::list_buckets(&client, sync_directory);
            Ok(())
        }
    }
    .map_err(|e: String| {
        println!("Error: {}", e);
        std::io::Error::new(std::io::ErrorKind::Other, e)
    })?;

    // Needed to give the datastores some time to commit before program is shut down.
    // 100ms isn't actually needed, seemed to work fine with as little as 10ms, but I'd rather give
    // it some wiggleroom.
    std::thread::sleep(std::time::Duration::from_millis(100));

    Ok(())
}
