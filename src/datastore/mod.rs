extern crate rusqlite;
extern crate chrono;

use std::collections::HashMap;

use rusqlite::Connection;
use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::Utc;
use chrono::Duration;

use super::models::bucket::Bucket;
use super::models::event::Event;

/*
 * TODO:
 * - Optimize with transactions
 */

#[derive(Debug)]
pub enum DatastoreError {
    NoSuchBucket,
    BucketAlreadyExists,
    InternalError
}

pub struct DatastoreInstance {
    conn: Connection,
    buckets_cache: HashMap<String, Bucket>
}

fn _create_tables(conn: &Connection) {
    /* Set up bucket table and index */
    conn.execute("
        CREATE TABLE IF NOT EXISTS buckets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            type TEXT NOT NULL,
            client TEXT NOT NULL,
            hostname TEXT NOT NULL,
            created TEXT NOT NULL
        )", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS bucket_id_index ON buckets(id)", &[]).unwrap();

    conn.execute("
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bucketrow INTEGER NOT NULL,
            starttime INTEGER NOT NULL,
            endtime INTEGER NOT NULL,
            data TEXT NOT NULL,
            FOREIGN KEY (bucketrow) REFERENCES buckets(id)
        )", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS events_bucketrow_index ON events(bucketrow)", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS events_starttime_index ON events(starttime)", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS events_endtime_index ON events(endtime)", &[]).unwrap();
}

impl DatastoreInstance {

    pub fn new(dbpath: String) -> Self {
        let conn = Connection::open(dbpath).unwrap();
        _create_tables(&conn);
        let mut ds = DatastoreInstance {
            conn: conn,
            buckets_cache: HashMap::new()
        };
        ds.get_stored_buckets();
        return ds;
    }

    fn get_stored_buckets(&mut self) {
        let mut stmt = self.conn.prepare("SELECT id, name, type, client, hostname, created FROM buckets").unwrap();
        let buckets = stmt.query_map(&[], |row| {
            Bucket {
                bid: row.get(0),
                id: row.get(1),
                _type: row.get(2),
                client: row.get(3),
                hostname: row.get(4),
                created: row.get(5),
            }
        }).unwrap();
        for bucket in buckets {
            match bucket {
                Ok(b) => {self.buckets_cache.insert(b.id.clone(), b.clone());},
                Err(e) => {
                    println!("Failed to parse bucket from SQLite, database is corrupt!");
                    println!("{}", e);
                }
            }
        };
        ()
    }

