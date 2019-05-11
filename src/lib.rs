extern crate reqwest;
extern crate gethostname;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use serde_json::{Value, Map};

use std::vec::Vec;
use std::collections::HashMap;

#[derive(Serialize,Deserialize,Debug)]
pub struct Bucket {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub _type: String,
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub last_updated: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Event {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub data: Map<String, Value>,
}

#[derive(Deserialize)]
pub struct Info {
  pub hostname: String,
  pub testing: bool,
}

pub struct AwClient {
    client: reqwest::Client,
    pub baseurl: String,
    pub name: String,
    pub hostname: String
}

impl AwClient {
    pub fn new(ip: String, port: String, name: String) -> AwClient {
        let baseurl = String::from(format!("http://{}:{}", ip, port));
        let client = reqwest::Client::new();
        let hostname = gethostname::gethostname().into_string().unwrap();
        return AwClient {
            client: client,
            baseurl: baseurl,
            name: name,
            hostname: hostname
        };
    }

    pub fn get_bucket(client: &mut AwClient, bucketname: &String) -> Result<Bucket, reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", client.baseurl, bucketname);
        let bucket : Bucket = client.client.get(&url).send()?.json()?;
        Ok(bucket)
    }

    pub fn get_buckets(client: &mut AwClient) -> Result<HashMap<String, Bucket>, reqwest::Error> {
        let url = format!("{}/api/0/buckets/", client.baseurl);
        Ok(client.client.get(&url).send()?.json()?)
    }

    pub fn create_bucket(client: &mut AwClient, bucketname: &String, buckettype: &String) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", client.baseurl, bucketname);
        let data = Bucket {
            id: bucketname.clone(),
            client: client.name.clone(),
            _type: buckettype.clone(),
            hostname: client.hostname.clone(),
            created: None,
            name: None,
            last_updated: None,
        };
        client.client.post(&url).json(&data).send()?;
        Ok(())
    }

    pub fn delete_bucket(client: &mut AwClient, bucketname: &String) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", client.baseurl, bucketname);
        client.client.delete(&url).send()?;
        Ok(())
    }

    pub fn get_events(client: &mut AwClient, bucketname: &String) -> Result<Vec<Event>, reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}/events", client.baseurl, bucketname);
        Ok(client.client.get(&url).send()?.json()?)
    }

    pub fn insert_event(client: &mut AwClient, bucketname: &String, event: &Event) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}/events", client.baseurl, bucketname);
        let mut eventlist = Vec::new();
        eventlist.push(event.clone());
        client.client.post(&url).json(&eventlist).send()?;
        Ok(())
    }

    pub fn get_info(client: &mut AwClient) -> Result<Info, reqwest::Error> {
        let url = format!("{}/api/0/info", client.baseurl);
        Ok(client.client.get(&url).send()?.json()?)
    }
}
