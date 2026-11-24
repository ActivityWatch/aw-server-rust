extern crate aw_datastore;
extern crate aw_server;
extern crate rocket;

#[cfg(test)]
mod query_deadline {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use rocket::http::{ContentType, Header, Status};
    use rocket::local::blocking::Client;
    use serde_json::json;

    use aw_server::config;
    use aw_server::endpoints;

    fn setup_testserver(query_timeout: Duration) -> rocket::Rocket<rocket::Build> {
        let state = endpoints::ServerState {
            query_timeout: Some(query_timeout),
            ..endpoints::ServerState::new(
                aw_datastore::Datastore::new_in_memory(false),
                endpoints::AssetResolver::new(None),
                "test_id".to_string(),
            )
        };
        endpoints::build_rocket(state, config::AWConfig::default())
    }

    /// A categorize rule using a fancy-regex feature (lookahead) runs on the
    /// backtracking VM, and each failing match burns up to the backtrack limit.
    /// categorize must stop at the query deadline between matches instead of
    /// pinning a Rocket worker far past `query_timeout` (#893).
    #[test]
    fn categorize_backtracking_regex_respects_query_timeout() {
        // Enough work that even a release build runs far past the budget
        // without in-transform deadline checks.
        const N_EVENTS: usize = 600;
        const N_RULES: usize = 1;
        let budget = Duration::from_secs(1);

        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let client = Client::untracked(setup_testserver(budget)).expect("valid instance");
            let res = client
                .post("/api/0/buckets/b")
                .header(ContentType::JSON)
                .header(Header::new("Host", "127.0.0.1:5600"))
                .body(r#"{"id":"b","type":"currentwindow","client":"c","hostname":"h"}"#)
                .dispatch();
            assert_eq!(res.status(), Status::Ok);

            let events: Vec<_> = (0..N_EVENTS)
                .map(|i| {
                    json!({
                        "timestamp": format!("2020-01-01T{:02}:{:02}:{:02}Z", i / 3600, (i / 60) % 60, i % 60),
                        "duration": 1.0,
                        // Distinct titles: categorize caches by event data.
                        "data": {"title": format!("{}b{i}", "a".repeat(100))}
                    })
                })
                .collect();
            let res = client
                .post("/api/0/buckets/b/events")
                .header(ContentType::JSON)
                .header(Header::new("Host", "127.0.0.1:5600"))
                .body(serde_json::to_string(&events).unwrap())
                .dispatch();
            assert_eq!(res.status(), Status::Ok);

            let rules: Vec<String> = (0..N_RULES)
                .map(|i| {
                    format!(r#"[["c{i}"], {{"type": "regex", "regex": "^(a|aa|(?=a)a)*${i}"}}]"#)
                })
                .collect();
            let query = json!({
                "timeperiods": ["2020-01-01T00:00:00Z/2020-01-02T00:00:00Z"],
                "query": [
                    "events = query_bucket(\"b\");",
                    format!("events = categorize(events, [{}]);", rules.join(", ")),
                    "RETURN = events;"
                ]
            });
            let started = Instant::now();
            let res = client
                .post("/api/0/query/?cache=false")
                .header(ContentType::JSON)
                .header(Header::new("Host", "127.0.0.1:5600"))
                .body(query.to_string())
                .dispatch();
            let status = res.status();
            let body = res.into_string().unwrap_or_default();
            let _ = sender.send((started.elapsed(), status, body));
        });

        match receiver.recv_timeout(Duration::from_secs(120)) {
            Ok((elapsed, status, body)) => {
                println!(
                    "query took {:.1}s with a {}s budget, status {status}, body {:.200}",
                    elapsed.as_secs_f64(),
                    budget.as_secs(),
                    body
                );
                assert_eq!(status, Status::ServiceUnavailable, "{body}");
                assert!(body.contains("before categorize"), "{body}");
                assert!(
                    elapsed < budget * 5,
                    "one categorize query with a backtracking regex ran {:.1}s \
                     against a {}s query_timeout",
                    elapsed.as_secs_f64(),
                    budget.as_secs()
                );
            }
            Err(_) => panic!("query did not return within 120s (budget was 1s)"),
        }
    }
}
