#[macro_use] extern crate log;

use aw_server::*;

use std::fs;
use std::path::Path;

fn main() -> std::io::Result<()> {
    println!("Started aw-sync-rust...");
    logging::setup_logger().expect("Failed to setup logging");
    setup()?;
    warn!("Not implemented yet, exiting");
    Ok(())
}

fn setup() -> std::io::Result<()> {
    // TODO: Get path using dirs module
    let p = Path::new("/tmp/aw-sync-rust/testing");
    fs::create_dir_all(p)?;
    info!("Created syncing directory");
    datastore::Datastore::new(p.join("test.db").to_str().unwrap().to_string());
    Ok(())
}
