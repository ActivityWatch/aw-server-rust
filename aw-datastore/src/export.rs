use std::collections::{HashMap, HashSet};
use std::io::Write;

use aw_models::{Bucket, Event};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{
    ser::{Error, SerializeMap, SerializeSeq},
    Serialize, Serializer,
};

use crate::datastore::{events_source, parse_event_row, prefer_endtime_index};
use crate::DatastoreError;

struct EventRows<'a> {
    conn: &'a Connection,
    db_version: i32,
    bucket: &'a Bucket,
}

impl Serialize for EventRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let sql = format!(
            "SELECT id, starttime, endtime, data
             FROM {}
             WHERE bucketrow = ?1 AND endtime >= 0 AND starttime <= ?2
             ORDER BY starttime DESC, endtime ASC, id ASC",
            events_source(self.db_version, false)
        );
        let mut stmt = self.conn.prepare_cached(&sql).map_err(S::Error::custom)?;
        let mut rows = stmt
            .query(rusqlite::params![self.bucket.bid.unwrap(), i64::MAX])
            .map_err(S::Error::custom)?;
        let mut seq = serializer.serialize_seq(None)?;
        while let Some(row) = rows.next().map_err(S::Error::custom)? {
            // Match get_events' default range, clipping and corrupt-row policy.
            match crate::datastore::parse_event_row(row, Some((0, i64::MAX))) {
                Ok(event) => seq.serialize_element(&event)?,
                Err(err) => warn!("Corrupt event in bucket {}: {}", self.bucket.id, err),
            }
        }
        seq.end()
    }
}

struct ExportBucket<'a> {
    conn: &'a Connection,
    db_version: i32,
    bucket: &'a Bucket,
}

impl Serialize for ExportBucket<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Reuse Bucket's field names and serialization rules, replacing only
        // events. This allocation contains bucket metadata, never event rows.
        let metadata = serde_json::to_value(self.bucket).map_err(S::Error::custom)?;
        let metadata = metadata
            .as_object()
            .ok_or_else(|| S::Error::custom("invalid bucket metadata"))?;
        let mut map = serializer.serialize_map(Some(metadata.len()))?;
        for (key, value) in metadata {
            if key != "events" {
                map.serialize_entry(key, value)?;
            }
        }
        map.serialize_entry(
            "events",
            &EventRows {
                conn: self.conn,
                db_version: self.db_version,
                bucket: self.bucket,
            },
        )?;
        map.end()
    }
}

struct ExportBuckets<'a> {
    conn: &'a Connection,
    db_version: i32,
    buckets: &'a HashMap<String, Bucket>,
    selected: Option<&'a str>,
}

impl Serialize for ExportBuckets<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (id, bucket) in self.buckets {
            if self.selected.is_none() || self.selected == Some(id.as_str()) {
                map.serialize_entry(
                    id,
                    &ExportBucket {
                        conn: self.conn,
                        db_version: self.db_version,
                        bucket,
                    },
                )?;
            }
        }
        map.end()
    }
}

pub(crate) fn write_export(
    conn: &Connection,
    db_version: i32,
    buckets: &HashMap<String, Bucket>,
    selected: Option<&str>,
    writer: impl Write,
) -> Result<(), DatastoreError> {
    if let Some(id) = selected {
        if !buckets.contains_key(id) {
            return Err(DatastoreError::NoSuchBucket(id.to_owned()));
        }
    }
    #[derive(Serialize)]
    struct Export<'a> {
        buckets: ExportBuckets<'a>,
    }
    serde_json::to_writer(
        writer,
        &Export {
            buckets: ExportBuckets {
                conn,
                db_version,
                buckets,
                selected,
            },
        },
    )
    .map_err(|err| DatastoreError::InternalError(format!("Failed to write export: {err}")))
}

fn csv_io_err(err: std::io::Error) -> DatastoreError {
    DatastoreError::InternalError(format!("Failed to write CSV export: {err}"))
}

/// Prefix spreadsheet-formula starters so Excel/Sheets will not execute them.
/// Characters spreadsheets ignore before evaluating a formula starter
/// (Excel and LibreOffice trim leading whitespace, so " =1+1" executes).
const FORMULA_LEADING_WHITESPACE: [char; 4] = [' ', '\t', '\r', '\n'];

