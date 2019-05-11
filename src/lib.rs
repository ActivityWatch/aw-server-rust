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
    pub fn new(ip: &str, port: &str, name: &str) -> AwClient {
        let baseurl = String::from(format!("http://{}:{}", ip, port));
        let client = reqwest::Client::new();
        let hostname = gethostname::gethostname().into_string().unwrap();
        return AwClient {
            client: client,
            baseurl: baseurl,
            name: name.to_string(),
            hostname: hostname
        };
    }

    pub fn get_bucket(&self, bucketname: &str) -> Result<Bucket, reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", self.baseurl, bucketname);
        let bucket : Bucket = self.client.get(&url).send()?.json()?;
        Ok(bucket)
    }

    pub fn get_buckets(&self) -> Result<HashMap<String, Bucket>, reqwest::Error> {
        let url = format!("{}/api/0/buckets/", self.baseurl);
        Ok(self.client.get(&url).send()?.json()?)
    }

    pub fn create_bucket(&self, bucketname: &str, buckettype: &str) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", self.baseurl, bucketname);
        let data = Bucket {
            id: bucketname.to_string(),
            client: self.name.clone(),
            _type: buckettype.to_string(),
            hostname: self.hostname.clone(),
            created: None,
            name: None,
            last_updated: None,
        };
        self.client.post(&url).json(&data).send()?;
        Ok(())
    }

    pub fn delete_bucket(&self, bucketname: &str) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}", self.baseurl, bucketname);
        self.client.delete(&url).send()?;
        Ok(())
    }

    pub fn get_events(&self, bucketname: &str) -> Result<Vec<Event>, reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}/events", self.baseurl, bucketname);
        Ok(self.client.get(&url).send()?.json()?)
    }

    pub fn insert_event(&self, bucketname: &str, event: &Event) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/0/buckets/{}/events", self.baseurl, bucketname);
        let mut eventlist = Vec::new();
        eventlist.push(event.clone());
        self.client.post(&url).json(&eventlist).send()?;
        Ok(())
    }

    pub fn get_info(&self) -> Result<Info, reqwest::Error> {
        let url = format!("{}/api/0/info", self.baseurl);
        Ok(self.client.get(&url).send()?.json()?)
    }
}
