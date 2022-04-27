#[macro_use]
extern crate log;

use std::env;

use clap::Parser;

use aw_server::*;

#[cfg(all(target_os = "linux", target_arch = "x86"))]
extern crate jemallocator;
#[cfg(all(target_os = "linux", target_arch = "x86"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

/// Rust server for ActivityWatch
#[derive(Parser)]
#[clap(version = "0.10", author = "Johan Bjäreholt, Erik Bjäreholt")]
struct Opts {
    /// Run in testing mode
    #[clap(long)]
    testing: bool,
    /// Address to listen to
    #[clap(long)]
    host: Option<String>,
    /// Port to listen on
    #[clap(long)]
    port: Option<String>,
    /// Path to database override
    #[clap(long)]
    dbpath: Option<String>,
    /// Path to webui override
    #[clap(long)]
    webpath: Option<String>,
    /// Device ID override
    #[clap(long)]
    device_id: Option<String>,
    /// Don't import from aw-server-python if no aw-server-rust db found
    #[clap(long)]
    no_legacy_import: bool,
}

#[rocket::main]
async fn main() -> Result<(), rocket::Error> {
    let opts: Opts = Opts::parse();

    use std::sync::Mutex;

    let mut testing = opts.testing;
    // Always override environment if --testing is specified
    if !testing {
        if cfg!(debug_assertions) {
            testing = true;
        }
    }

    logging::setup_logger(testing).expect("Failed to setup logging");

    let mut config = config::create_config(testing);

    // set host if overridden
    if let Some(host) = opts.host {
        config.address = host;
    }

    // set port if overridden
    if let Some(port) = opts.port {
        config.port = port.parse().unwrap();
    }

    // Set db path if overridden
    let db_path: String = if let Some(dbpath) = opts.dbpath {
        dbpath
    } else {
        dirs::db_path(testing)
            .expect("Failed to get db path")
            .to_str()
            .unwrap()
            .to_string()
    };
    info!("Using DB at path {:?}", db_path);

    let asset_path = match opts.webpath {
        Some(webpath) => PathBuf::from(webpath),
        None => get_asset_path(),
    };
    info!("Using aw-webui assets at path {:?}", asset_path);

    let legacy_import = !opts.no_legacy_import;

    let device_id: String = if let Some(id) = opts.device_id {
        id
    } else {
        device_id::get_device_id()
    };

    let server_state = endpoints::ServerState {
        // Even if legacy_import is set to true it is disabled on Android so
        // it will not happen there
        datastore: Mutex::new(aw_datastore::Datastore::new(db_path, legacy_import)),
        asset_path,
        device_id,
    };

    endpoints::build_rocket(server_state, config).launch().await
}

use std::ffi::OsString;
use std::path::PathBuf;

// The appdirs implementation of site_data_dir is broken on computers which has flatpak installed
// as flatpak adds its ~/.local/share/flatpak/exports/share directory first and then doesn't care
// about the rest of the paths.
// This is a rewrite of site_data_dir which takes the first folder which exists out of all folders
// in XDG_DATA_DIRS
// TODO: Should we talk to upstream about this? This changes the behavior quite a lot so maybe they
// don't want this change?
fn site_data_dir(app: Option<&str>, _: Option<&str>) -> Result<PathBuf, ()> {
    // Iterate over all XDG_DATA_DIRS and return first match that exists
    let joined = match env::var_os("XDG_DATA_DIRS") {
        // If $XDG_DATA_DIRS is either not set or empty, a value equal to /usr/local/share/:/usr/share/ should be used.
        // https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html
        Some(path) => {
            if path.is_empty() {
                OsString::from("/usr/local/share:/usr/share")
            } else {
                path
            }
        }
        None => OsString::from("/usr/local/share:/usr/share"),
    };

    for mut data_dir in env::split_paths(&joined) {
        if let Some(app) = app {
            data_dir.push(app);
        }
        if !data_dir.is_dir() {
            continue;
        }
        return Ok(data_dir);
    }
    // If no dirs exists in XDG_DATA_DIRS, fallback to /usr/local/share
    let default = "/usr/local/share";
    let mut data_dir = PathBuf::new();
    data_dir.push(default);
    if let Some(app) = app {
        data_dir.push(app);
    }

    if data_dir.is_dir() {
        Ok(data_dir)
    } else {
        Err(())
    }
}

fn get_asset_path() -> PathBuf {
    use std::env::current_exe;

    // Search order for asset path is:
    // 1. ./aw-webui/dist
    // 2. $current_exe_dir/aw_server_rust/static
    //    NOTE: Slightly different for .app bundles on macOS
    // 3. $XDG_DATA_DIR/aw_server_rust/static
    // 4. (fallback) ./aw-webui/dist

    // cargo_dev_path
    // (for running with cargo run)
    let cargo_dev_path = PathBuf::from("./aw-webui/dist/");
    if cargo_dev_path.as_path().exists() {
        return cargo_dev_path;
    }

    // current_exe_path
    // (for self-contained deployed binaries)
    if let Ok(mut current_exe_path) = current_exe() {
        current_exe_path.pop(); // remove name of executable
        current_exe_path.push("./static/");
        if current_exe_path.as_path().exists() {
            return current_exe_path;
        }
    }

    // For .app bundles on macOS
    //
    // On macOS, the executable location is ActivityWatch.app/Contents/MacOS/aw-server-rust,
    // and the webui location is ActivityWatch.app/Contents/Resources/aw_server_rust/static.
    if let Ok(mut current_exe_path) = current_exe() {
        current_exe_path.pop(); // remove name of executable
        current_exe_path.pop(); // step up into the Contents directory
        current_exe_path.push("Resources/aw_server_rust/static/");
        if current_exe_path.as_path().exists() {
            return current_exe_path;
        }
    }

    // usr_path
    // (for linux usr installs)
    if let Ok(mut usr_path) = site_data_dir(Some("aw-server"), None) {
        usr_path.push("static");
        if usr_path.as_path().exists() {
            return usr_path;
        }
    }

    warn!("Unable to find an aw-webui asset path which exists, falling back to ./aw-webui/dist");
    cargo_dev_path
}