/// Prefix spreadsheet-formula starters so Excel/Sheets will not execute them.
/// Looks past leading whitespace so values like " =1+1" are neutralized too.
fn neutralize_formula(s: &str) -> String {
    let trimmed = s.trim_start_matches(FORMULA_LEADING_WHITESPACE);
    match trimmed.chars().next() {
        Some('=' | '+' | '-' | '@') => format!("'{s}"),
        _ => s.to_owned(),
    }
}

/// RFC-4180 field escaping, with formula neutralization applied first.
fn csv_escape(s: &str) -> String {
    let s = neutralize_formula(s);
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// Exact fractional-second duration, matching the JSON nanosecond contract
/// without going through `num_milliseconds()` (which truncates sub-ms).
///
/// Composed from whole seconds and the signed subsecond remainder rather than
/// `num_nanoseconds()`, which returns `None` (and would export a silent
/// `0.000000000`) for magnitudes outside the `i64` nanosecond range.
fn duration_csv(duration: &chrono::Duration) -> String {
    // chrono stores a negative duration as a negative whole-second part plus a
    // non-negative subsecond remainder: -1.5s is secs=-2, nanos=500_000_000.
    // The sign therefore has to be taken off before splitting into seconds and
    // nanos — `num_seconds()` alone reports -2 for -1.5s, and pairing it with
    // the remainder would render "-2.500000000". `duration` comes from
    // `Duration::nanoseconds(endtime - starttime)`, so negating cannot overflow.
    let (sign, magnitude) = if *duration < chrono::Duration::zero() {
        ("-", -*duration)
    } else {
        ("", *duration)
    };
    format!(
        "{sign}{}.{:09}",
        magnitude.num_seconds(),
        magnitude.subsec_nanos()
    )
}

/// SQLite binds `LIMIT` as a signed 64-bit integer, and a negative limit means
/// "unbounded". Converting a `u64` limit with `as` would wrap values above
/// `i64::MAX` to a negative number, turning the client's cap into an unbounded
/// export; saturate instead.
fn sql_limit(limit_opt: Option<u64>) -> i64 {
    match limit_opt {
        Some(l) => i64::try_from(l).unwrap_or(i64::MAX),
        None => -1,
    }
}

fn event_field_value(event: &Event, key: &str) -> String {
    match event.data.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

fn write_csv_record(
    writer: &mut impl Write,
    fields: impl IntoIterator<Item = String>,
) -> Result<(), DatastoreError> {
    let mut first = true;
    for field in fields {
        if !first {
            writer.write_all(b",").map_err(csv_io_err)?;
        }
        first = false;
        writer
            .write_all(csv_escape(&field).as_bytes())
            .map_err(csv_io_err)?;
    }
    // RFC-4180 puts each record on its own line delimited by CRLF.
    writer.write_all(b"\r\n").map_err(csv_io_err)
}

/// Maximum number of `data` keys exported as individual CSV columns.
///
/// A bucket whose events each carry a distinct key would otherwise emit
/// `events × keys` cells — quadratic in the event count, and enough to fill the
/// staging file. Above this cap the export keeps a single `data` column holding
/// each event's full data object as JSON: bounded, and still lossless.
const MAX_CSV_DATA_COLUMNS: usize = 32;

/// Resolve the data columns for an export: the union of keys, or a single
/// `data` JSON column when that union exceeds `MAX_CSV_DATA_COLUMNS`.
fn csv_data_columns(key_order: Vec<String>) -> (Vec<String>, bool) {
    if key_order.len() > MAX_CSV_DATA_COLUMNS {
        (vec!["data".to_string()], true)
    } else {
        (key_order, false)
    }
}

/// Header names for the columns: `id`, `timestamp`, `duration`, then one per
/// data key. A data key that would repeat an earlier header name (e.g. a data
/// key `duration`) is prefixed with `data.` until unique, so every column name
/// is distinct. Only the header changes; values are still read by the original
/// key.
fn csv_header_names(data_keys: &[String]) -> Vec<String> {
    let mut fields = vec![
        "id".to_string(),
        "timestamp".to_string(),
        "duration".to_string(),
    ];
    let mut used: HashSet<String> = fields.iter().cloned().collect();
    for key in data_keys {
        let mut name = key.clone();
        while used.contains(&name) {
            name = format!("data.{name}");
        }
        used.insert(name.clone());
        fields.push(name);
    }
    fields
}

fn write_csv_header(writer: &mut impl Write, data_keys: &[String]) -> Result<(), DatastoreError> {
    write_csv_record(writer, csv_header_names(data_keys))
}

fn write_csv_event(
    writer: &mut impl Write,
    event: &Event,
    data_keys: &[String],
    data_json_column: bool,
) -> Result<(), DatastoreError> {
    let mut fields = vec![
        event.id.map(|i| i.to_string()).unwrap_or_default(),
        event.timestamp.to_rfc3339(),
        duration_csv(&event.duration),
    ];
    if data_json_column {
        fields.push(serde_json::Value::Object(event.data.clone()).to_string());
    } else {
        for key in data_keys {
            fields.push(event_field_value(event, key));
        }
    }
    write_csv_record(writer, fields)
}

/// Stream events for one bucket as RFC-4180 CSV, writing one row at a time.
///
/// Columns: `id`, `timestamp`, `duration`, then the union of data keys across
/// all matched events, collected in a pre-pass so heterogeneous event data is
/// not truncated to the first event's schema. A data key that collides with an
/// earlier column name gets a `data.` prefix (see `csv_header_names`). If the
/// union exceeds
/// `MAX_CSV_DATA_COLUMNS`, a single `data` column holds each event's JSON data
/// object instead (bounded output, no keys dropped).
/// Query filters, clipping, and corrupt-row skipping match `get_events`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_events_csv(
    conn: &Connection,
    db_version: i32,
    buckets: &HashMap<String, Bucket>,
    bucket_id: &str,
    starttime_opt: Option<DateTime<Utc>>,
    endtime_opt: Option<DateTime<Utc>>,
    limit_opt: Option<u64>,
    mut writer: impl Write,
) -> Result<(), DatastoreError> {
    let bucket = match buckets.get(bucket_id) {
        Some(bucket) => bucket,
        None => return Err(DatastoreError::NoSuchBucket(bucket_id.to_owned())),
    };

    let starttime_filter_ns: i64 = match starttime_opt {
        Some(dt) => dt.timestamp_nanos_opt().unwrap(),
        None => 0,
    };
    let endtime_filter_ns: i64 = match endtime_opt {
        Some(dt) => dt.timestamp_nanos_opt().unwrap(),
        None => i64::MAX,
    };
    if starttime_filter_ns > endtime_filter_ns {
        warn!("Starttime in event query was lower than endtime!");
        write_csv_header(&mut writer, &[])?;
        return writer.flush().map_err(csv_io_err);
    }
    let limit = sql_limit(limit_opt);

    let source = events_source(
        db_version,
        prefer_endtime_index(bucket, starttime_filter_ns, endtime_filter_ns, limit_opt),
    );
    let sql = format!(
        "SELECT id, starttime, endtime, data
             FROM {source}
             WHERE bucketrow = ?1 AND endtime >= ?2 AND starttime <= ?3
             ORDER BY starttime DESC, endtime ASC, id ASC LIMIT ?4"
    );
    // First pass: collect the union of data keys across matched rows so the
    // header includes keys the first event may lack. Rows are still streamed
    // in the second pass, and the key set is bounded by
    // `MAX_CSV_DATA_COLUMNS`: collection stops as soon as the union exceeds
    // the cap, because the fallback decision is already made at that point.
    let keys_sql = sql.replace("SELECT id, starttime, endtime, data", "SELECT data");
    let mut key_order: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut keys_stmt = conn.prepare_cached(&keys_sql).map_err(|err| {
        DatastoreError::InternalError(format!("Failed to prepare CSV export key pass: {err}"))
    })?;
    let mut key_rows = keys_stmt
        .query(rusqlite::params![
            bucket.bid.unwrap(),
            starttime_filter_ns,
            endtime_filter_ns,
            limit,
        ])
        .map_err(|err| {
            DatastoreError::InternalError(format!("Failed to query CSV export key pass: {err}"))
        })?;
    let mut key_overflow = false;
    while let Some(row) = key_rows.next().map_err(|err| {
        DatastoreError::InternalError(format!("Failed to read CSV export key row: {err}"))
    })? {
        let data: Option<String> = row.get(0).map_err(|err| {
            DatastoreError::InternalError(format!("Failed to read CSV export key data: {err}"))
        })?;
        let data = match data {
            Some(d) => d,
            None => continue,
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(obj) = value.as_object() {
                for key in obj.keys() {
                    if seen.insert(key.clone()) {
                        key_order.push(key.clone());
                        if key_order.len() > MAX_CSV_DATA_COLUMNS {
                            key_overflow = true;
                            break;
                        }
                    }
                }
            }
        }
        if key_overflow {
            break;
        }
    }
    drop(key_rows);
    drop(keys_stmt);

    let mut stmt = conn.prepare_cached(&sql).map_err(|err| {
        DatastoreError::InternalError(format!("Failed to prepare CSV export SQL: {err}"))
    })?;
    let mut rows = stmt
        .query(rusqlite::params![
            bucket.bid.unwrap(),
            starttime_filter_ns,
            endtime_filter_ns,
            limit,
        ])
        .map_err(|err| {
            DatastoreError::InternalError(format!("Failed to query CSV export SQL: {err}"))
        })?;

    let clip = Some((starttime_filter_ns, endtime_filter_ns));
    let (data_keys, data_json_column) = csv_data_columns(key_order);
    write_csv_header(&mut writer, &data_keys)?;
    while let Some(row) = rows.next().map_err(|err| {
        DatastoreError::InternalError(format!("Failed to read CSV export row: {err}"))
    })? {
        let event = match parse_event_row(row, clip) {
            Ok(event) => event,
            Err(err) => {
                warn!("Corrupt event in bucket {}: {}", bucket_id, err);
                continue;
            }
        };
        write_csv_event(&mut writer, &event, &data_keys, data_json_column)?;
    }
    writer.flush().map_err(csv_io_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DatastoreInstance;
    use aw_models::{BucketMetadata, BucketsExport, Event, TryVec};
    use chrono::{DateTime, Duration};

    fn setup() -> (Connection, DatastoreInstance) {
        let conn = Connection::open_in_memory().unwrap();
        let mut ds = DatastoreInstance::new(&conn, true).unwrap();
        for id in ["populated", "empty"] {
            ds.create_bucket(
                &conn,
                Bucket {
                    bid: None,
                    id: id.into(),
                    _type: "test".into(),
                    client: "test".into(),
                    hostname: "host".into(),
                    created: None,
                    data: Default::default(),
                    metadata: BucketMetadata::default(),
                    events: None,
                    last_updated: None,
                },
            )
            .unwrap();
        }
        let events = [(-10, 20), (5, 20), (5, 10), (5, 10), (30, 0)]
            .into_iter()
            .map(|(start, duration)| {
                Event::new(
                    DateTime::from_timestamp(start, 0).unwrap(),
                    Duration::seconds(duration),
                    serde_json::from_value(
                        serde_json::json!({"text": "quotes \" and unicode ☀", "nested": [1, true]}),
                    )
                    .unwrap(),
                )
            })
            .collect();
        ds.insert_events(&conn, "populated", events).unwrap();
        conn.execute("INSERT INTO events(bucketrow,starttime,endtime,data) VALUES(1,6000000000,7000000000,'invalid json')", []).unwrap();
        (conn, ds)
    }

    #[test]
    fn streamed_json_matches_materialized_exports() {
        let (conn, mut ds) = setup();
        for selected in [None, Some("populated"), Some("empty")] {
            let mut buckets = ds.get_buckets();
            buckets.retain(|id, _| selected.is_none() || selected == Some(id.as_str()));
            for (id, bucket) in &mut buckets {
                bucket.events = Some(TryVec::new(
                    ds.get_events(&conn, id, None, None, None).unwrap(),
                ));
            }
            let expected = serde_json::to_value(BucketsExport { buckets }).unwrap();
            let mut output = Vec::new();
            ds.write_export(&conn, selected, &mut output).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn missing_bucket_and_writer_failures_propagate() {
        let (conn, ds) = setup();
        let mut output = Vec::new();
        assert!(matches!(
            ds.write_export(&conn, Some("missing"), &mut output),
            Err(DatastoreError::NoSuchBucket(_))
        ));
        assert!(output.is_empty());
        struct FailingWriter(usize);
        impl Write for FailingWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, "disk full"));
                }
                let n = bytes.len().min(self.0);
                self.0 -= n;
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(matches!(
            ds.write_export(&conn, None, FailingWriter(500)),
            Err(DatastoreError::InternalError(_))
        ));
    }

    #[test]
    fn csv_duration_preserves_sub_millisecond_nanos() {
        assert_eq!(
            duration_csv(&Duration::nanoseconds(1_500_000)),
            "0.001500000"
        );
        assert_eq!(duration_csv(&Duration::milliseconds(1)), "0.001000000");
        assert_eq!(duration_csv(&Duration::seconds(0)), "0.000000000");
    }

    #[test]
    fn csv_duration_handles_negative_durations() {
        // -1.5s is stored by chrono as secs=-2, nanos=500_000_000; taking the
        // magnitude before splitting is what keeps this at -1.5 and not -2.5.
        assert_eq!(
            duration_csv(&Duration::milliseconds(-1_500)),
            "-1.500000000"
        );
        assert_eq!(
            duration_csv(&Duration::nanoseconds(-1_500_000)),
            "-0.001500000"
        );
        assert_eq!(duration_csv(&Duration::seconds(-1)), "-1.000000000");
    }

    #[test]
    fn csv_limit_saturates_instead_of_wrapping() {
        assert_eq!(sql_limit(None), -1);
        assert_eq!(sql_limit(Some(10)), 10);
        assert_eq!(sql_limit(Some(i64::MAX as u64)), i64::MAX);
        // Would have wrapped to -1 (SQLite: "no limit") under `as`.
        assert_eq!(sql_limit(Some(u64::MAX)), i64::MAX);
    }

    #[test]
    fn csv_escape_neutralizes_formula_prefixes_and_quotes_rfc4180() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("=1+1"), "'=1+1");
        assert_eq!(csv_escape("+cmd"), "'+cmd");
        assert_eq!(csv_escape("-1+1"), "'-1+1");
        assert_eq!(csv_escape("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(
            csv_escape("A \"quoted\" title"),
            "\"A \"\"quoted\"\" title\""
        );
        assert_eq!(csv_escape("=1,2"), "\"'=1,2\"");
    }

    #[test]
    fn csv_escape_neutralizes_whitespace_prefixed_formulas() {
        assert_eq!(csv_escape(" =1+1"), "' =1+1");
        assert_eq!(csv_escape("\t+cmd"), "'\t+cmd");
        assert_eq!(csv_escape(" plain"), " plain");
    }

    #[test]
    fn csv_header_prefixes_data_keys_that_collide_with_columns() {
        let keys: Vec<String> = ["app", "duration", "id", "data.id", "timestamp"]
            .iter()
            .map(|k| k.to_string())
            .collect();
        assert_eq!(
            csv_header_names(&keys),
            [
                "id",
                "timestamp",
                "duration",
                "app",
                "data.duration",
                "data.id",
                "data.data.id",
                "data.timestamp",
            ]
        );
    }

    #[test]
    fn streamed_csv_columns_use_union_of_keys_across_events() {
        let (conn, mut ds) = setup();
        let events = [
            serde_json::json!({"app": "firefox", "title": "t"}),
            serde_json::json!({"app": "firefox", "url": "u"}),
        ]
        .into_iter()
        .map(|data| {
            Event::new(
                DateTime::from_timestamp(0, 0).unwrap(),
                Duration::seconds(1),
                serde_json::from_value(data).unwrap(),
            )
        })
        .collect();
        ds.insert_events(&conn, "empty", events).unwrap();

        let mut output = Vec::new();
        ds.write_events_csv(&conn, "empty", None, None, None, &mut output)
            .unwrap();
        let csv = String::from_utf8(output).unwrap();
        let header = csv.lines().next().unwrap();
        assert!(header.ends_with(",app,title,url"), "header: {header}");
        assert!(csv.contains(",u"), "second row lost url: {csv}");
    }

    #[test]
    fn csv_falls_back_to_a_json_data_column_for_wide_schemas() {
        let (conn, mut ds) = setup();
        // One distinct key per event: the union grows with the event count.
        let events: Vec<Event> = (0..MAX_CSV_DATA_COLUMNS + 1)
            .map(|i| {
                let mut data = serde_json::Map::new();
                data.insert(format!("k{i}"), serde_json::json!(i));
                Event::new(
                    DateTime::from_timestamp(i as i64, 0).unwrap(),
                    Duration::seconds(1),
                    data,
                )
            })
            .collect();
        ds.insert_events(&conn, "empty", events).unwrap();

        let mut output = Vec::new();
        ds.write_events_csv(&conn, "empty", None, None, None, &mut output)
            .unwrap();
        let csv = String::from_utf8(output).unwrap();
        // Bounded header: one `data` column instead of one column per key.
        assert_eq!(csv.lines().next().unwrap(), "id,timestamp,duration,data");
        assert_eq!(csv.lines().count(), MAX_CSV_DATA_COLUMNS + 2, "{csv}");
        // No keys are dropped: every event's data object survives as JSON.
        assert!(csv.contains("k0"), "first key lost: {csv}");
        assert!(csv.contains("k32"), "last key lost: {csv}");
    }

    #[test]
    fn streamed_csv_terminates_every_record_with_crlf() {
        let (conn, mut ds) = setup();
        let event = Event::new(
            DateTime::from_timestamp(0, 0).unwrap(),
            Duration::seconds(1),
            serde_json::from_value(serde_json::json!({"app": "firefox"})).unwrap(),
        );
        ds.insert_events(&conn, "empty", vec![event]).unwrap();

        let mut output = Vec::new();
        ds.write_events_csv(&conn, "empty", None, None, None, &mut output)
            .unwrap();
        let csv = String::from_utf8(output).unwrap();
        // RFC-4180: header + one event row, each CRLF-terminated.
        assert!(csv.ends_with("\r\n"), "{csv:?}");
        assert_eq!(csv.matches("\r\n").count(), 2, "{csv:?}");
    }

    #[test]
    fn streamed_csv_stops_key_pass_at_the_cap_without_truncating_rows() {
        let (conn, mut ds) = setup();
        // Far more distinct keys than the cap: the key pre-pass must stop
        // collecting once the fallback is decided instead of holding every key.
        let count = 1_000;
        let events: Vec<Event> = (0..count)
            .map(|i| {
                let mut data = serde_json::Map::new();
                data.insert(format!("k{i}"), serde_json::json!(i));
                Event::new(
                    DateTime::from_timestamp(i as i64, 0).unwrap(),
                    Duration::seconds(1),
                    data,
                )
            })
            .collect();
        ds.insert_events(&conn, "empty", events).unwrap();

        let mut output = Vec::new();
        ds.write_events_csv(&conn, "empty", None, None, None, &mut output)
            .unwrap();
        let csv = String::from_utf8(output).unwrap();
        assert_eq!(csv.lines().next().unwrap(), "id,timestamp,duration,data");
        // Leaving the key pre-pass early must not truncate the data pass.
        assert_eq!(csv.lines().count(), count as usize + 1, "{csv}");
    }

    #[test]
    fn csv_flush_errors_propagate() {
        struct FlushFailingWriter;
        impl Write for FlushFailingWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "disk full",
                ))
            }
        }
        let (conn, mut ds) = setup();
        let event = Event::new(
            DateTime::from_timestamp(0, 0).unwrap(),
            Duration::nanoseconds(1_500_000),
            serde_json::from_value(serde_json::json!({"app": "firefox"})).unwrap(),
        );
        ds.insert_events(&conn, "empty", vec![event]).unwrap();
        assert!(matches!(
            ds.write_events_csv(&conn, "empty", None, None, None, FlushFailingWriter),
            Err(DatastoreError::InternalError(msg)) if msg.contains("disk full")
        ));
    }

    #[test]
    fn streamed_csv_preserves_precision_neutralizes_formulas_and_rejects_missing() {
        let (conn, mut ds) = setup();
        let event = Event::new(
            DateTime::from_timestamp(0, 0).unwrap(),
            Duration::nanoseconds(1_500_000),
            serde_json::from_value(serde_json::json!({
                "app": "firefox",
                "title": "=cmd|calc"
            }))
            .unwrap(),
        );
        ds.insert_events(&conn, "empty", vec![event]).unwrap();

        let mut output = Vec::new();
        ds.write_events_csv(&conn, "empty", None, None, None, &mut output)
            .unwrap();
        let csv = String::from_utf8(output).unwrap();
        assert!(csv.starts_with("id,timestamp,duration,"), "{csv}");
        assert!(csv.contains("0.001500000"), "duration: {csv}");
        assert!(csv.contains("'=cmd|calc"), "formula: {csv}");

        let mut missing = Vec::new();
        assert!(matches!(
            ds.write_events_csv(&conn, "missing", None, None, None, &mut missing),
            Err(DatastoreError::NoSuchBucket(_))
        ));
        assert!(missing.is_empty());
    }

    #[test]
    fn streamed_csv_matches_get_events_row_count() {
        let (conn, mut ds) = setup();
        let events = ds.get_events(&conn, "populated", None, None, None).unwrap();
        let mut output = Vec::new();
        ds.write_events_csv(&conn, "populated", None, None, None, &mut output)
            .unwrap();
        let csv = String::from_utf8(output).unwrap();
        assert_eq!(csv.lines().count(), events.len() + 1, "{csv}");
        assert!(csv.contains("\"quotes \"\" and unicode ☀\""), "{csv}");
    }
}
