extern crate rusqlite;
extern crate chrono;

use std::sync::Mutex;

use rusqlite::Connection;
use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::Utc;

use super::models::bucket::Bucket;
use super::models::event::Event;
use super::models::duration::Duration;

/*
 * TODO:
 * - Optimize with bucket cache (for IDs and get)
 * - Optimize witht transactions
 * - Do not unwrap Mutex in case it get poisoned
 */

pub struct DatastoreInstance {
    conn: Mutex<Connection>,
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
        return DatastoreInstance {
            conn: Mutex::new(conn)
        }
    }

    pub fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().unwrap();
        _create_tables(&conn);
        return DatastoreInstance {
            conn: Mutex::new(conn)
        }
    }

    pub fn create_bucket(&self, bucket: &Bucket) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let res = conn.execute("
            INSERT INTO buckets (name, type, client, hostname, created)
            VALUES (?1, ?2, ?3, ?4, ?5)",
            &[&bucket.id, &bucket._type, &bucket.client, &bucket.hostname, &bucket.created]);

        match res {
            Ok(_) => return Ok(true),
            Err(e) => match e {
                rusqlite::Error::SqliteFailure { 0: sqlerr, 1: _} => match sqlerr.code {
                    rusqlite::ErrorCode::ConstraintViolation => Ok(false),
                    _ => Err(e)
                },
                _ => Err(e)
            }
        }
    }

    pub  fn delete_bucket(&self, bucket_id: &str) -> Result<(), rusqlite::Error>{
        let conn = self.conn.lock().unwrap();
        match conn.execute("DELETE FROM buckets WHERE name = ?1", &[&bucket_id]) {
            Ok(_) => Ok(()),
            Err(err) => Err(err)
        }
    }

    pub fn get_bucket(&self, bucket_id: &str) -> Result<Option<Bucket>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let res = conn.query_row("SELECT name, type, client, hostname, created FROM buckets WHERE name = ?1", &[&bucket_id], |row| {
            Some(Bucket {
                id: row.get(0),
                _type: row.get(1),
                client: row.get(2),
                hostname: row.get(3),
                created: row.get(4),
            })
        });
        match res {
            Ok(b) => Ok(Some(b.unwrap())),
            Err(e) => match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                unknown_err => Err(unknown_err),
            }
        }
    }

    pub fn get_buckets(&self) -> Result<Vec<Bucket>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut list = Vec::new();
        let mut stmt = try!(conn.prepare("SELECT name, type, client, hostname, created FROM buckets"));
        let buckets = try!(stmt.query_map(&[], |row| {
            Bucket {
                id: row.get(0),
                _type: row.get(1),
                client: row.get(2),
                hostname: row.get(3),
                created: row.get(4),
            }
        }));
        for bucket in buckets {
            list.push(bucket.unwrap());
        }
        Ok(list)
    }

    pub fn insert_events(&self, bucket_id: &str, events: &Vec<Event>) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = try!(conn.prepare("
            INSERT INTO events(bucketrow, starttime, endtime, data)
            VALUES ((SELECT id FROM buckets WHERE name = ?1), ?2, ?3, ?4)"));
        for event in events {
            let starttime_nanos : i64 = event.timestamp.timestamp_nanos();
            let duration_nanos : i64 = event.duration.num_nanos() as i64;
            let endtime_nanos : i64 = starttime_nanos + duration_nanos;
            if endtime_nanos < starttime_nanos {
                println!("SHIT!");
            }
            try!(stmt.execute(&[&bucket_id, &starttime_nanos, &endtime_nanos, &event.data]));
        }
        Ok(())
    }

    pub fn replace_last_event(&self, bucket_id: &str, event: &Event) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        /* TODO: Optimize this query*/
        let mut stmt = try!(conn.prepare("
            UPDATE events
            SET starttime = ?2, endtime = ?3, data = ?4
            WHERE bucketrow = (SELECT id FROM buckets WHERE name = ?1)
                AND endtime = (SELECT max(endtime) FROM events WHERE bucketrow = (SELECT id FROM buckets WHERE name = ?1))"));
        let starttime_nanos : i64 = event.timestamp.timestamp_nanos();
        let duration_nanos : i64 = event.duration.num_nanos() as i64;
        let endtime_nanos : i64 = starttime_nanos + duration_nanos;
        if endtime_nanos < starttime_nanos {
            println!("SHIT!");
        }
        try!(stmt.execute(&[&bucket_id, &starttime_nanos, &endtime_nanos, &event.data]));
        Ok(())
    }

    pub fn get_events(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>, limit_opt: Option<u64>) -> Result<Vec<Event>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut list = Vec::new();

        let starttime_filter_ns = match starttime_opt {
            Some(dt) => dt.timestamp_nanos() as i64,
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

        let mut stmt = try!(conn.prepare("SELECT id, starttime, endtime, data FROM events WHERE bucketrow = (SELECT id FROM buckets WHERE name = ?1) AND endtime >= ?2 AND starttime <= ?3 ORDER BY starttime DESC LIMIT ?4;"));
        let rows = try!(stmt.query_map(&[&bucket_id, &starttime_filter_ns, &endtime_filter_ns, &limit], |row| {
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
                duration: Duration::from_nanos(duration_ns),
                data: data,
            }
        }));
        for row in rows {
            list.push(row.unwrap());
        }
        Ok(list)
    }

    pub fn get_events_count(&self, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>) -> Result<i64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
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
        let ret : i64 = try!(conn.query_row("SELECT count(*) FROM events WHERE bucketrow = (SELECT id FROM buckets WHERE name = ?1) AND (starttime >= ?2 OR endtime <= ?3)", &[&bucket_id, &starttime_filter_ns, &endtime_filter_ns], |row| row.get(0)));
        return Ok(ret);
    }
}
