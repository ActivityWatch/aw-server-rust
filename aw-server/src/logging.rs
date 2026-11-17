use std::fs;
use std::path::PathBuf;

use fern::colors::{Color, ColoredLevelConfig};

use crate::dirs;

/// Render a single log line.
///
/// `level` is taken as a `Display` so the caller decides whether it is colored:
/// stdout passes a colorized level, the logfile passes the plain one.
fn format_log_line(
    level: &dyn std::fmt::Display,
    target: &str,
    message: &std::fmt::Arguments,
) -> String {
    format!(
        "[{}][{}][{}]: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        level,
        target,
        message,
    )
}

pub fn setup_logger(module: &str, profile: &str, verbose: bool) -> Result<(), fern::InitError> {
    let testing = profile == "testing";
    let mut logfile_path: PathBuf =
        dirs::get_log_dir(module).expect("Unable to get log dir to store logs in");
    fs::create_dir_all(logfile_path.clone()).expect("Unable to create folder for logs");
    // Isolated profile roots (including new-style testing) use a bare filename
    // — the directory already isolates. The `-testing` infix stays only in
    // the legacy shared-root layout so existing log files keep matching.
    let filename = if dirs::legacy_testing_suffix(profile).is_empty() {
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
        // Colored output to stdout
        .chain(
            fern::Dispatch::new()
                .format(move |out, message, record| {
                    out.finish(format_args!(
                        "{}",
                        format_log_line(&colors.color(record.level()), record.target(), message)
                    ))
                })
                .chain(std::io::stdout()),
        )
        // Uncolored output to logfile, so the file doesn't contain ANSI escapes
        .chain(
            fern::Dispatch::new()
                .format(|out, message, record| {
                    out.finish(format_args!(
                        "{}",
                        format_log_line(&record.level(), record.target(), message)
                    ))
                })
                .chain(fern::log_file(logfile_path)?),
        )
        .apply()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{format_log_line, setup_logger};
    use fern::colors::{Color, ColoredLevelConfig};

    /* disable this test.
     * This is due to it failing in GitHub actions, claiming that the logger
     * has been initialized twice which is not allowed */
    #[ignore]
    #[test]
    fn test_setup_logger() {
        setup_logger("aw-server-rust", "testing", true).unwrap();
    }

    /* The formatting is tested directly rather than through setup_logger, which
     * installs the global logger and so can only ever run once per process. */
    #[test]
    fn test_format_log_line_is_plain_for_logfile() {
        let line = format_log_line(&log::Level::Info, "aw_server::test", &format_args!("hello"));

        assert!(
            !line.contains('\u{1b}'),
            "logfile lines must not contain ANSI escapes, got {line:?}"
        );
        assert!(
            line.ends_with("[INFO][aw_server::test]: hello"),
            "unexpected log line: {line:?}"
        );
    }

    #[test]
    fn test_format_log_line_is_colored_for_stdout() {
        let colors = ColoredLevelConfig::new().info(Color::Green);
        let line = format_log_line(
            &colors.color(log::Level::Info),
            "aw_server::test",
            &format_args!("hello"),
        );

        assert!(
            line.contains('\u{1b}'),
            "stdout lines should keep their color, got {line:?}"
        );
        assert!(line.ends_with("[aw_server::test]: hello"));
    }
}
