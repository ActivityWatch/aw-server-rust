use crate::datastore::DatastoreInstance;
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub enum LegacyDatastoreImportError {
    SQLPrepareError(String),
    SQLMapError(String),
}

pub fn legacy_import(
    new_ds: &mut DatastoreInstance,
    new_conn: &Connection,
) -> Result<(), LegacyDatastoreImportError> {
    import::legacy_import(new_ds, new_conn)
}

#[cfg(not(target_os = "android"))]
mod import {
    use std::path::PathBuf;

    use rusqlite::types::ToSql;
    use rusqlite::Connection;

    use chrono::DateTime;
    use chrono::Duration;
    use chrono::Utc;

    use aw_models::Bucket;
    use aw_models::BucketMetadata;
    use aw_models::Event;

    use crate::datastore::DatastoreInstance;

    use super::LegacyDatastoreImportError;

    fn dbfile_path() -> PathBuf {
        let mut dir = appdirs::user_data_dir(Some("activitywatch"), None, false).unwrap();
        dir.push("aw-server");
        dir.push("peewee-sqlite.v2.db");
        dir
    }

    fn get_legacy_buckets(conn: &Connection) -> Result<Vec<Bucket>, LegacyDatastoreImportError> {
        let mut stmt = match conn
            .prepare("SELECT key, id, type, client, hostname, created FROM bucketmodel")
        {
            Ok(stmt) => stmt,
            Err(err) => return Err(LegacyDatastoreImportError::SQLPrepareError(err.to_string())),
        };
        let bucket_rows = match stmt.query_map(&[] as &[&dyn ToSql], |row| {
            Ok(Bucket {
                bid: row.get(0)?,
                id: row.get(1)?,
                _type: row.get(2)?,
                client: row.get(3)?,
                hostname: row.get(4)?,
                created: row.get(5)?,
                data: json_map! {},
                events: None,
                last_updated: None,
                metadata: BucketMetadata {
                    start: None,
                    end: None,
                },
            })
        }) {
            Ok(buckets) => buckets,
            Err(err) => {
                return Err(LegacyDatastoreImportError::SQLMapError(format!(
                    "Failed to query get_legacy_buckets SQL statement: {:?}",
                    err
                )))
            }
        };

        let mut buckets = Vec::new();
        for bucket_res in bucket_rows {
            match bucket_res {
                Ok(bucket) => buckets.push(bucket),
                Err(err) => panic!("{:?}", err),
            }
        }
        Ok(buckets)
    }

    fn get_legacy_events(
        conn: &Connection,
        bucket_id: &i64,
    ) -> Result<Vec<Event>, LegacyDatastoreImportError> {
        let mut stmt = match conn.prepare(
            "
                SELECT timestamp, duration, datastr
                FROM eventmodel
                WHERE bucket_id = ?1
                ORDER BY timestamp DESC
            ;",
        ) {
            Ok(stmt) => stmt,
            Err(err) => return Err(LegacyDatastoreImportError::SQLPrepareError(err.to_string())),
        };

        let rows = match stmt.query_map(&[&bucket_id], |row| {
            let timestamp_str: String = row.get(0)?;
            let duration_float: f64 = row.get(1)?;
            let data_str: String = row.get(2)?;

            let timestamp_str = timestamp_str.replace(" ", "T");
            let timestamp = match DateTime::parse_from_rfc3339(&timestamp_str) {
                Ok(timestamp) => timestamp.with_timezone(&Utc),
                Err(err) => panic!("Timestamp string {}: {:?}", timestamp_str, err),
            };

            let duration_ns = (duration_float * 1_000_000_000.0) as i64;

            let data: serde_json::map::Map<String, serde_json::Value> =
                match serde_json::from_str(&data_str) {
                    Ok(data) => data,
                    Err(err) => panic!(
                        "Unable to parse JSON data in event from bucket {}\n{}\n{}",
                        bucket_id, err, data_str
                    ),
                };

            return Ok(Event {
                id: None,
                timestamp: timestamp,
                duration: Duration::nanoseconds(duration_ns),
                data: data,
            });
        }) {
            Ok(rows) => rows,
            Err(err) => {
                return Err(LegacyDatastoreImportError::SQLMapError(format!(
                    "Failed to query get_legacy_events SQL statement: {:?}",
                    err
                )))
            }
        };
        let mut list = Vec::new();
        for row in rows {
            match row {
                Ok(event) => list.push(event),
                Err(err) => panic!("Corrupt event in bucket {}: {}", bucket_id, err),
            };
        }
        Ok(list)
    }

    pub fn legacy_import(
        new_ds: &mut DatastoreInstance,
        new_conn: &Connection,
    ) -> Result<(), LegacyDatastoreImportError> {
        let legacy_db_path = dbfile_path();
        if !legacy_db_path.exists() {
            return Ok(());
        }
        info!("Importing legacy DB");
        let legacy_conn =
            Connection::open(legacy_db_path).expect("Unable to open corrupt legacy db file");

        let buckets = get_legacy_buckets(&legacy_conn)?;
        for bucket in &buckets {
            println!("Importing legacy bucket: {:?}", bucket);
            match new_ds.create_bucket(new_conn, bucket.clone()) {
                Ok(_) => (),
                Err(err) => panic!("Failed to create bucket '{}': {:?}", bucket.id, err),
            };
            let events = get_legacy_events(&legacy_conn, &bucket.bid.unwrap())?;
            let num_events = events.len(); // Save len before lending events to insert_events
            println!("Importing {} events for {}", num_events, bucket.id);
            match new_ds.insert_events(new_conn, &bucket.id, events) {
                Ok(_) => (),
                Err(err) => panic!(
                    "Failed to insert events to bucket '{}': {:?}",
                    bucket.id, err
                ),
            };
            //assert_eq!(new_ds.get_events(new_conn, &bucket.id, None, None, None).unwrap().len(), num_events);
        }
        assert_eq!(new_ds.get_buckets().len(), buckets.len());
        Ok(())
    }

    /* This test is disabled because it requires manual set-up of a old aw-server database
     * Can be run with:
     * cargo test --features legacy_import,legacy_import_tests */
    #[test]
    #[cfg_attr(not(feature = "legacy_import_tests"), ignore)]
    fn test_legacy_import() {
        assert!(dbfile_path().exists());
        let mut new_conn =
            Connection::open_in_memory().expect("Unable to open corrupt legacy db file");
        let mut ds = DatastoreInstance::new(&mut new_conn, true).unwrap();
        assert!(ds.ensure_legacy_import(&new_conn).unwrap(), true);
        let buckets = ds.get_buckets();
        assert!(buckets.len() > 0);
        let mut num_events = 0;
        for (bucket_id, _bucket) in buckets {
            let events = ds
                .get_events(&new_conn, &bucket_id, None, None, Some(1000))
                .unwrap();
            num_events += events.len();
        }
        assert!(num_events > 0);
    }
}

#[cfg(target_os = "android")]
mod import {
    use super::LegacyDatastoreImportError;
    use crate::datastore::DatastoreInstance;
    use rusqlite::Connection;

    pub fn legacy_import(
        _new_ds: &mut DatastoreInstance,
        _new_conn: &Connection,
    ) -> Result<(), LegacyDatastoreImportError> {
        Ok(())
    }
}
