extern crate serde;
#[cfg_attr(test, macro_use)] // Only macro use for tests
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate chrono;
#[macro_use]
extern crate log;

// TODO: Move me to an appropriate place
#[cfg(test)] // Only macro use for tests
#[macro_use]
macro_rules! json_map {
    { $( $key:literal : $value:expr),* } => {{
        use serde_json::Value;
        use serde_json::map::Map;
        #[allow(unused_mut)]
        let mut map : Map<String, Value> = Map::new();
        $(
          map.insert( $key.to_string(), json!($value) );
        )*
        map
    }};
}

mod bucket;
mod duration;
mod event;
mod key_value;
mod query;
mod timeinterval;

pub use self::bucket::Bucket;
pub use self::bucket::BucketMetadata;
pub use self::bucket::BucketsExport;
pub use self::event::Event;
pub use self::key_value::Key;
pub use self::key_value::KeyValue;
pub use self::query::Query;
pub use self::timeinterval::TimeInterval;
