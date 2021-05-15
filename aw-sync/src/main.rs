#[macro_use]
extern crate log;
extern crate chrono;
extern crate serde;
extern crate serde_json;

use std::path::Path;

use aw_client_rust::AwClient;

mod sync;

fn main() {
    // What needs to be done:
    //  - [x] Setup local sync bucket
    //  - [x] Import local buckets and sync events from aw-server (either through API or through creating a read-only Datastore)
    //  - [x] Import buckets and sync events from remotes
    //  - [ ] Add CLI arguments
    //     - [ ] For which local server to use
    //     - [ ] For which sync dir to use

    println!("Started aw-sync-rust...");
    aw_server::logging::setup_logger(true).expect("Failed to setup logging");

    // TODO: Get path using dirs module
    let sync_directory = Path::new("sync-testing");

    let client = AwClient::new("127.0.0.1", "5667", "aw-sync-rust");

    sync::sync_run(sync_directory, client);
    info!("Finished successfully, exiting...");

    // Needed to give the datastores some time to commit before program is shut down.
    // 100ms isn't actually needed, seemed to work fine with as little as 10ms, but I'd rather give
    // it some wiggleroom.
    std::thread::sleep(std::time::Duration::from_millis(100));
}
