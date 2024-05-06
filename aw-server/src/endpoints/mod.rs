use rust_embed::RustEmbed;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gethostname::gethostname;
use rocket::fs::FileServer;
use rocket::http::ContentType;
use rocket::serde::json::Json;
use rocket::State;

use crate::config::AWConfig;

use aw_datastore::Datastore;
use aw_models::Info;

#[derive(RustEmbed)]
#[folder = "$AW_WEBUI_DIR"]
struct EmbeddedAssets;

pub struct AssetResolver {
    asset_path: Option<PathBuf>,
}

impl AssetResolver {
    pub fn new(asset_path: Option<PathBuf>) -> Self {
        Self { asset_path }
    }

    fn resolve(&self, file_path: &str) -> Option<Vec<u8>> {
        if let Some(asset_path) = &self.asset_path {
            let content = std::fs::read(asset_path.join(file_path));
            if let Ok(data) = content {
                return Some(data);
            }
        }
        Some(EmbeddedAssets::get(file_path)?.data.to_vec())
    }
}

pub struct ServerState {
    pub datastore: Mutex<Datastore>,
    pub asset_resolver: AssetResolver,
    pub device_id: String,
}

#[macro_use]
mod util;
mod bucket;
mod cors;
mod export;
mod hostcheck;
mod import;
mod query;
mod settings;

pub use util::HttpErrorJson;

#[get("/")]
fn root_index(state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file("index.html".into(), state)
}

#[get("/css/<file..>")]
fn root_css(file: PathBuf, state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file(Path::new("css").join(file), state)
}

#[get("/fonts/<file..>")]
fn root_fonts(file: PathBuf, state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file(Path::new("fonts").join(file), state)
}

#[get("/js/<file..>")]
fn root_js(file: PathBuf, state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file(Path::new("js").join(file), state)
}

#[get("/static/<file..>")]
fn root_static(file: PathBuf, state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file(Path::new("static").join(file), state)
}

#[get("/favicon.ico")]
fn root_favicon(state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file("favicon.ico".into(), state)
}

#[get("/dark.css")]
fn root_dark(state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file("dark.css".into(), state)
}

#[get("/logo.png")]
fn root_logo(state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file("logo.png".into(), state)
}

#[get("/manifest.json")]
fn root_manifest(state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    get_file("manifest.json".into(), state)
}

#[get("/")]
fn server_info(config: &State<AWConfig>, state: &State<ServerState>) -> Json<Info> {
    #[allow(clippy::or_fun_call)]
    let hostname = gethostname().into_string().unwrap_or("unknown".to_string());
    const VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

    Json(Info {
        hostname,
        version: format!("v{} (rust)", VERSION.unwrap_or("(unknown)")),
        testing: config.testing,
        device_id: state.device_id.clone(),
    })
}

fn get_file(file: PathBuf, state: &State<ServerState>) -> Option<(ContentType, Vec<u8>)> {
    let asset = state.asset_resolver.resolve(&file.display().to_string())?;

    let content_type = file
        .extension()
        .and_then(OsStr::to_str)
        .and_then(ContentType::from_extension)
        .unwrap_or(ContentType::Bytes);

    Some((content_type, asset))
}

pub fn build_rocket(server_state: ServerState, config: AWConfig) -> rocket::Rocket<rocket::Build> {
    info!(
        "Starting aw-server-rust at {}:{}",
        config.address, config.port
    );
    let cors = cors::cors(&config);
    let hostcheck = hostcheck::HostCheck::new(&config);
    let custom_static = config.custom_static.clone();

    let mut rocket = rocket::custom(config.to_rocket_config())
        .attach(cors.clone())
        .attach(hostcheck)
        .manage(cors)
        .manage(server_state)
        .manage(config)
        .mount(
            "/",
            routes![
                root_index,
                root_favicon,
                root_fonts,
                root_css,
                root_js,
                root_static,
                // custom static files
                root_dark,
                root_logo,
                root_manifest
            ],
        )
        .mount("/api/0/info", routes![server_info])
        .mount(
            "/api/0/buckets",
            routes![
                bucket::bucket_new,
                bucket::bucket_delete,
                bucket::buckets_get,
                bucket::bucket_get,
                bucket::bucket_events_get,
                bucket::bucket_events_create,
                bucket::bucket_events_heartbeat,
                bucket::bucket_event_count,
                bucket::bucket_events_get_single,
                bucket::bucket_events_delete_by_id,
                bucket::bucket_export
            ],
        )
        .mount("/api/0/query", routes![query::query])
        .mount(
            "/api/0/import",
            routes![import::bucket_import_json, import::bucket_import_form],
        )
        .mount("/api/0/export", routes![export::buckets_export])
        .mount(
            "/api/0/settings",
            routes![
                settings::setting_get,
                settings::setting_set,
                settings::setting_delete,
                settings::settings_get,
            ],
        )
        .mount("/", rocket_cors::catch_all_options_routes());

    // for each custom static directory, mount it at the given name
    for (name, dir) in custom_static {
        info!(
            "Serving /pages/{} custom static directory from {}",
            name, dir
        );
        rocket = rocket.mount(&format!("/pages/{name}"), FileServer::from(dir));
    }
    rocket
}

mod tests {
    #[test]
    fn test_filesystem_resolver() {
        let resolver = super::AssetResolver::new(Some(".".into()));

        let content = resolver.resolve("Cargo.toml").unwrap();

        assert!(String::from_utf8(content).unwrap().contains("aw-server"));
    }

    #[test]
    fn test_resolver_without_asset() {
        let resolver = super::AssetResolver::new(Some(".".into()));

        let content = resolver.resolve("Cargo.json");

        assert!(content.is_none());
    }
}
