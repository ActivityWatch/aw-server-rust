use serde::Serialize;
use serde::Serializer;
use serde::Deserialize;
use serde::Deserializer;

use chrono;

/*
 * FIXME:
 * This datatype is actually only written because I had to, I didn't really want to code it to
 * begin with.
 * What I actually wanted was to use chronos Duration datatype, but since I was not able to extend
 * it to do serde_json serialization/deserialization (because it's in a seperate crate) I wrote a
 * new datatype instead.
 */

pub struct Duration (chrono::Duration);

impl Duration {
    pub fn from_seconds(seconds: f64) -> Duration {
        Duration ( chrono::Duration::nanoseconds((seconds*1000000000f64) as i64) )
    }

    pub fn inner(&self) -> &chrono::Duration {
        &self.0
    }

    pub fn num_seconds(&self) -> f64 {
        (self.0.num_nanoseconds().unwrap() as f64)/1000000000f64
    }

    pub fn from_nanos(nanos: i64) -> Duration {
        Duration ( chrono::Duration::nanoseconds(nanos))
    }

    pub fn num_nanos(&self) -> i64 {
        self.0.num_nanoseconds().unwrap()
    }
}

impl Serialize for Duration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.num_seconds())
    }
}

use std::fmt;

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}s", self.num_seconds())
    }
}

impl Clone for Duration {
    fn clone(&self) -> Duration {
        Duration( self.0.clone() )
    }
}

use serde::de::{self, Visitor};

struct DurationVisitor;

impl<'de> Visitor<'de> for DurationVisitor {
    type Value = Duration;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a floating point number")
    }

    fn visit_f64<E>(self, value: f64) -> Result<Duration, E>
    where
        E: de::Error,
    {
        Ok(Duration::from_seconds(value))
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_f64(DurationVisitor)
    }
}


impl PartialEq for Duration {
    fn eq(&self, other: &Duration) -> bool {
        self.num_nanos() == other.num_nanos()
    }
}
