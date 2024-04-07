use reqwest;
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};
use aw_client_rust::AwClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new client
    let aw_client = AwClient::new("localhost", 5600, "test").unwrap();

    let start = Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2024, 4, 7, 0, 0, 0).unwrap();

    let query = "window_events = query_bucket(find_bucket(\"aw-watcher-window_\"));
    RETURN = window_events;";
    
    let timeperiods = vec!(
        (start, end)
    );
    
    let res = aw_client.query(&query, timeperiods).await.unwrap();

    let res_string = serde_json::to_string(&res).unwrap();
    // strip the leading and trailing '[' and ']'
    let res_string = &res_string[1..res_string.len()-1];

    // let url = "https://us-central1-aw-mockup.cloudfunctions.net/uploadData";
    let url = "http://localhost:5001/aw-mockup/us-central1/uploadData";

    // Prepare the request body
    let payload = json!({
        "apiKey": "Je_Q45pexF2Y17gioBIt_ePU.iH",
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
