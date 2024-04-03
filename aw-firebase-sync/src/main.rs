// use aw_models::{Query, TimeInterval};
use reqwest;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Value};
use aw_client_rust::AwClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new client
    let aw_client = AwClient::new("localhost", 5600, "test").unwrap();

    let start = Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2024, 4, 1, 0, 0, 0).unwrap();
    
    let res = aw_client.get_events("aw-watcher-window_brayo",Some(start),Some(end), Some(50)).await.unwrap();

    let res_string = serde_json::to_string(&res).unwrap();
    // println!("{:?}", res_string);
    // Your Firebase callable function URL
    let url = "https://us-central1-aw-mockup.cloudfunctions.net/storeDataREST";

    // Prepare the request body
    let payload = json!({
        "apiKey": "fv3yShDm3VHuMjts1P7A+LcjvR66",
        "data": res_string
    });

    // Send the POST request
    let http_client = reqwest::Client::new();
    let response = http_client
        .post(url)
        .json(&payload)
        .send()
        .await?
        .json::<Value>()
        .await?;

    // Handle the response
    println!("Response: {:?}", response);

    Ok(())
}
