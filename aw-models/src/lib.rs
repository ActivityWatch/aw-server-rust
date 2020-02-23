extern crate serde;
#[macro_use] extern crate serde_json;
#[macro_use] extern crate serde_derive;
extern crate chrono;
#[macro_use] extern crate log;

// TODO: Move me to an appropriate place
#[macro_export]
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

mod duration;
mod bucket;
mod event;
mod timeinterval;
mod query;
mod key_value;

pub use self::bucket::Bucket;
pub use self::bucket::BucketMetadata;
pub use self::bucket::BucketsExport;
pub use self::event::Event;
pub use self::timeinterval::TimeInterval;
pub use self::query::Query;
pub use self::key_value::KeyValue;
