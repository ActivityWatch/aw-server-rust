//! `aw-client`: command-line utility for talking to an ActivityWatch server, the Rust
//! counterpart of aw-client-python's `aw-client` CLI.
//!
//! Built only with the `cli` feature: `cargo run -p aw-client-rust --features cli -- --help`.

use std::error::Error;
use std::io::Write;

use aw_client_rust::blocking::AwClient;
use aw_client_rust::classes::{self, CategoryId, CategorySpec};
use aw_client_rust::queries::{self, DesktopQueryParams, QueryParams, QueryParamsBase};
use aw_client_rust::Event;
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aw-client",
    about = "CLI utility for aw-client to aid in interacting with the ActivityWatch server"
)]
struct Cli {
    /// Address of host
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Port to use [default: 5600, or 5666 with --testing]
    #[arg(long)]
    port: Option<u16>,
    /// Use the testing server port by default
    #[arg(long)]
    testing: bool,
    /// API key, if the server requires one
    #[arg(long, env = "AW_API_KEY", hide_env_values = true)]
    api_key: Option<String>,
    /// Print the generated queries to stderr
    #[arg(short, long)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send a heartbeat to bucket with ID `bucket_id` with JSON `data`
    Heartbeat {
        bucket_id: String,
        data: String,
        /// Pulsetime to use for merging heartbeats
        #[arg(long, default_value_t = 60.0)]
        pulsetime: f64,
    },
    /// List all buckets
    Buckets,
    /// List events from bucket with ID `bucket_id`
    Events { bucket_id: String },
    /// Run a query in file at `path` on the server
    Query {
        path: String,
        /// Print the raw JSON result
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        range: Range,
    },
    /// Generate an activity report for a host
    Report {
        hostname: String,
        #[command(flatten)]
        range: Range,
        /// Number of rows per table (at most 100, the number of titles the query returns)
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
    },
    /// Query 'canonical events' for a single host (filtered, classified)
    Canonical {
        hostname: String,
        #[command(flatten)]
        range: Range,
    },
}

#[derive(clap::Args)]
struct Range {
    /// Start of the time period: RFC 3339, or a local date/time such as
    /// `2024-05-01` or `2024-05-01 09:00` [default: 24 hours ago]
    #[arg(long, value_parser = parse_datetime)]
    start: Option<DateTime<Utc>>,
    /// End of the time period, same formats as --start [default: a year from now]
    #[arg(long, value_parser = parse_datetime)]
    stop: Option<DateTime<Utc>>,
}

impl Range {
    fn resolve(&self) -> (DateTime<Utc>, DateTime<Utc>) {
        let now = Utc::now();
        (
            self.start.unwrap_or(now - Duration::days(1)),
            self.stop.unwrap_or(now + Duration::days(365)),
        )
    }
}

