use aw_models::{Bucket, Event};
use aw_server::{
    config::AWConfig,
    endpoints::{self, AssetResolver, ServerState},
};
use rocket::{
    http::{ContentType, Header, Status},
    local::blocking::Client,
};
use serde_json::{json, Value};

#[test]
fn category_api_is_bucket_scoped_validated_and_clearable() {
    let ds = aw_datastore::Datastore::new_in_memory(false);
    for name in ["one", "two"] {
        let bucket: Bucket = serde_json::from_value(
            json!({"id":name,"type":"test","client":"test","hostname":"test"}),
        )
        .unwrap();
        ds.create_bucket(&bucket).unwrap();
    }
    let event = ds
        .insert_events("one", &[Event::default()])
        .unwrap()
        .remove(0);
    let raw = ds.get_event("one", event.id.unwrap()).unwrap();
    let state = ServerState::new(ds.clone(), AssetResolver::new(None), "test".into());
    let client = Client::untracked(endpoints::build_rocket(state, AWConfig::default())).unwrap();
    let path = format!("/api/0/buckets/one/events/{}/category", event.id.unwrap());
    let wrong = path.replace("/one/", "/two/");
    let get = || {
        client
            .get(&path)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch()
    };
    assert_eq!(get().status(), Status::Ok);
    assert_eq!(get().into_json::<Value>().unwrap(), Value::Null);
    for body in [json!([]), json!([""]), json!([" "])] {
        let response = client
            .put(&path)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .header(ContentType::JSON)
            .body(body.to_string())
            .dispatch();
        assert_eq!(response.status(), Status::BadRequest);
    }
    for method in [
        rocket::http::Method::Get,
        rocket::http::Method::Put,
        rocket::http::Method::Delete,
    ] {
        let response = client
            .req(method, &wrong)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .header(ContentType::JSON)
            .body("[\"Work\"]")
            .dispatch();
        assert_eq!(response.status(), Status::NotFound);
    }
    for _ in 0..2 {
        let response = client
            .put(&path)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .header(ContentType::JSON)
            .body("[\"Work\",\"Client\"]")
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
    }
    assert_eq!(
        get().into_json::<Value>().unwrap(),
        json!(["Work", "Client"])
    );
    assert_eq!(ds.get_event("one", event.id.unwrap()).unwrap(), raw);
    for _ in 0..2 {
        assert_eq!(
            client
                .delete(&path)
                .header(Header::new("Host", "127.0.0.1:5600"))
                .dispatch()
                .status(),
            Status::Ok
        );
    }
    assert_eq!(get().into_json::<Value>().unwrap(), Value::Null);
}

#[test]
fn category_writes_invalidate_cached_queries_and_allow_cors_put() {
    let ds = aw_datastore::Datastore::new_in_memory(false);
    let bucket: Bucket = serde_json::from_value(
        json!({"id":"history","type":"test","client":"test","hostname":"test"}),
    )
    .unwrap();
    ds.create_bucket(&bucket).unwrap();
    let event: Event = serde_json::from_value(
        json!({"timestamp":"2026-01-01T10:00:00Z","duration":10,"data":{"app":"editor"}}),
    )
    .unwrap();
    let id = ds.insert_events("history", &[event]).unwrap()[0]
        .id
        .unwrap();
    let state = ServerState::new(ds, AssetResolver::new(None), "test".into());
    let cache = state.query_cache.clone();
    let client = Client::untracked(endpoints::build_rocket(state, AWConfig::default())).unwrap();
    let query = json!({"timeperiods":["2026-01-01T00:00:00Z/2026-01-02T00:00:00Z"],"query":["return categorize(query_bucket(\"history\"), []);"]}).to_string();
    let read = || {
        client
            .post("/api/0/query")
            .header(Header::new("Host", "127.0.0.1:5600"))
            .header(ContentType::JSON)
            .body(&query)
            .dispatch()
            .into_json::<Value>()
            .unwrap()
    };
    let automatic = read();
    assert_eq!(
        automatic[0][0]["data"]["$category"],
        json!(["Uncategorized"])
    );
    assert_eq!(read(), automatic);
    let path = format!("/api/0/buckets/history/events/{id}/category");
    let before = cache.generation();
    assert_eq!(
        client
            .put(&path)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .header(ContentType::JSON)
            .body("[\"Work\"]")
            .dispatch()
            .status(),
        Status::Ok
    );
    assert!(cache.generation() > before);
    assert_eq!(read()[0][0]["data"]["$category"], json!(["Work"]));
    assert_eq!(
        client
            .delete(&path)
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch()
            .status(),
        Status::Ok
    );
    assert_eq!(read(), automatic);
    let response = client
        .options(&path)
        .header(Header::new("Host", "127.0.0.1:5600"))
        .header(Header::new("Origin", "http://127.0.0.1:5600"))
        .header(Header::new("Access-Control-Request-Method", "PUT"))
        .dispatch();
    assert!(response.status().code < 300);
    let info = client
        .get("/api/0/info")
        .header(Header::new("Host", "127.0.0.1:5600"))
        .dispatch()
        .into_json::<Value>()
        .unwrap();
    assert_eq!(info["manual_event_category"], true);
}
