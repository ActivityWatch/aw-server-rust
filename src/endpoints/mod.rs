use std::path::{Path,PathBuf};

use rocket;
use rocket::response::{NamedFile};
use rocket_contrib::{Json, Value};

use std::sync::Mutex;
use rusqlite::Connection;

use datastore;

pub struct ServerState {
    pub dbconnection: Connection
}
pub type ServerStateMutex = Mutex<ServerState>;

pub mod bucket;

#[get("/")]
fn root_index() -> Option<NamedFile> {
    NamedFile::open(Path::new("frontend/dist/index.html")).ok()
}
#[get("/0.css")]
fn root_css() -> Option<NamedFile> {
    NamedFile::open(Path::new("frontend/dist/0.css")).ok()
}
#[get("/0.css.map")]
fn root_css_map() -> Option<NamedFile> {
    NamedFile::open(Path::new("frontend/dist/0.css.map")).ok()
}
#[get("/static/<file..>")]
fn root_static(file: PathBuf) -> Option<NamedFile> {
    NamedFile::open(Path::new("frontend/dist/static/").join(file)).ok()
}

#[get("/favicon.ico")]
fn root_favicon() -> Option<NamedFile> {
    NamedFile::open(Path::new("frontend/dist/favicon.ico")).ok()
}

#[get("/")]
fn server_info() -> Json<Value> {
    Json(json!({
        "hostname": "johan-desktop",
        "version": "aw-server-rust_v0.1",
        "testing": false
    }))
}


#[catch(404)]
fn not_found() -> Json<Value> {
    /* TODO: Set to HTML page */
    Json(json!({
        "status": "error",
        "reason": "Resource was not found."
    }))
}

pub fn rocket() -> rocket::Rocket {
    let server_state = ServerState {
        dbconnection: datastore::setup("/tmp/test.db".to_string())
    };

    rocket::ignite()
        .mount("", routes![
               root_index, root_favicon, root_static, root_css, root_css_map,
        ])
        .mount("/api/0/info", routes![server_info])
        .mount("/api/0/buckets", routes![
               bucket::bucket_new, bucket::bucket_delete, bucket::buckets_get, bucket::bucket_get,
               bucket::bucket_events_get, bucket::bucket_events_create, bucket::bucket_events_heartbeat, bucket::bucket_events_count
        ])
        .catch(catchers![not_found])
        .manage(ServerStateMutex::new(server_state))
}
