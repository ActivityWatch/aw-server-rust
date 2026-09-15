use std::{collections::HashMap, io::Write};

use aw_models::Bucket;
use rusqlite::Connection;
use serde::{
    ser::{Error, SerializeMap, SerializeSeq},
    Serialize, Serializer,
};

use crate::DatastoreError;

struct EventRows<'a> {
    conn: &'a Connection,
    bucket: &'a Bucket,
}

impl Serialize for EventRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT id, starttime, endtime, data
             FROM events INDEXED BY events_bucketrow_starttime_endtime_index
             WHERE bucketrow = ?1 AND endtime >= 0 AND starttime <= ?2
             ORDER BY starttime DESC, endtime ASC, id ASC",
            )
            .map_err(S::Error::custom)?;
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
                bucket: self.bucket,
            },
        )?;
        map.end()
    }
}

struct ExportBuckets<'a> {
    conn: &'a Connection,
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
                buckets,
                selected,
            },
        },
    )
    .map_err(|err| DatastoreError::InternalError(format!("Failed to write export: {err}")))
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
}
