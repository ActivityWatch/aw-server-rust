#![feature(plugin,try_from,custom_derive)]
#![plugin(rocket_codegen)]
extern crate rocket;
#[macro_use] extern crate rocket_contrib;

extern crate serde;
extern crate serde_json;
#[macro_use] extern crate serde_derive;

extern crate rusqlite;

extern crate mpsc_requests;

extern crate chrono;

pub mod models;
pub mod transform;
pub mod datastore;
pub mod endpoints;

fn main() {
    let server_state = endpoints::ServerState {
        datastore: datastore::Datastore::new("/tmp/test.db".to_string())
    };

    endpoints::rocket(server_state).launch();
}
