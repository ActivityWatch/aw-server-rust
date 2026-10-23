use aw_datastore::Datastore;
use aw_models::Bucket;
use aw_server::{
    config::AWConfig,
    endpoints::{self, AssetResolver, ServerState},
};
use rocket::{
    http::{ContentType, Header, Status},
    local::blocking::Client,
};
use serde_json::{json, Value};

fn client() -> Client {
    Client::untracked(endpoints::build_rocket(
        ServerState::new(
            Datastore::new_in_memory(false),
            AssetResolver::new(None),
            "server-uuid".into(),
        ),
        AWConfig::default(),
    ))
    .unwrap()
}

fn request(
    client: &Client,
    method: rocket::http::Method,
    path: &str,
    body: Option<Value>,
) -> (Status, String) {
    let mut request = client
        .req(method, path)
        .header(Header::new("Host", "127.0.0.1:5600"));
    if let Some(body) = body {
        request = request.header(ContentType::JSON).body(body.to_string());
    }
    let response = request.dispatch();
    (
        response.status(),
        response.into_string().unwrap_or_default(),
    )
}

#[test]
fn device_bucket_routes_preserve_scope_through_crud_events_and_streamed_exports() {
    use rocket::http::Method::{Delete, Get, Post};
    let client = client();
    for (device, id, hostname) in [
        ("local", "window_host", "host"),
        ("peer", "window_other", "other"),
    ] {
        let path = format!("/api/0/devices/{device}/buckets/{id}");
        let (status, body) = request(
            &client,
            Post,
            &path,
            Some(
                json!({"id": "ignored", "hostname": hostname, "device_id": "forged", "type": "test", "client": "test"}),
            ),
        );
        assert_eq!(status, Status::Ok, "{body}");
        let (status, body) = request(&client, Get, &path, None);
        assert_eq!(status, Status::Ok, "{body}");
        let stored: Bucket = serde_json::from_str(&body).unwrap();
        assert_eq!(stored.id, id);
        assert_eq!(stored.device_id, device);
    }
    // The alias is globally ambiguous but unambiguous inside each device.
    assert_eq!(
        request(&client, Get, "/api/0/buckets/window", None).0,
        Status::Conflict
    );
    for (device, duration) in [("server-uuid", 1), ("peer", 2)] {
        let base = format!("/api/0/devices/{device}/buckets/window");
        let (status, body) = request(
            &client,
            Post,
            &format!("{base}/events"),
            Some(
                json!([{"timestamp": "2026-01-01T00:00:00Z", "duration": duration, "data": {"source": device}}]),
            ),
        );
        assert_eq!(status, Status::Ok, "{body}");
        let events: Value = serde_json::from_str(&body).unwrap();
        let id = events[0]["id"].as_i64().unwrap();
        assert_eq!(
            request(&client, Get, &format!("{base}/events/count"), None),
            (Status::Ok, "1".into())
        );
        let (status, body) = request(&client, Get, &format!("{base}/events/{id}"), None);
        assert_eq!(status, Status::Ok);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["duration"].as_f64(),
            Some(f64::from(duration))
        );
        let (status, body) = request(&client, Get, &format!("{base}/events"), None);
        assert_eq!(status, Status::Ok);
        assert_eq!(
            serde_json::from_str::<Value>(&body)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        for suffix in ["export", "export/csv"] {
            let (status, body) = request(&client, Get, &format!("{base}/{suffix}"), None);
            assert_eq!(status, Status::Ok, "{body}");
            assert!(body.contains(device), "{body}");
            assert!(
                !body.contains(if device == "peer" {
                    "server-uuid"
                } else {
                    "peer"
                }),
                "{body}"
            );
        }
        for suffix in ["buckets/", "export"] {
            let (status, body) = request(
                &client,
                Get,
                &format!("/api/0/devices/{device}/{suffix}"),
                None,
            );
            assert_eq!(status, Status::Ok, "{body}");
            let value: Value = serde_json::from_str(&body).unwrap();
            let buckets = if suffix == "export" {
                &value["buckets"]
            } else {
                &value
            };
            assert_eq!(buckets.as_object().unwrap().len(), 1);
            assert!(buckets
                .get(if device == "peer" {
                    "window_other"
                } else {
                    "window_host"
                })
                .is_some());
        }
        let heartbeat = json!({"timestamp": "2026-01-01T00:00:10Z", "duration": 0, "data": {"heartbeat": device}});
        assert_eq!(
            request(
                &client,
                Post,
                &format!("{base}/heartbeat?pulsetime=5"),
                Some(heartbeat)
            )
            .0,
            Status::Ok
        );
        assert_eq!(
            request(&client, Delete, &format!("{base}/events/{id}"), None).0,
            Status::Ok
        );
        let (_, body) = request(&client, Get, &format!("{base}/events"), None);
        let events: Value = serde_json::from_str(&body).unwrap();
        let remaining = events[0]["id"].as_i64().unwrap();
        assert_eq!(
            request(
                &client,
                Post,
                &format!("{base}/events/delete"),
                Some(json!({"ids": [remaining, remaining]}))
            ),
            (Status::Ok, "1".into())
        );
    }
    assert_eq!(
        request(&client, Get, "/api/0/devices/absent/buckets/window", None).0,
        Status::NotFound
    );
    assert_eq!(
        request(
            &client,
            Get,
            "/api/0/devices/local/buckets/window_other/events",
            None
        )
        .0,
        Status::NotFound
    );
    assert_eq!(
        request(
            &client,
            Delete,
            "/api/0/devices/server-uuid/buckets/window",
            None
        )
        .0,
        Status::Ok
    );
    assert_eq!(
        request(&client, Get, "/api/0/devices/local/buckets/window", None).0,
        Status::NotFound
    );
    assert_eq!(
        request(&client, Get, "/api/0/devices/peer/buckets/window", None).0,
        Status::Ok
    );
}