/// Parse RFC 3339, or a date/time without an offset in the local time zone (as the
/// Python CLI's report and canonical commands do).
fn parse_datetime(value: &str) -> Result<DateTime<Utc>, String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }
    let naive = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
    .or_else(|| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
    })
    .ok_or_else(|| format!("invalid date/time {value:?}"))?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.with_timezone(&Utc))
        .ok_or_else(|| format!("{value:?} doesn't exist in the local time zone"))
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli, &mut std::io::stdout().lock()) {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli, out: &mut impl Write) -> Result<(), Box<dyn Error>> {
    let port = cli.port.unwrap_or(if cli.testing { 5666 } else { 5600 });
    // A per-process name, so concurrent invocations don't trip the single-instance lock.
    let name = format!("aw-client-cli-{}", std::process::id());
    let client = AwClient::new_with_api_key(&cli.host, port, &name, cli.api_key)?;

    match cli.command {
        Command::Heartbeat {
            bucket_id,
            data,
            pulsetime,
        } => {
            let data = match serde_json::from_str(&data)? {
                serde_json::Value::Object(map) => map,
                _ => return Err("data must be a JSON object".into()),
            };
            let event = Event {
                id: None,
                timestamp: Utc::now(),
                duration: Duration::zero(),
                data,
            };
            client.heartbeat(&bucket_id, &event, pulsetime)?;
            writeln!(out, "{}", serde_json::to_string(&event)?)?;
        }
        Command::Buckets => {
            let mut ids: Vec<_> = client.get_buckets()?.into_keys().collect();
            ids.sort();
            writeln!(out, "Buckets:")?;
            for id in ids {
                writeln!(out, " - {}", printable(&id))?;
            }
        }
        Command::Events { bucket_id } => {
            writeln!(out, "events:")?;
            for event in client.get_events(&bucket_id, None, None, None)? {
                writeln!(
                    out,
                    " - {} ({}) {}",
                    event.timestamp.format("%Y-%m-%d %H:%M:%S"),
                    fmt_duration(event.duration),
                    serde_json::Value::Object(event.data)
                )?;
            }
        }
        Command::Query { path, json, range } => {
            let query = std::fs::read_to_string(&path)?;
            let result = client.query(&query, vec![range.resolve()])?;
            if json {
                writeln!(out, "{}", serde_json::to_string(&result)?)?;
            } else {
                for period in result {
                    let events: Vec<Event> = serde_json::from_value(period)
                        .map_err(|err| format!("query didn't return a list of events: {err}"))?;
                    writeln!(
                        out,
                        "Showing {} out of {} events:",
                        events.len().min(10),
                        events.len()
                    )?;
                    for event in events.iter().take(10) {
                        writeln!(
                            out,
                            " - Duration: {} \tData: {}",
                            fmt_duration(event.duration),
                            serde_json::Value::Object(event.data.clone())
                        )?;
                    }
                    writeln!(out, "Total duration:\t {}", fmt_duration(total(&events)))?;
                }
            }
        }
        Command::Report {
            hostname,
            range,
            limit,
        } => {
            let limit = usize::from(limit);
            let params = desktop_params(&hostname, server_classes(&client));
            let query = queries::full_desktop_query(&params);
            if cli.verbose {
                eprintln!("Query:\n{query}");
            }
            for period in client.query(&query, vec![range.resolve()])? {
                writeln!(out)?;
                let cat_events: Vec<Event> =
                    serde_json::from_value(period["window"]["cat_events"].clone())?;
                print_top(out, &cat_events, "Categories", limit, |e| {
                    category_name(&e.data)
                })?;
                let title_events: Vec<Event> =
                    serde_json::from_value(period["window"]["title_events"].clone())?;
                print_top(out, &title_events, "Titles", limit, |e| {
                    data_str(&e.data, "title")
                })?;
                let duration = period["window"]["duration"].as_f64().unwrap_or(0.0);
                writeln!(
                    out,
                    "Total duration:\t {}",
                    fmt_duration(Duration::milliseconds((duration * 1000.0) as i64))
                )?;
            }
        }
        Command::Canonical { hostname, range } => {
            let params = desktop_params(&hostname, classes::default_classes());
            let query = format!(
                "{}\nRETURN = events;",
                QueryParams::Desktop(params).canonical_events()
            );
            if cli.verbose {
                eprintln!("Query:\n{query}");
            }
            for period in client.query(&query, vec![range.resolve()])? {
                writeln!(out)?;
                let events: Vec<Event> = serde_json::from_value(period)?;
                writeln!(out, "Showing last 10 out of {} events:", events.len())?;
                let rows: Vec<Vec<String>> = events[events.len().saturating_sub(10)..]
                    .iter()
                    .map(|e| {
                        vec![
                            e.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                            fmt_duration(e.duration),
                            format!(
                                "[{}] {}",
                                data_str(&e.data, "app"),
                                shorten(&data_str(&e.data, "title"), 60)
                            ),
                        ]
                    })
                    .collect();
                write_table(out, &["Timestamp", "Duration", "Data"], &rows)?;
                writeln!(out)?;
                writeln!(out, "Total duration:\t {}", fmt_duration(total(&events)))?;
            }
        }
    }
    Ok(())
}

fn desktop_params(hostname: &str, classes: Vec<(CategoryId, CategorySpec)>) -> DesktopQueryParams {
    DesktopQueryParams {
        base: QueryParamsBase {
            bid_browsers: vec![],
            classes,
            filter_classes: vec![],
            filter_afk: true,
            include_audible: true,
        },
        bid_window: format!("aw-watcher-window_{hostname}"),
        bid_afk: format!("aw-watcher-afk_{hostname}"),
        always_active_pattern: None,
    }
}

/// The server's categorization classes, or the defaults if they can't be fetched.
fn server_classes(client: &AwClient) -> Vec<(CategoryId, CategorySpec)> {
    match client.get_setting("classes") {
        Ok(value) => classes::classes_from_settings_json(&value),
        Err(err) => {
            eprintln!("Failed to get classes from server, using default classes: {err}");
            classes::default_classes()
        }
    }
}

fn total(events: &[Event]) -> Duration {
    events
        .iter()
        .fold(Duration::zero(), |sum, e| sum + e.duration)
}

