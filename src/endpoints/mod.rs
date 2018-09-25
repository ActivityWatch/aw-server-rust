use std::path::{Path,PathBuf};

use rocket;
use rocket::response::{NamedFile};
use rocket_contrib::{Json, Value};

use std::sync::Mutex;
use rusqlite::Connection;

pub struct ServerState {
    pub dbconnection: Connection
}
pub type ServerStateMutex = Mutex<ServerState>;

pub mod user;

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
        dbconnection: super::datastore::setup("/tmp/test.db".to_string())
    };

    rocket::ignite()
        .mount("", routes![
               root_index, root_favicon, root_static, root_css, root_css_map,
        ])
        .mount("/api/0/buckets", routes![
               user::buckets_get, user::bucket_get, user::bucket_event_count, user::bucket_new, user::bucket_delete
        ])
        .catch(catchers![not_found])
        .manage(ServerStateMutex::new(server_state))
}
