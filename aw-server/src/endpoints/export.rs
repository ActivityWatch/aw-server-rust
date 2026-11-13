use rocket::State;

use crate::endpoints::bucket::parse_time_param;
use crate::endpoints::util::BucketsExportRocket;
use crate::endpoints::{HttpErrorJson, ServerState};

/// Export every bucket as JSON, with all events, or with `start`/`end` (RFC 3339)
/// those overlapping that range, cut to it like `/events`.
#[get("/?<start>&<end>")]
pub fn buckets_export(
    start: Option<String>,
    end: Option<String>,
    state: &State<ServerState>,
) -> Result<BucketsExportRocket, HttpErrorJson> {
    BucketsExportRocket::new(
        &state.datastore,
        None,
        parse_time_param("start", start)?,
        parse_time_param("end", end)?,
    )
}
