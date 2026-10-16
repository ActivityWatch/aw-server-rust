#[macro_use]
extern crate log;
extern crate chrono;
extern crate serde;
extern crate serde_json;

mod report;
pub use report::{
    last_report_path, load_last_report, persist_last_report, persist_last_report_warn,
    BucketReport, PeerOutcome, PeerReport, SyncMode, SyncReport,
};

mod sync;
pub use sync::create_datastore;
pub use sync::sanitize_hostname;
pub use sync::sync_datastores;
pub use sync::sync_run;
pub use sync::SyncSpec;

mod sync_wrapper;
pub use sync_wrapper::{pull, pull_all};
pub use sync_wrapper::{push, push_with_hostname};

mod accessmethod;
pub use accessmethod::AccessMethod;

mod dirs;
#[cfg(feature = "cli")]
mod legacy_clean;
#[cfg(feature = "cli")]
pub use legacy_clean::run_clean_legacy;
#[cfg(feature = "cli")]
mod status;
#[cfg(feature = "cli")]
pub use status::run_status;
mod util;

#[cfg(feature = "sync-v2")]
pub mod v2;

#[cfg(target_os = "android")]
pub mod android;
