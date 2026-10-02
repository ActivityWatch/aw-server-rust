use rust_embed::RustEmbed;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use gethostname::gethostname;
use rocket::fs::FileServer;
use rocket::http::{ContentType, Status};
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

    /// The web UI entry point, or an explanatory page (503) when this build
    /// has no web UI assets, instead of Rocket's bare 404.
    fn index_or_placeholder(&self) -> (Status, ContentType, Vec<u8>) {
        match self.resolve("index.html") {
            Some(data) => (Status::Ok, ContentType::HTML, data),
            None => {
                let checked = self
                    .asset_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(none; --webpath not set)".to_string());
                let html = missing_index_html(&checked);
                (
                    Status::ServiceUnavailable,
                    ContentType::HTML,
                    html.into_bytes(),
                )
            }
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn missing_index_html(checked_path: &str) -> String {
    let escaped = html_escape(checked_path);
    format!(
        "<!DOCTYPE html><html><head><title>ActivityWatch: web UI missing</title></head>\
<body><h1>The web UI is not installed</h1>\
<p>aw-server is running, but this build has no <code>index.html</code>, \
neither embedded nor in the <code>--webpath</code> directory that was checked: \
<code>{escaped}</code>.</p>\
<p>The API still works: <a href=\"/api/0/info\">/api/0/info</a>. \
Build with <code>AW_WEBUI_DIR</code> pointing at a built aw-webui, or pass \
<code>--webpath</code>. See the <a href=\"https://docs.activitywatch.net/\">docs</a>.</p>\
</body></html>"
    )
}

// The Datastore is just a cheap handle to the DB worker thread (a crossbeam
// channel sender), which serializes all DB access internally. No mutex is
// needed here — wrapping it in one would serialize all HTTP requests, letting
// a slow query block every heartbeat.
pub struct ServerState {
    pub datastore: Datastore,
    pub asset_resolver: AssetResolver,
    pub device_id: String,
    /// Cache of query results for finished past periods (see `query_cache`).
    pub query_cache: std::sync::Arc<query_cache::QueryCache>,
    /// Set to false via config (`query_cache = false`) to bypass the cache.
    pub query_cache_enabled: bool,
    /// Serializes the read-modify-invalidate sequence in the event write
    /// handlers. Without it, two concurrent replacements of the same event
    /// could each invalidate only their own view of the old range and leave an
    /// intermediate period cached (see `bucket_events_create`).
    pub write_lock: std::sync::Mutex<()>,
}

impl ServerState {
    /// Build a state with the default (enabled) query cache.
    pub fn new(datastore: Datastore, asset_resolver: AssetResolver, device_id: String) -> Self {
        Self {
            datastore,
            asset_resolver,
            device_id,
            query_cache: std::sync::Arc::new(query_cache::QueryCache::new()),
            query_cache_enabled: true,
            write_lock: std::sync::Mutex::new(()),
        }
    }
}

#[macro_use]
mod util;
mod apikey;
mod bucket;
mod cors;
mod export;
mod extension_cors;
mod hostcheck;
mod import;
mod query;
pub mod query_cache;
mod settings;

#[cfg(target_os = "android")]
pub(crate) use settings::settings_datastore_key;
pub use util::HttpErrorJson;

#[get("/")]
fn root_index(state: &State<ServerState>) -> (Status, (ContentType, Vec<u8>)) {
    let (status, content_type, body) = state.asset_resolver.index_or_placeholder();
    (status, (content_type, body))
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

    Json(Info {
        hostname,
        version: crate::version::version_string(),
        testing: config.testing,
        profile: crate::config::get_profile().to_string(),
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
    let extension_cors = extension_cors::ExtensionCorsScope::new(&config);
    let hostcheck = hostcheck::HostCheck::new(&config);
    let apikey = apikey::ApiKeyCheck::new(&config);
    let custom_static = config.custom_static.clone();

    let mut rocket = rocket::custom(config.to_rocket_config())
        .attach(cors.clone())
        // Attached before the other request fairings so a blocked extension
        // request is rewritten to the 403 route before they inspect the path.
        .attach(extension_cors)
        .attach(hostcheck)
        .attach(apikey)
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
                bucket::bucket_events_get_csv,
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
    fn test_missing_index_html_content() {
        // Test the placeholder HTML generation directly — independent of whether
        // this build has embedded assets.
        let html = super::missing_index_html("/nonexistent-webpath");
        assert!(html.contains("/nonexistent-webpath"));
        assert!(html.contains("/api/0/info"));
    }

    #[test]
    fn test_missing_index_html_escapes_path() {
        let html = super::missing_index_html("/path/<evil>&\"chars\"");
        assert!(!html.contains("<evil>"));
        assert!(html.contains("&lt;evil&gt;"));
        assert!(html.contains("&amp;"));
        assert!(html.contains("&quot;"));
    }

    #[test]
    fn test_resolver_without_asset() {
        let resolver = super::AssetResolver::new(Some(".".into()));

        let content = resolver.resolve("Cargo.json");

        assert!(content.is_none());
    }
}
