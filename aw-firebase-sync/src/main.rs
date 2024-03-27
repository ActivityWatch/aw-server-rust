use tokio;
use aw_client_rust::AwClient;
use chrono::prelude::*;
pub struct TimeInterval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new client
    let client = AwClient::new("localhost", 5600, "my-client").unwrap();

    // Define the query
    let _query = vec![
        "events = query_bucket(\"aw-watcher-window_my-hostname\");".to_string(),
        "events = merge_events_by_keys(events, [\"app\"]);".to_string(),
        "RETURN = sort_by_duration(events);".to_string(),
    ];

    let start = Utc.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2024, 3, 28, 0, 0, 0).unwrap();
    let _timeperiod = TimeInterval{start, end};

    // let buckets = client.get_buckets().await?;
    // println!("{:?}", buckets);
    let data = client.get_events("aw-watcher-window_brayo", Some(start), Some(end), Some(50)).await.unwrap();
    // Print the result
    println!("{:?}", data);

    Ok(())
}