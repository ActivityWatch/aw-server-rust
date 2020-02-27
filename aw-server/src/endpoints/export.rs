use std::collections::HashMap;
use std::io::Cursor;

use rocket::http::Header;
use rocket::http::Status;
use rocket::response::Response;
use rocket::State;

use aw_models::BucketsExport;

use crate::endpoints::ServerState;

#[get("/")]
pub fn buckets_export(state: State<ServerState>) -> Result<Response, Status> {
    let datastore = endpoints_get_lock!(state.datastore);
    let mut export = BucketsExport {
        buckets: HashMap::new(),
    };
    let mut buckets = datastore.get_buckets().unwrap();
    for (bid, mut bucket) in buckets.drain() {
        bucket.events = Some(
            datastore
                .get_events(&bid, None, None, None)
                .expect("Failed to get events for bucket"),
        );
        export.buckets.insert(bid, bucket);
    }

    let response = Response::build()
        .status(Status::Ok)
        .header(Header::new(
            "Content-Disposition",
            "attachment; filename=aw-buckets-export.json",
        ))
        .sized_body(Cursor::new(
            serde_json::to_string(&export).expect("Failed to serialize"),
        ))
        .finalize();
    return Ok(response);
}
