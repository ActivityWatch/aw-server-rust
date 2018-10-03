extern crate rusqlite;
extern crate chrono;

use rusqlite::Connection;
use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::Utc;
use std::convert::TryInto;

use super::models::bucket::Bucket;
use super::models::event::Event;
use super::models::duration::Duration;

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

pub fn setup(dbpath: String) -> Connection {
    let conn = Connection::open(dbpath).unwrap();
    _create_tables(&conn);
    return conn;
}

pub fn setup_memory() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    _create_tables(&conn);
    return conn;
}

pub fn create_bucket(conn: &Connection, bucket: &Bucket) -> Result<(), rusqlite::Error> {
    /*
    match get_bucket(conn, &bucket.id) {
        Ok(_) => return Err(),
        Err(_) => ()
    }
    */
    try!(conn.execute("
        INSERT INTO buckets (name, type, client, hostname, created)
        VALUES (?1, ?2, ?3, ?4, ?5)",
        &[&bucket.id, &bucket._type, &bucket.client, &bucket.hostname, &bucket.created]));
    Ok(())
}

pub  fn delete_bucket(conn: &Connection, bucket_id: &str) -> Result<(), rusqlite::Error>{
    match conn.execute("DELETE FROM buckets WHERE name = ?1", &[&bucket_id]) {
        Ok(_) => Ok(()),
        Err(err) => Err(err)
    }
}

pub fn get_bucket(conn: &Connection, bucket_id: &str) -> Result<Bucket, rusqlite::Error> {
    conn.query_row("SELECT name, type, client, hostname, created FROM buckets WHERE name = ?1", &[&bucket_id], |row| {
        Bucket {
            id: row.get(0),
            _type: row.get(1),
            client: row.get(2),
            hostname: row.get(3),
            created: row.get(4),
        }
    })
}

pub fn get_buckets(conn: &Connection) -> Result<Vec<Bucket>, rusqlite::Error> {
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

pub fn insert_events(conn: &Connection, bucket_id: &str, events: &Vec<Event>) -> Result<(), rusqlite::Error> {
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

pub fn replace_last_event(conn: &Connection, bucket_id: &str, event: &Event) -> Result<(), rusqlite::Error> {
    let mut stmt = try!(conn.prepare("
        UPDATE events
        SET starttime = ?2, endtime = ?3, data = ?4
        WHERE bucketrow = (SELECT id FROM buckets WHERE name = ?1)
            AND endtime = (SELECT max(endtime) FROM events)"));
    let starttime_nanos : i64 = event.timestamp.timestamp_nanos();
    let duration_nanos : i64 = event.duration.num_nanos() as i64;
    let endtime_nanos : i64 = starttime_nanos + duration_nanos;
    if endtime_nanos < starttime_nanos {
        println!("SHIT!");
    }
    try!(stmt.execute(&[&bucket_id, &starttime_nanos, &endtime_nanos, &event.data]));
    Ok(())
}

pub fn get_events(conn: &Connection, bucket_id: &str, starttime_opt: Option<DateTime<Utc>>, endtime_opt: Option<DateTime<Utc>>, limit_opt: Option<u64>) -> Result<Vec<Event>, rusqlite::Error> {
    let mut list = Vec::new();
    let starttime_ns = match starttime_opt {
        Some(dt) => dt.timestamp_nanos() as i64,
        None => 0
    };
    let endtime_ns = match endtime_opt {
        Some(dt) => dt.timestamp_nanos() as i64,
        None => std::i64::MAX
    };
    if starttime_ns >= endtime_ns {
        println!("Endtime in event query was same or lower than starttime!");
        return Ok(list);
    }
    let limit = match limit_opt {
        Some(l) => l as i64,
        None => -1
    };
    let mut stmt = try!(conn.prepare("SELECT id, starttime, endtime, data FROM events WHERE bucketrow = (SELECT id FROM buckets WHERE name = ?1) AND (starttime >= ?2 OR endtime <= ?3) ORDER BY starttime ASC LIMIT ?4;"));
    let rows = try!(stmt.query_map(&[&bucket_id, &starttime_ns, &endtime_ns, &limit], |row| {
        let mut starttime : i64 = row.get(1);
        if starttime < starttime_ns {
            starttime = starttime_ns;
        }
        let mut endtime : i64 = row.get(2);
        if endtime > endtime_ns {
            endtime = endtime_ns;
        }
        let mut duration_ns : u64 = 0;
        if endtime < starttime {
            println!("Endtime is lower than starttime! start:{} end:{}", starttime, endtime);
        } else {
            duration_ns = (endtime - starttime).try_into().unwrap();
        }
        let time_seconds : i64 = (starttime/1000000000).try_into().unwrap();
        let time_nanos : u32 = (starttime%1000000000).try_into().unwrap();
        return Event {
            id: Some(row.get(0)),
            timestamp: DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(time_seconds, time_nanos), Utc),
            duration: Duration::from_nanos(duration_ns),
            data: row.get(3),
        }
    }));
    for row in rows {
        list.push(row.unwrap());
    }
    Ok(list)
}
