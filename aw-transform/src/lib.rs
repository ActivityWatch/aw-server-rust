#[macro_use]
extern crate log;

// TODO: Move this to some more suitable place
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

pub mod classify;

mod heartbeat;
pub use heartbeat::heartbeat;

mod find_bucket;
pub use find_bucket::find_bucket;

mod flood;
pub use flood::flood;

mod merge;
pub use merge::merge_events_by_keys;

mod chunk;
pub use chunk::chunk_events_by_key;

mod sort;
pub use sort::{sort_by_duration, sort_by_timestamp};

mod filter_keyvals;
pub use filter_keyvals::filter_keyvals;

mod filter_period;
pub use filter_period::filter_period_intersect;

mod split_url;
pub use split_url::split_url_event;