#[test]
fn scoped_writes_invalidate_queries_and_validation_cannot_touch_another_device() {
    use rocket::http::Method::{Get, Post};
    let client = client();
    for (device, id) in [("local", "window_host"), ("peer", "window_other")] {
        assert_eq!(request(&client, Post, &format!("/api/0/devices/{device}/buckets/{id}"), Some(json!({"hostname": if device == "local" {"host"} else {"other"}, "type": "test", "client": "test"}))).0, Status::Ok);
    }
    let query = json!({"query": ["return sum_durations(query_bucket(\"window\"));"], "timeperiods": ["2026-01-01T00:00:00Z/2026-01-02T00:00:00Z"]});
    let read_total = |device: &str| {
        let (status, body) = request(
            &client,
            Post,
            &format!("/api/0/devices/{device}/query"),
            Some(query.clone()),
        );
        assert_eq!(status, Status::Ok, "{body}");
        serde_json::from_str::<Value>(&body).unwrap()[0]
            .as_f64()
            .unwrap()
    };
    assert_eq!(read_total("local"), 0.0);
    assert_eq!(read_total("peer"), 0.0);
    let event = json!({"timestamp": "2026-01-01T00:00:00Z", "duration": 3, "data": {}});
    assert_eq!(
        request(
            &client,
            Post,
            "/api/0/devices/server-uuid/buckets/window/events",
            Some(json!([event.clone()]))
        )
        .0,
        Status::Ok
    );
    assert_eq!(read_total("local"), 3.0);
    assert_eq!(read_total("peer"), 0.0);
    assert_eq!(
        request(
            &client,
            Post,
            "/api/0/devices/peer/buckets/window/heartbeat?pulsetime=-1",
            Some(event)
        )
        .0,
        Status::BadRequest
    );
    assert_eq!(
        request(
            &client,
            Post,
            "/api/0/devices/peer/buckets/window/events",
            Some(json!([{"timestamp": "2026-01-01T00:00:00Z", "duration": -1, "data": {}}]))
        )
        .0,
        Status::BadRequest
    );
    assert_eq!(
        request(
            &client,
            Get,
            "/api/0/devices/peer/buckets/window/events?start=invalid",
            None
        )
        .0,
        Status::BadRequest
    );
    assert_eq!(
        request(
            &client,
            Post,
            "/api/0/devices/peer/buckets/window/events/delete",
            Some(json!({"ids": vec![1; 10_001]}))
        )
        .0,
        Status::PayloadTooLarge
    );
    assert_eq!(read_total("local"), 3.0);
    assert_eq!(read_total("peer"), 0.0);
    assert_eq!(
        request(
            &client,
            Post,
            "/api/0/devices/server-uuid/buckets/window",
            Some(json!({"hostname": "host", "type": "test", "client": "test"}))
        )
        .0,
        Status::NotModified
    );
    let (status, body) = request(&client, Get, "/api/0/devices/local/buckets", None);
    assert_eq!(status, Status::Ok);
    assert_eq!(
        serde_json::from_str::<Value>(&body)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn device_and_bucket_url_components_are_decoded_without_composite_keys() {
    use rocket::http::Method::{Get, Post};
    let client = client();
    let path = "/api/0/devices/peer%3Aone/buckets/window%3Ahost%20space";
    assert_eq!(
        request(
            &client,
            Post,
            path,
            Some(json!({"hostname": "host", "type": "test", "client": "test"}))
        )
        .0,
        Status::Ok
    );
    let (status, body) = request(&client, Get, path, None);
    assert_eq!(status, Status::Ok, "{body}");
    let bucket: Bucket = serde_json::from_str(&body).unwrap();
    assert_eq!(bucket.device_id, "peer:one");
    assert_eq!(bucket.id, "window:host space");
    assert_eq!(
        request(
            &client,
            Get,
            "/api/0/devices/peer/buckets/one%3Awindow%3Ahost%20space",
            None
        )
        .0,
        Status::NotFound
    );
}

#[test]
fn local_hostname_sentinel_cannot_forge_peer_metadata() {
    use rocket::http::Method::{Get, Post};
    let client = client();
    let bucket = json!({"hostname": "!local", "type": "test", "client": "test"});
    assert_eq!(
        request(
            &client,
            Post,
            "/api/0/devices/peer/buckets/sentinel",
            Some(bucket.clone())
        )
        .0,
        Status::BadRequest
    );
    assert_eq!(
        request(&client, Get, "/api/0/devices/peer/buckets/sentinel", None).0,
        Status::NotFound
    );
    assert_eq!(
        request(
            &client,
            Post,
            "/api/0/devices/server-uuid/buckets/sentinel",
            Some(bucket)
        )
        .0,
        Status::Ok
    );
    let (_, body) = request(&client, Get, "/api/0/devices/local/buckets/sentinel", None);
    let stored: Bucket = serde_json::from_str(&body).unwrap();
    assert_eq!(stored.device_id, "local");
    assert_eq!(stored.data["device_id"], "server-uuid");
}

#[test]
fn legacy_import_normalizes_server_uuid_before_alias_resolution_and_merge() {
    use rocket::http::Method::{Get, Post};
    let client = client();
    let import = json!({"buckets": {"window_host": {"id": "window_host", "hostname": "host", "device_id": "server-uuid", "type": "test", "client": "test", "events": [{"id": 42, "timestamp": "2026-01-01T00:00:00Z", "duration": 2, "data": {"app": "test"}}]}}});
    for _ in 0..2 {
        let (status, body) = request(&client, Post, "/api/0/import", Some(import.clone()));
        assert_eq!(status, Status::Ok, "{body}");
        let (status, body) = request(
            &client,
            Get,
            "/api/0/devices/server-uuid/buckets/window",
            None,
        );
        assert_eq!(status, Status::Ok, "{body}");
        let stored: Bucket = serde_json::from_str(&body).unwrap();
        assert_eq!(stored.device_id, "local");
        assert_eq!(stored.id, "window_host");
        assert_eq!(
            request(
                &client,
                Get,
                "/api/0/devices/local/buckets/window/events/count",
                None
            ),
            (Status::Ok, "1".into())
        );
    }
    assert_eq!(
        request(
            &client,
            Post,
            "/api/0/buckets/window",
            Some(json!({"hostname": "host", "type": "test", "client": "test"}))
        )
        .0,
        Status::NotModified
    );
}
