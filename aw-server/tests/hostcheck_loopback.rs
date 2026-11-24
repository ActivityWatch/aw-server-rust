extern crate aw_datastore;
extern crate aw_server;
extern crate rocket;

use rocket::http::{ContentType, Header, Status};
use rocket::local::blocking::Client;

use aw_server::config::AWConfig;
use aw_server::endpoints;

fn client_bound_to(address: &str) -> Client {
    let state = endpoints::ServerState::new(
        aw_datastore::Datastore::new_in_memory(false),
        endpoints::AssetResolver::new(None),
        "test_id".to_string(),
    );
    let aw_config = AWConfig {
        address: address.to_string(),
        ..AWConfig::default()
    };
    Client::untracked(endpoints::build_rocket(state, aw_config)).expect("valid rocket")
}

/// A server bound to the IPv6 loopback (`address = "::1"`) is just as local as
/// one bound to 127.0.0.1, so a DNS-rebinding page (AAAA attacker.example -> ::1)
/// must not be able to read data. HostCheck used to skip validation for this bind.
#[test]
fn ipv6_loopback_bind_rejects_rebinding_host_on_read() {
    let client = client_bound_to("::1");
    let res = client
        .get("/api/0/buckets/")
        .header(Header::new("Host", "attacker.example:5600"))
        .dispatch();
    assert_eq!(
        res.status(),
        Status::BadRequest,
        "rebinding Host was served on ::1 bind: {:?}",
        res.into_string()
    );
}

/// Same as above but for a write: creating a bucket via a rebinding page.
#[test]
fn ipv6_loopback_bind_rejects_rebinding_host_on_write() {
    let client = client_bound_to("::1");
    let res = client
        .post("/api/0/buckets/rebound")
        .header(ContentType::JSON)
        .header(Header::new("Host", "attacker.example:5600"))
        .body(r#"{"id":"rebound","type":"t","client":"c","hostname":"h"}"#)
        .dispatch();
    assert_eq!(res.status(), Status::BadRequest);

    let res = client
        .get("/api/0/buckets/rebound")
        .header(Header::new("Host", "[::1]:5600"))
        .dispatch();
    assert_eq!(
        res.status(),
        Status::NotFound,
        "expected the bucket to be absent after the rejected write"
    );
}

/// Any other loopback address (127.0.0.0/8, IPv4-mapped) is equally local.
#[test]
fn other_ipv4_loopback_bind_rejects_rebinding_host() {
    for address in ["127.0.0.2", "::ffff:127.0.0.1"] {
        let client = client_bound_to(address);
        let res = client
            .get("/api/0/buckets/")
            .header(Header::new("Host", "attacker.example:5600"))
            .dispatch();
        assert_eq!(res.status(), Status::BadRequest, "bind {address}");
    }
}

/// The fix must not lock out legitimate clients of a non-default loopback bind.
#[test]
fn loopback_binds_accept_their_own_host() {
    for (address, host) in [
        ("::1", "[::1]:5600"),
        ("::1", "[::1]"),
        ("::1", "localhost:5600"),
        ("::1", "[0:0:0:0:0:0:0:1]:5600"),
        ("::ffff:127.0.0.1", "[::ffff:127.0.0.1]:5600"),
        ("::ffff:127.0.0.1", "[::ffff:7f00:1]:5600"),
        ("::ffff:127.0.0.2", "[::ffff:7f00:2]:5600"),
        ("::ffff:7f00:2", "127.0.0.2:5600"),
        ("127.0.0.2", "[::ffff:127.0.0.2]:5600"),
        ("127.0.0.2", "127.0.0.2:5600"),
        ("127.0.0.1", "127.0.0.1:5600"),
    ] {
        let client = client_bound_to(address);
        let res = client
            .get("/api/0/info")
            .header(Header::new("Host", host))
            .dispatch();
        assert_eq!(res.status(), Status::Ok, "bind {address}, Host {host}");
    }
}

/// Canonicalizing IP literals must not broaden the allowlist to unrelated IPs.
#[test]
fn loopback_bind_rejects_unrelated_ip_hosts() {
    let client = client_bound_to("127.0.0.2");
    for host in [
        "127.0.0.3:5600",
        "[::ffff:7f00:3]:5600",
        "192.168.0.1:5600",
        "[::ffff:192.168.0.1]:5600",
        "[2001:db8::1]:5600",
    ] {
        let res = client
            .get("/api/0/info")
            .header(Header::new("Host", host))
            .dispatch();
        assert_eq!(res.status(), Status::BadRequest, "Host {host}");
    }
}

/// A malformed bracketed Host is rejected rather than parsed loosely.
#[test]
fn malformed_ipv6_host_is_rejected() {
    let client = client_bound_to("::1");
    for host in [
        "[::1",
        "[::1]x",
        "[attacker.example]:5600",
        "[localhost]:5600",
        "[127.0.0.1]:5600",
        "[::1]:not-a-port",
        "[::1]:",
        "[::1]:5600:extra",
        "localhost:not-a-port",
        "localhost:5600:extra",
    ] {
        let res = client
            .get("/api/0/info")
            .header(Header::new("Host", host))
            .dispatch();
        assert_eq!(res.status(), Status::BadRequest, "Host {host}");
    }
}
