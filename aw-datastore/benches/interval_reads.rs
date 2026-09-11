use aw_datastore::DatastoreInstance;
use aw_models::{Bucket, BucketMetadata};
use chrono::{DateTime, Utc};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use rusqlite::Connection;

fn interval_reads(c: &mut Criterion) {
    let conn = Connection::open_in_memory().unwrap();
    let mut ds = DatastoreInstance::new(&conn, true).unwrap();
    ds.create_bucket(
        &conn,
        Bucket {
            bid: None,
            id: "bench".into(),
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
    conn.execute_batch(
        "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<1000000)
        INSERT INTO events(bucketrow, starttime, endtime, data)
        SELECT 1, n*1000000000, n*1000000000+500000000, '{\"app\":\"browser\"}' FROM seq;",
    )
    .unwrap();
    let mut ds = DatastoreInstance::new(&conn, true).unwrap();
    let mut group = c.benchmark_group("interval_reads");
    for (name, start, end) in [
        ("recent", 999_000, 1_000_000),
        ("middle", 500_000, 501_000),
        ("early", 0, 1_000),
    ] {
        let start = Some(DateTime::from_timestamp(start, 0).unwrap());
        let end = Some(DateTime::from_timestamp(end, 0).unwrap());
        group.bench_function(name, |b| {
            b.iter(|| ds.get_events(&conn, "bench", start, end, None).unwrap())
        });
    }
    group.bench_function("latest_one", |b| {
        b.iter(|| ds.get_events(&conn, "bench", None, None, Some(1)).unwrap())
    });
    let start = Some(DateTime::from_timestamp(999_000, 0).unwrap());
    group.bench_function("recent_count", |b| {
        b.iter(|| ds.get_event_count(&conn, "bench", start, None).unwrap())
    });
    group.finish();
}

fn index_write_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_write_cost");
    for second_index in [false, true] {
        group.bench_function(if second_index { "two_indexes" } else { "one_index" }, |b| {
            b.iter_batched(|| {
                let conn = Connection::open_in_memory().unwrap();
                conn.execute_batch("CREATE TABLE events(id INTEGER PRIMARY KEY AUTOINCREMENT,
                    bucketrow INTEGER NOT NULL, starttime INTEGER NOT NULL, endtime INTEGER NOT NULL, data TEXT NOT NULL);
                    CREATE INDEX by_start ON events(bucketrow, starttime DESC, endtime);").unwrap();
                if second_index {
                    conn.execute_batch("CREATE INDEX by_end ON events(bucketrow, endtime, starttime DESC);").unwrap();
                }
                conn
            }, |conn| {
                conn.execute_batch("WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<10000)
                    INSERT INTO events(bucketrow,starttime,endtime,data)
                    SELECT 1,n*1000000000,n*1000000000+500000000,'{}' FROM seq;").unwrap();
                conn
            }, BatchSize::SmallInput);
        });
    }
    group.finish();
}

criterion_group!(benches, interval_reads, index_write_cost);
criterion_main!(benches);
