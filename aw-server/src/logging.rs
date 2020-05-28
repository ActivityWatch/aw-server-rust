use std::fs;
use std::path::PathBuf;

use fern::colors::{Color, ColoredLevelConfig};

use crate::dirs;

pub fn setup_logger(testing: bool) -> Result<(), fern::InitError> {
    let mut logfile_path: PathBuf =
        dirs::get_log_dir().expect("Unable to get log dir to store logs in");
    fs::create_dir_all(logfile_path.clone()).expect("Unable to create folder for logs");
    logfile_path.push(
        chrono::Local::now()
            .format(if !testing {
                "aw-server_%Y-%m-%dT%H-%M-%S%z.log"
            } else {
                "aw-server-testing_%Y-%m-%dT%H-%M-%S%z.log"
            })
            .to_string(),
    );

    let colors = ColoredLevelConfig::new()
        .debug(Color::White)
        .info(Color::Green)
        .warn(Color::Yellow)
        .error(Color::Red);

    fern::Dispatch::new()
        // Set some Rocket messages to debug level
        // TODO: Log more if run in development/testing mode
        .level(log::LevelFilter::Info)
        .level_for("rocket", log::LevelFilter::Warn)
        .level_for("_", log::LevelFilter::Warn) // Rocket requests
        .level_for("launch_", log::LevelFilter::Warn) // Rocket config info
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
                        "{}",
                        message,
                    ))
                })
                .chain(fern::log_file(logfile_path)?),
        )
        .apply()?;

    Ok(())
}

#[test]
fn test_setup_logger() {
    setup_logger(true).unwrap();
}
