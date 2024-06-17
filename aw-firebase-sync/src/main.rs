use aw_client_rust::AwClient;
use chrono::Utc;
use dirs::config_dir;
use reqwest;
use serde_json::{json, Value};
use serde_yaml;
use std::fs::{DirBuilder, File};
use std::io::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let aw_client = AwClient::new("localhost", 5600, "aw-firebase-sync").unwrap();
    let start = Utc::now().date().and_hms_opt(0, 0, 0).unwrap() - chrono::Duration::days(6);
    let end = Utc::now().date().and_hms_opt(0, 0, 0).unwrap() + chrono::Duration::days(1);

    let path = config_dir()
        .map(|mut path| {
            path.push("activitywatch");
            path.push("aw-firebase-sync");
            path.push("config.yaml");
            path
        })
        .unwrap();

    if !path.exists() {
        DirBuilder::new()
            .recursive(true)
            .create(path.as_path().parent().expect("Unable to get parent path"))
            .expect("Unable to create config directory");
        let mut file = File::create(&path).expect("Unable to create file");
        file.write_all(b"apikey: your-api-key\n")
            .expect("Unable to write to file");
        panic!("Please set your API key at {:?}", path.to_str().unwrap());
    }

    let mut file = File::open(path).expect("Unable to open file");
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .expect("Unable to read file");
    let yaml: Value = serde_yaml::from_str(&contents).expect("Failed parsing yaml from config file");
    let apikey = yaml["apikey"].as_str().expect("unable to get api key from config file");
    if apikey == "your-api-key" || apikey == "" {
        panic!("Please set your API key in the config.yaml file");
    }

    let query = "window_events = query_bucket(find_bucket(\"aw-watcher-window_\"));
    RETURN = window_events;";

    let timeperiods = vec![(start, end)];

    // TODO: handle errors
    let res = aw_client.query(&query, timeperiods).await.unwrap(); 

    let res_string = serde_json::to_string(&res).unwrap();
    // strip the leading and trailing '[' and ']'
    let res_string = &res_string[1..res_string.len() - 1];

    let firebase_url = "https://us-central1-aw-mockup.cloudfunctions.net/uploadData";
    // let firebase_url = "http://localhost:5001/aw-mockup/us-central1/uploadData";

    let payload = json!({
        "apiKey": apikey,
        "data": res_string
    });

    let firebase_client = reqwest::Client::new();
    let response = firebase_client
        .post(firebase_url)
        .json(&payload)
        .send()
        .await?
        .json::<Value>()
        .await?;

    println!("Response: {:?}", response);

    Ok(())
}
