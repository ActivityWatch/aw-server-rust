#![feature(plugin,try_from)]
#![feature(proc_macro_hygiene)]
#![feature(custom_attribute)]
#![feature(decl_macro)]
#[macro_use] extern crate rocket;
#[macro_use] extern crate rocket_contrib;
extern crate rocket_cors;

extern crate serde;
extern crate serde_json;
#[macro_use] extern crate serde_derive;

extern crate rusqlite;

extern crate mpsc_requests;

extern crate chrono;

extern crate plex;

extern crate appdirs;

#[macro_use] extern crate lazy_static;

pub mod models;
pub mod transform;
pub mod datastore;
pub mod query;
pub mod endpoints;
pub mod dirs;

fn main() {
    let db_path = dirs::db_path().to_str().unwrap().to_string();
    println!("Using DB at path {:?}", db_path);

    let server_state = endpoints::ServerState {
        datastore: datastore::Datastore::new(db_path)
    };

    endpoints::rocket(server_state).launch();
}
