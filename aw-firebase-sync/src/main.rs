use aw_client_rust::AwClient;
use chrono::Utc;
use dirs::config_dir;
use reqwest;
use serde_json::{json, Value};
use serde_yaml;
use std::fs::{DirBuilder, File};
use std::io::prelude::*;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    let args: Vec<String> = env::args().collect();
    let mut port: u16 = 5600;
    if args.len() > 1 {
        for idx in 1..args.len() {
            if args[idx] == "--port" {
                port = args[idx + 1].parse().expect("Invalid port number");
                break;
            }
            if args[idx] == "--testing" {
                port = 5699;
            }
            if args[idx] == "--help" {
                println!("Usage: aw-firebase-sync [--testing] [--port PORT] [--help]");
                return Ok(());
            }
        }
    }
    let aw_client = AwClient::new("localhost", port, "aw-firebase-sync").unwrap();

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
    let yaml: Value =
        serde_yaml::from_str(&contents).expect("Failed parsing yaml from config file");
    let apikey = yaml["apikey"]
        .as_str()
        .expect("unable to get api key from config file");
    if apikey == "your-api-key" || apikey == "" {
        panic!("Please set your API key in the config.yaml file");
    }


    let query = "
            events = flood(query_bucket(find_bucket(\"aw-watcher-window_\")));
            not_afk = flood(query_bucket(find_bucket(\"aw-watcher-afk_\")));
            not_afk = filter_keyvals(not_afk, \"status\", [\"not-afk\"]);
            events = filter_period_intersect(events, not_afk);
            events = categorize(events, [[[\"Work\"], {\"type\": \"regex\", \"regex\": \"aw|ActivityWatch\", \"ignore_case\": true}]]);
            events = filter_keyvals(events, \"$category\", [[\"Work\"]]);
            RETURN = events;
        ";

    let timeperiods = vec![(start, end)];

    let query_result = aw_client.query(&query, timeperiods).await.expect("Failed to query data");

    let query_data = serde_json::to_string(&query_result[0]).unwrap();

    let firebase_url = "https://us-central1-aw-mockup.cloudfunctions.net/uploadData";
    // let firebase_url = "http://localhost:5001/aw-mockup/us-central1/uploadData";

    let payload = json!({
        "apiKey": apikey,
        "data": query_data
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
