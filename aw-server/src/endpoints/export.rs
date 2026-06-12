use std::collections::HashMap;

use rocket::State;

use aw_models::BucketsExport;
use aw_models::TryVec;

use crate::endpoints::util::BucketsExportRocket;
use crate::endpoints::{HttpErrorJson, ServerState};

#[get("/")]
pub fn buckets_export(state: &State<ServerState>) -> Result<BucketsExportRocket, HttpErrorJson> {
    let datastore = &state.datastore;
    let mut export = BucketsExport {
        buckets: HashMap::new(),
    };
    let mut buckets = datastore.get_buckets()?;
    for (bid, mut bucket) in buckets.drain() {
        let events = datastore.get_events(&bid, None, None, None)?;
        bucket.events = Some(TryVec::new(events));
        export.buckets.insert(bid, bucket);
    }

    Ok(export.into())
}
