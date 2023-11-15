use std::fs;
use std::path::PathBuf;

use fern::colors::{Color, ColoredLevelConfig};

use crate::dirs;

pub fn setup_logger(module: &str, testing: bool, verbose: bool) -> Result<(), fern::InitError> {
    let mut logfile_path: PathBuf =
        dirs::get_log_dir(module).expect("Unable to get log dir to store logs in");
    fs::create_dir_all(logfile_path.clone()).expect("Unable to create folder for logs");
    let filename = if !testing {
        format!("{}_%Y-%m-%dT%H-%M-%S%z.log", module)
    } else {
        format!("{}-testing_%Y-%m-%dT%H-%M-%S%z.log", module)
    };

    logfile_path.push(chrono::Local::now().format(&filename).to_string());

    log_panics::init();

    let colors = ColoredLevelConfig::new()
        .debug(Color::White)
        .info(Color::Green)
        .warn(Color::Yellow)
        .error(Color::Red);

    let default_log_level = if testing || verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    let log_level = std::env::var("LOG_LEVEL").map_or(default_log_level, |level| {
        match level.to_lowercase().as_str() {
            "trace" => log::LevelFilter::Trace,
            "debug" => log::LevelFilter::Debug,
            "info" => log::LevelFilter::Info,
            "warn" => log::LevelFilter::Warn,
            "error" => log::LevelFilter::Error,
            _ => default_log_level,
        }
    });

    let mut dispatch = fern::Dispatch::new().level(log_level);
    // Set some Rocket messages to debug level

    let is_debug = matches!(log_level, log::LevelFilter::Trace | log::LevelFilter::Debug);
    if !is_debug {
        dispatch = dispatch
            .level_for("rocket", log::LevelFilter::Warn)
            // rocket_cors has a lot of unhelpful info messages that spam the log on every request
            // https://github.com/ActivityWatch/activitywatch/issues/975
            .level_for("rocket_cors", log::LevelFilter::Warn)
            .level_for("_", log::LevelFilter::Warn) // Rocket requests
            .level_for("launch_", log::LevelFilter::Warn); // Rocket config info
    }

    dispatch
        // Formatting
        .format(move |out, message, record| {
            out.finish(format_args!(
                "[{}][{}][{}]: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                colors.color(record.level()),
                record.target(),
                message,
            ))
        })
        // Color and higher log levels to stdout
        .chain(fern::Dispatch::new().chain(std::io::stdout()))
        // No color and lower log levels to logfile
        .chain(
            fern::Dispatch::new()
                .format(|out, message, _record| {
                    out.finish(format_args!(
                        // TODO: Strip color info
                        "{message}",
                    ))
                })
                .chain(fern::log_file(logfile_path)?),
        )
        .apply()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::setup_logger;

    /* disable this test.
     * This is due to it failing in GitHub actions, claiming that the logger
     * has been initialized twice which is not allowed */
    #[ignore]
    #[test]
    fn test_setup_logger() {
        setup_logger("aw-server-rust", true, true).unwrap();
    }
}
