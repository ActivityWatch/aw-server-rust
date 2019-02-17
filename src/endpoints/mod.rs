use std::path::{Path,PathBuf};

use rocket;
use rocket::response::{NamedFile};
use rocket_contrib::json::JsonValue;

mod bucket;
mod query;
mod import;
mod cors;

use datastore::Datastore;

pub struct ServerState {
    pub datastore: Datastore
}

#[get("/")]
fn root_index() -> Option<NamedFile> {
    NamedFile::open(Path::new("aw-webui/dist/index.html")).ok()
}
#[get("/0.css")]
fn root_css() -> Option<NamedFile> {
    NamedFile::open(Path::new("aw-webui/dist/0.css")).ok()
}
#[get("/0.css.map")]
fn root_css_map() -> Option<NamedFile> {
    NamedFile::open(Path::new("aw-webui/dist/0.css.map")).ok()
}
#[get("/static/<file..>")]
fn root_static(file: PathBuf) -> Option<NamedFile> {
    NamedFile::open(Path::new("aw-webui/dist/static/").join(file)).ok()
}

#[get("/favicon.ico")]
fn root_favicon() -> Option<NamedFile> {
    NamedFile::open(Path::new("aw-webui/dist/favicon.ico")).ok()
}

#[get("/")]
fn server_info() -> JsonValue {
    json!({
        "hostname": "johan-desktop",
        "version": "aw-server-rust_v0.1",
        "testing": false
    })
}

#[catch(304)]
fn not_modified() -> JsonValue {
    json!({
        "status": 304,
        "reason": "Not modified."
    })
}

#[catch(404)]
fn not_found() -> JsonValue {
    json!({
        "status": 404,
        "reason": "Resource was not found."
    })
}

pub fn rocket(server_state: ServerState) -> rocket::Rocket {
    // TODO: add info!("Starting aw-server at 127.0.0.1:port")
    rocket::ignite()
        .mount("/", routes![
               root_index, root_favicon, root_static, root_css, root_css_map,
        ])
        .mount("/api/0/info", routes![server_info])
        .mount("/api/0/buckets", routes![
               bucket::bucket_new, bucket::bucket_delete, bucket::buckets_get, bucket::bucket_get,
               bucket::bucket_events_get, bucket::bucket_events_create, bucket::bucket_events_heartbeat, bucket::bucket_event_count
        ])
        .mount("/api/0/query", routes![
               query::query
        ])
        .mount("/api/0/import/", routes![
               import::bucket_import
        ])
        .attach(cors::cors())
        .register(catchers![not_modified, not_found])
        .manage(server_state)
}
