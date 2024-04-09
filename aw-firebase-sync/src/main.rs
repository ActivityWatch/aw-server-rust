use reqwest;
use chrono::Utc;
use serde_json::{json, Value};
use serde_yaml;
use aw_client_rust::AwClient;
use std::fs::File;
use std::io::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new client
    let aw_client = AwClient::new("localhost", 5600, "aw-firebase-sync").unwrap();
    // 7 days ago
    let start = Utc::now().date().and_hms_opt(0, 0, 0).unwrap() - chrono::Duration::days(6);
    let end = Utc::now().date().and_hms_opt(0, 0, 0).unwrap() + chrono::Duration::days(1);

    let mut file = File::open("config.yaml").expect("Unable to open file");
    let mut contents = String::new();
    file.read_to_string(&mut contents).expect("Unable to read file");
    let yaml: Value = serde_yaml::from_str(&contents).unwrap();
    let apikey = yaml["apikey"].as_str().unwrap().to_string();
    if apikey == "your-api-key" || apikey == "" {
        panic!("Please set your API key in the config.yaml file");
    }

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
        "apiKey": apikey,
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
