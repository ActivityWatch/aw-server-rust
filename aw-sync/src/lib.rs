#[macro_use]
extern crate log;
extern crate chrono;
extern crate serde;
extern crate serde_json;

mod sync;
pub use sync::sync_datastores;
