//! Reproducible export-memory comparison using synthetic, in-memory data only.
//! Build with `cargo build -p aw-datastore --example export_memory --release`.
//! Run the resulting executable under a memory profiler with one of:
//!
//! - `stream 100000` — JSON export, incremental (`write_export`)
//! - `materialized 100000` — JSON export, pre-#677 in-memory baseline
//! - `csv 100000` — CSV export, row-at-a-time (`write_events_csv`)
//! - `csv-union 100000` — CSV export where every event carries a distinct data
//!   key, exercising the `MAX_CSV_DATA_COLUMNS` fallback
//!
//! No existing database or server is opened.
use aw_datastore::DatastoreInstance;
use aw_models::{Bucket, BucketMetadata, BucketsExport, TryVec};
use chrono::Utc;
use rusqlite::Connection;
use std::{hint::black_box, time::Instant};

/// Discards written bytes while counting them, so a single run reports both the
/// serialization cost and the size of the produced document.
#[derive(Default)]
struct CountingWriter(u64);

impl std::io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "stream".into());
    assert!(matches!(
        mode.as_str(),
        "stream" | "materialized" | "csv" | "csv-union"
    ));
    let count: u32 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "100000".into())
        .parse()
        .unwrap();
    let conn = Connection::open_in_memory().unwrap();
    let mut ds = DatastoreInstance::new(&conn, true).unwrap();
    ds.create_bucket(
        &conn,
        Bucket {
            bid: None,
            id: "synthetic".into(),
            _type: "test".into(),
            client: "test".into(),
            hostname: "test".into(),
            created: Some(Utc::now()),
            data: Default::default(),
            metadata: BucketMetadata::default(),
            events: None,
            last_updated: None,
        },
    )
    .unwrap();
    if mode == "csv-union" {
        // Every event gets its own data key, so the union of keys is `count`
        // and the spreadsheet-style column layout is abandoned for the single
        // JSON `data` column.
        conn.execute(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<?1)
            INSERT INTO events(bucketrow,starttime,endtime,data)
            SELECT 1,n*1000000000,n*1000000000+500000000,json_object('key' || n, n) FROM seq",
            rusqlite::params![count],
        )
        .unwrap();
    } else {
        let payload = serde_json::json!({"app": "browser", "title": "x".repeat(256)}).to_string();
        conn.execute(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<?1)
            INSERT INTO events(bucketrow,starttime,endtime,data)
            SELECT 1,n*1000000000,n*1000000000+500000000,?2 FROM seq",
            rusqlite::params![count, payload],
        )
        .unwrap();
    }
    let mut ds = DatastoreInstance::new(&conn, true).unwrap();
    let start = Instant::now();
    match mode.as_str() {
        "csv" | "csv-union" => {
            let mut out = CountingWriter::default();
            ds.write_events_csv(&conn, "synthetic", None, None, None, &mut out)
                .unwrap();
            println!(
                "{mode}: {count} synthetic events, {:?}, {} bytes of CSV",
                start.elapsed(),
                out.0
            );
        }
        "stream" => {
            ds.write_export(&conn, None, std::io::sink()).unwrap();
            println!("{mode}: {count} synthetic events, {:?}", start.elapsed());
        }
        "materialized" => {
            let mut buckets = ds.get_buckets();
            for (id, bucket) in &mut buckets {
                bucket.events = Some(TryVec::new(
                    ds.get_events(&conn, id, None, None, None).unwrap(),
                ));
            }
            let export = BucketsExport { buckets };
            let body = serde_json::to_string(&export).unwrap();
            black_box((&export, &body));
            println!("{mode}: {count} synthetic events, {:?}", start.elapsed());
        }
        _ => unreachable!(),
    }
}
