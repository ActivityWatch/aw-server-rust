#[macro_use]
extern crate log;
extern crate getopts;

use getopts::Options;
use rocket::config::Environment;

use std::env;

use aw_server::*;

#[cfg(all(target_os = "linux", target_arch = "x86"))]
extern crate jemallocator;
#[cfg(all(target_os = "linux", target_arch = "x86"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} FILE [options]", program);
    print!("{}", opts.usage(&brief));
}

fn main() {
    use std::sync::Mutex;

    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optflag("", "testing", "run in testing mode");
    opts.optflag("h", "help", "print this help menu");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => panic!(f.to_string()),
    };
    if matches.opt_present("h") {
        print_usage(&program, opts);
        return;
    }

    let mut testing = matches.opt_present("testing");
    // Always override environment if --testing is specified
    if !testing {
        let env = Environment::active().expect("Failed to get current environment");
        testing = match env {
            Environment::Production => false,
            Environment::Development => true,
            Environment::Staging => panic!("Staging environment not supported"),
        };
    }

    logging::setup_logger(testing).expect("Failed to setup logging");

    let config = config::create_config(testing);

    let db_path = dirs::db_path(testing)
        .expect("Failed to get db path")
        .to_str()
        .unwrap()
        .to_string();
    info!("Using DB at path {:?}", db_path);

    let asset_path = get_asset_path();
    info!("Using aw-webui assets at path {:?}", asset_path);

    let server_state = endpoints::ServerState {
        // Even if legacy_import is set to true it is disabled on Android so
        // it will not happen there
        datastore: Mutex::new(aw_datastore::Datastore::new(db_path, true)),
        asset_path,
        device_id: device_id::get_device_id(),
    };

    endpoints::build_rocket(server_state, config).launch();
}

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
    if let Some(joined) = env::var_os("XDG_DATA_DIRS") {
        for mut data_dir in env::split_paths(&joined) {
            if let Some(app) = app {
                data_dir.push(app);
            }
            if !data_dir.is_dir() {
                continue;
            }
            return Ok(data_dir);
        }
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

    // TODO: Add cmdline arg which can override asset path?

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
