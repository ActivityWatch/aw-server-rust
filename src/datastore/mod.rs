extern crate rusqlite;
extern crate chrono;

use rusqlite::Connection;

use super::models::Bucket;

fn _create_tables(conn: &Connection) {
    /* Set up bucket table and index */
    conn.execute("
        CREATE TABLE IF NOT EXISTS buckets (
            rowid INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT UNIQUE NOT NULL,
            type TEXT NOT NULL,
            client TEXT NOT NULL,
            hostname TEXT NOT NULL,
            created TEXT NOT NULL
        )", &[]).unwrap();
    conn.execute("CREATE INDEX IF NOT EXISTS bucket_id_index ON buckets(id)", &[]).unwrap();
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

pub fn create_bucket(conn: &Connection, bucket: &Bucket) {
    conn.execute("INSERT INTO buckets (id, type, client, hostname, created)
                  VALUES (?1, ?2, ?3, ?4, ?5)",
                 &[&bucket.id, &bucket._type, &bucket.client, &bucket.hostname, &bucket.created]).unwrap();
}

pub  fn delete_bucket(conn: &Connection, bucket_id: &str) -> Result<(), rusqlite::Error>{
    match conn.execute("DELETE FROM buckets WHERE id = ?1", &[&bucket_id]) {
        Ok(_) => Ok(()),
        Err(err) => Err(err)
    }
}

pub fn get_bucket(conn: &Connection, bucketid: &str) -> Result<Bucket, rusqlite::Error> {
    conn.query_row("SELECT id, type, client, hostname, created FROM buckets WHERE id = ?1", &[&bucketid], |row| {
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
    let mut stmt = conn.prepare("SELECT id, type, client, hostname, created FROM buckets").unwrap();
    let mut list = Vec::new();
    let buckets = stmt.query_map(&[], |row| {
        Bucket {
            id: row.get(0),
            _type: row.get(1),
            client: row.get(2),
            hostname: row.get(3),
            created: row.get(4),
        }
    }).unwrap();
    for bucket in buckets {
        list.push(bucket.unwrap());
    }
    Ok(list)
}
