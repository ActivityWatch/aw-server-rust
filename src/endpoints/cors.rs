use rocket_cors;
use rocket::http::Method;
use rocket_cors::{AllowedHeaders, AllowedOrigins};

pub fn cors() -> rocket_cors::Cors {
    // FIXME: This is highly unsecure!!!
    // rocket_cors does not have support for dynamic origins such as
    // "moz-extension://*" which we need to support aw-watcher-web
    let allowed_origins = AllowedOrigins::all();
    /*
    let (allowed_origins, failed_origins) = AllowedOrigins::some(&[
        "moz-extension://eace5a37-c519-4119-8d0e-a4683bdea380",
        // TODO: Add "http://127.0.0.1:27180" when running in testing
    ]);
    assert!(failed_origins.is_empty());
    */

    let allowed_methods = vec![Method::Get, Method::Post, Method::Delete]
        .into_iter().map(From::from).collect();

    let allowed_headers = AllowedHeaders::all(); // TODO: is this unsafe?

    // You can also deserialize this
    rocket_cors::Cors {
        allowed_origins: allowed_origins,
        allowed_methods: allowed_methods,
        allowed_headers: allowed_headers,
        allow_credentials: false,
        ..Default::default()
    }
}
