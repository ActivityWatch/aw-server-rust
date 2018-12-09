#![feature(plugin,try_from,custom_derive)]
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

pub mod models;
pub mod transform;
pub mod datastore;
pub mod query;
pub mod endpoints;

fn main() {
    let server_state = endpoints::ServerState {
        datastore: datastore::Datastore::new("/tmp/test.db".to_string())
    };

    endpoints::rocket(server_state).launch();
}
