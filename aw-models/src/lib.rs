#[macro_use]
extern crate log;

// TODO: Move me to an appropriate place
#[cfg(test)] // Only macro use for tests
macro_rules! json_map {
    { $( $key:literal : $value:expr),* } => {{
        use serde_json::{Value};
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
mod info;
mod query;
mod timeinterval;
mod tryvec;

pub use self::bucket::Bucket;
pub use self::bucket::BucketMetadata;
pub use self::bucket::BucketsExport;
pub use self::event::Event;
pub use self::info::Info;
pub use self::query::Query;
pub use self::timeinterval::TimeInterval;
pub use self::tryvec::TryVec;