fn data_str(data: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    match data.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn category_name(data: &serde_json::Map<String, serde_json::Value>) -> String {
    match data.get("$category") {
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .map(|p| {
                p.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| p.to_string())
            })
            .collect::<Vec<_>>()
            .join(" > "),
        _ => data_str(data, "$category"),
    }
}

/// Escape control characters (e.g. terminal escape sequences in a recorded window title)
/// so printing event data can't drive the terminal.
fn printable(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

/// `H:MM:SS`, like Python's `str(timedelta)` without the fractional seconds.
fn fmt_duration(duration: Duration) -> String {
    let secs = duration.num_seconds().max(0);
    format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// Collapse whitespace and cut to `width` characters with a `...` placeholder.
fn shorten(text: &str, width: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= width {
        return text;
    }
    let cut: String = text.chars().take(width.saturating_sub(3)).collect();
    format!("{}...", cut.trim_end())
}

fn print_top(
    out: &mut impl Write,
    events: &[Event],
    title: &str,
    n: usize,
    key: impl Fn(&Event) -> String,
) -> std::io::Result<()> {
    let mut sorted: Vec<&Event> = events.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.duration));
    let suffix = if events.len() > n {
        format!(" (out of {})", events.len())
    } else {
        String::new()
    };
    writeln!(out, "Top {n} {title}{suffix}")?;
    let rows: Vec<Vec<String>> = sorted
        .iter()
        .take(n)
        .map(|e| vec![fmt_duration(e.duration), key(e)])
        .collect();
    write_table(out, &["Duration", "Key"], &rows)?;
    writeln!(out)
}

/// A plain left-aligned table with a dashed rule under the headers.
fn write_table(
    out: &mut impl Write,
    headers: &[&str],
    rows: &[Vec<String>],
) -> std::io::Result<()> {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| printable(cell)).collect())
        .collect();
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain([h.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: Vec<String>| {
        cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    writeln!(
        out,
        "{}",
        line(headers.iter().map(|h| h.to_string()).collect())
    )?;
    writeln!(
        out,
        "{}",
        line(widths.iter().map(|w| "-".repeat(*w)).collect())
    )?;
    for row in &rows {
        writeln!(out, "{}", line(row.clone()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations_like_python_timedelta() {
        assert_eq!(fmt_duration(Duration::milliseconds(1500)), "0:00:01");
        assert_eq!(fmt_duration(Duration::seconds(3 * 3600 + 62)), "3:01:02");
        assert_eq!(fmt_duration(Duration::seconds(26 * 3600)), "26:00:00");
    }

    #[test]
    fn parses_rfc3339_and_local_datetimes() {
        let utc = parse_datetime("2024-05-01T09:00:00Z").unwrap();
        assert_eq!(utc, Utc.with_ymd_and_hms(2024, 5, 1, 9, 0, 0).unwrap());
        let local = Local.with_ymd_and_hms(2024, 5, 1, 9, 30, 0).unwrap();
        assert_eq!(parse_datetime("2024-05-01 09:30").unwrap(), local);
        assert_eq!(parse_datetime("2024-05-01T09:30:00").unwrap(), local);
        let midnight = Local.with_ymd_and_hms(2024, 5, 1, 0, 0, 0).unwrap();
        assert_eq!(parse_datetime("2024-05-01").unwrap(), midnight);
        assert!(parse_datetime("yesterday").is_err());
    }

    #[test]
    fn shortens_long_titles() {
        assert_eq!(shorten("short   title", 60), "short title");
        let long = "word ".repeat(30);
        let short = shorten(&long, 20);
        assert!(
            short.ends_with("...") && short.chars().count() <= 20,
            "{short}"
        );
    }

    #[test]
    fn escapes_control_characters_in_tables() {
        let mut out = Vec::new();
        write_table(
            &mut out,
            &["Key"],
            &[vec!["evil\u{1b}]0;pwned\u{7}\ttitle".into()]],
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(!out.chars().any(|c| c.is_control() && c != '\n'), "{out:?}");
        assert!(out.contains("evil\\u{1b}]0;pwned\\u{7}\\ttitle"), "{out:?}");
    }

    #[test]
    fn writes_aligned_tables() {
        let mut out = Vec::new();
        write_table(
            &mut out,
            &["Duration", "Key"],
            &[vec!["0:00:05".into(), "Work".into()]],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Duration  Key\n--------  ----\n0:00:05   Work\n"
        );
    }
}
