//! Reproducible export-memory comparison using synthetic, in-memory data only.
//! Build with `cargo build -p aw-datastore --example export_memory --release`.
//! Run the resulting executable under a memory profiler with `stream 100000`
//! or `materialized 100000`. No existing database or server is opened.
use aw_datastore::DatastoreInstance;
use aw_models::{Bucket, BucketMetadata, BucketsExport, TryVec};
use chrono::Utc;
use rusqlite::Connection;
use std::{hint::black_box, time::Instant};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "stream".into());
    assert!(matches!(mode.as_str(), "stream" | "materialized"));
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
    let payload = serde_json::json!({"app": "browser", "title": "x".repeat(256)}).to_string();
    conn.execute(
        "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<?1)
        INSERT INTO events(bucketrow,starttime,endtime,data)
        SELECT 1,n*1000000000,n*1000000000+500000000,?2 FROM seq",
        rusqlite::params![count, payload],
    )
    .unwrap();
    let mut ds = DatastoreInstance::new(&conn, true).unwrap();
    let start = Instant::now();
    if mode == "stream" {
        ds.write_export(&conn, None, std::io::sink()).unwrap();
    } else {
        let mut buckets = ds.get_buckets();
        for (id, bucket) in &mut buckets {
            bucket.events = Some(TryVec::new(
                ds.get_events(&conn, id, None, None, None).unwrap(),
            ));
        }
        let export = BucketsExport { buckets };
        let body = serde_json::to_string(&export).unwrap();
        black_box((&export, &body));
    }
    println!("{mode}: {count} synthetic events, {:?}", start.elapsed());
}
