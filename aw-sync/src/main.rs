#[macro_use]
extern crate log;
extern crate chrono;
extern crate serde;
extern crate serde_json;

mod sync;

fn main() {
    // What needs to be done:
    //  - [x] Setup local sync bucket
    //  - Import local buckets and sync events from aw-server (either through API or through creating a read-only Datastore)
    //  - Import buckets and sync events from remotes

    println!("Started aw-sync-rust...");
    aw_server::logging::setup_logger().expect("Failed to setup logging");

    sync::sync_run();
    info!("Finished successfully, exiting...");

    // Needed to give the datastores some time to commit before program is shut down.
    // 100ms isn't actually needed, seemed to work fine with as little as 10ms, but I'd rather give
    // it some wiggleroom.
    std::thread::sleep(std::time::Duration::from_millis(100));
}
