#[macro_use] extern crate log;

use activitywatch::*;

fn main() {
    println!("Started aw-sync-rust...");
    datastore::Datastore::new("/tmp/test.db".to_string());
    info!("Not implemented yet, exiting");
}
