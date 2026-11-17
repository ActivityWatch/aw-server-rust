use rocket::State;

use crate::endpoints::util::BucketsExportRocket;
use crate::endpoints::{HttpErrorJson, ServerState};

#[get("/")]
pub fn buckets_export(state: &State<ServerState>) -> Result<BucketsExportRocket, HttpErrorJson> {
    BucketsExportRocket::new(&state.datastore, None)
}