    pub fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().unwrap();
        _create_tables(&conn);
        return DatastoreInstance {
            conn: conn,
            buckets_cache: HashMap::new()
        }
    }

    pub fn create_bucket(&mut self, bucket: &Bucket) -> Result<(), DatastoreError> {
        let res = self.conn.execute("
            INSERT INTO buckets (name, type, client, hostname, created)
            VALUES (?1, ?2, ?3, ?4, ?5)",
            &[&bucket.id, &bucket._type, &bucket.client, &bucket.hostname, &bucket.created]);

        match res {
            Ok(_) => {
                let rowid = self.conn.last_insert_rowid();
                let mut inserted_bucket = bucket.clone();
                inserted_bucket.bid = Some(rowid);
                self.buckets_cache.insert(bucket.id.clone(), inserted_bucket);
                return Ok(())
            },
            // FIXME: This match is ugly, is it possible to write it in a cleaner way?
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _} => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => Err(DatastoreError::BucketAlreadyExists),
                    _ => { println!("{}", err); return Err(DatastoreError::InternalError) }
                },
                _ => { println!("{}", err); return Err(DatastoreError::InternalError) }
            }
        }
    }

    pub  fn delete_bucket(&mut self, bucket_id: &str) -> Result<(), DatastoreError>{
        let bucket = try!(self.get_bucket(&bucket_id));
        // Delete all events in bucket
        match self.conn.execute("DELETE FROM events WHERE id = ?1", &[&bucket.bid]) {
            Ok(_) => (),
            Err(err) => { println!("{}", err); return Err(DatastoreError::InternalError) }
        }
        // Delete bucket itself
        match self.conn.execute("DELETE FROM buckets WHERE name = ?1", &[&bucket.bid]) {
            Ok(_) => {
                self.buckets_cache.remove(bucket_id);
                return Ok(());
            },
            Err(err) => match err {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _} => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => Err(DatastoreError::BucketAlreadyExists),
                    _ => { println!("{}", err); return Err(DatastoreError::InternalError) }
                },
                _ => { println!("{}", err); return Err(DatastoreError::InternalError) }
            }
        }
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Bucket, DatastoreError> {
        let cached_bucket = self.buckets_cache.get(bucket_id);
        match cached_bucket {
            Some(bucket) => Ok(bucket.clone()),
            None => Err(DatastoreError::NoSuchBucket)
        }
    }

    pub fn get_buckets(&self) -> Result<HashMap<String, Bucket>, DatastoreError> {
        return Ok(self.buckets_cache.clone());
    }

    pub fn insert_events(&self, bucket_id: &str, events: &Vec<Event>) -> Result<(), DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

        let mut stmt = self.conn.prepare("
            INSERT INTO events(bucketrow, starttime, endtime, data)
            VALUES (?1, ?2, ?3, ?4)").unwrap();
        for event in events {
            let starttime_nanos = event.timestamp.timestamp_nanos();
            let duration_nanos = event.duration.num_nanoseconds().unwrap();
            let endtime_nanos = starttime_nanos + duration_nanos;
            let res = stmt.execute(&[&bucket.bid.unwrap(), &starttime_nanos, &endtime_nanos, &event.data]);
            match res {
                Ok(_) => (),
                Err(e) => {
                    println!("Failed to insert event: {}", e);
                    println!("{:?}", event);
                    return Err(DatastoreError::InternalError);
                }
            }
        }
        Ok(())
    }

    pub fn replace_last_event(&self, bucket_id: &str, event: &Event) -> Result<(), DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

        let mut stmt = self.conn.prepare("
            UPDATE events
            SET starttime = ?2, endtime = ?3, data = ?4
            WHERE bucketrow = ?1
                AND endtime = (SELECT max(endtime) FROM events WHERE bucketrow = ?1)
        ").unwrap();
        let starttime_nanos = event.timestamp.timestamp_nanos();
        let duration_nanos = event.duration.num_nanoseconds().unwrap();
        let endtime_nanos = starttime_nanos + duration_nanos;
        stmt.execute(&[&bucket.bid.unwrap(), &starttime_nanos, &endtime_nanos, &event.data]).unwrap();
        Ok(())
    }

    pub fn get_events(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>, limit_opt: Option<u64>) -> Result<Vec<Event>, DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

        let mut list = Vec::new();

        let starttime_filter_ns : i64 = match starttime_opt {
            Some(dt) => dt.timestamp_nanos(),
            None => 0
        };
        let endtime_filter_ns = match endtime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => std::i64::MAX
        };
        if starttime_filter_ns > endtime_filter_ns {
            println!("Starttime in event query was lower than endtime!");
            return Ok(list);
        }
        let limit = match limit_opt {
            Some(l) => l as i64,
            None => -1
        };

        let mut stmt = self.conn.prepare("
            SELECT id, starttime, endtime, data
            FROM events
            WHERE bucketrow = ?1
                AND endtime >= ?2
                AND starttime <= ?3
            ORDER BY starttime DESC
            LIMIT ?4
        ;").unwrap();

        let rows = stmt.query_map(&[&bucket.bid.unwrap(), &starttime_filter_ns, &endtime_filter_ns, &limit], |row| {
            let id = row.get(0);
            let mut starttime_ns : i64 = row.get(1);
            let mut endtime_ns : i64 = row.get(2);
            let data = row.get(3);

            if starttime_ns < starttime_filter_ns { starttime_ns = starttime_filter_ns }
            if endtime_ns > endtime_filter_ns { endtime_ns = endtime_filter_ns }
            let duration_ns = endtime_ns - starttime_ns;

            let time_seconds : i64 = (starttime_ns/1000000000) as i64;
            let time_subnanos : u32 = (starttime_ns%1000000000) as u32;

            return Event {
                id: Some(id),
                timestamp: DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(time_seconds, time_subnanos), Utc),
                duration: Duration::nanoseconds(duration_ns),
                data: data,
            }
        }).unwrap();
        for row in rows {
            list.push(row.unwrap());
        }
        Ok(list)
    }

    pub fn get_events_count(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>) -> Result<i64, DatastoreError> {
        let bucket = try!(self.get_bucket(&bucket_id));

        let starttime_filter_ns = match starttime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => 0
        };
        let endtime_filter_ns = match endtime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
            None => std::i64::MAX
        };
        if starttime_filter_ns >= endtime_filter_ns {
            println!("Endtime in event query was same or lower than starttime!");
            return Ok(0);
        }
        let ret : i64 = self.conn.query_row("
            SELECT count(*) FROM events
            WHERE bucketrow = ?1
                AND (starttime >= ?2 OR endtime <= ?3)",
            &[&bucket.bid.unwrap(), &starttime_filter_ns, &endtime_filter_ns],
            |row| row.get(0)
        ).unwrap();
        return Ok(ret);
    }
}
