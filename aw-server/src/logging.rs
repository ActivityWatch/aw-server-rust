use std::fs;
use std::path::PathBuf;

use fern::colors::{Color, ColoredLevelConfig};

use crate::dirs;

pub fn setup_logger() -> Result<(), fern::InitError> {
    let mut logfile_path: PathBuf =
        dirs::get_cache_dir().expect("Unable to get cache dir to store logs in");
    logfile_path.push("logs");
    fs::create_dir_all(logfile_path.clone()).expect("Unable to create folder for logs");
    #[cfg(debug_assertions)]
    {
        logfile_path.push(
            chrono::Local::now()
                .format("aw-server-testing_%Y-%m-%dT%H-%M-%S%z.log")
                .to_string(),
        );
    }
    #[cfg(not(debug_assertions))]
    {
        logfile_path.push(
            chrono::Local::now()
                .format("aw-server_%Y-%m-%dT%H-%M-%S%z.log")
                .to_string(),
        );
    }

    let colors = ColoredLevelConfig::new()
        .debug(Color::White)
        .info(Color::Green)
        .warn(Color::Yellow)
        .error(Color::Red);

    fern::Dispatch::new()
        // Color and higher log levels to stdout
        .chain(
            fern::Dispatch::new()
                .format(move |out, message, record| {
                    out.finish(format_args!(
                        "{}[{}][{}] {}",
                        chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                        record.target(),
                        colors.color(record.level()),
                        message
                    ))
                })
                .chain(std::io::stdout())
                .level(log::LevelFilter::Info), //.level_for("aw_server", log::LevelFilter::Debug)
        )
        // No color and lower log levels to logfile
        .chain(
            fern::Dispatch::new()
                .format(|out, message, record| {
                    out.finish(format_args!(
                        "{}[{}][{}] {}",
                        chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                        record.target(),
                        record.level(),
                        message
                    ))
                })
                .chain(fern::log_file(logfile_path)?)
                .level(log::LevelFilter::Warn)
                .level_for("aw_server", log::LevelFilter::Info),
        )
        .apply()?;

    Ok(())
}
