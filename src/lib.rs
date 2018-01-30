extern crate restson;

#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use serde_json::{Value, Map};

use std::vec::Vec;

use restson::{RestClient,RestPath,Error};

#[derive(Deserialize,Debug)]
pub struct Bucket {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub name: Value,
    #[serde(rename = "type")]
    pub _type: String,
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub last_updated: String,
}

#[derive(Deserialize,Debug)]
#[serde(untagged)]
pub enum BucketList {
    // TODO: Inherit Bucket enum
    // Might be harder than expected
    // https://serde.rs/deserialize-map.html
    Object(Map<String, Value>)
}

#[derive(Serialize)]
pub struct CreateBucket {
    #[serde(default)]
    pub client: String,
    #[serde(rename = "type")]
    pub _type: String,
    #[serde(default)]
    pub hostname: String
}

pub struct DeleteBucket {}

#[derive(Serialize, Deserialize, Debug)]
pub struct Event {
    // FIXME: Make optional somehow
    #[serde(skip)]
    pub id: i64,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub data: Map<String, Value>,
}

#[derive(Deserialize,Debug)]
#[serde(untagged)]
pub enum EventList {
    Array(Vec<Event>)
}
#[derive(Deserialize)]
pub struct Info {
  pub hostname: String,
  pub testing: bool,
}

impl RestPath<String> for Bucket {
    fn get_path(bucket: String) -> Result<String,Error> { Ok(format!("/api/0/buckets/{}", bucket)) }
}

impl RestPath<()> for BucketList {
    fn get_path(_: ()) -> Result<String,Error> { Ok(String::from("/api/0/buckets/")) }
}

impl RestPath<String> for CreateBucket {
    fn get_path(bucket: String) -> Result<String,Error> { Ok(format!("/api/0/buckets/{}", bucket)) }
}

impl RestPath<String> for DeleteBucket {
    fn get_path(bucket: String) -> Result<String,Error> { Ok(format!("/api/0/buckets/{}", bucket)) }
}

impl RestPath<String> for Event {
    fn get_path(bucket: String) -> Result<String,Error> { Ok(format!("/api/0/buckets/{}/events", bucket)) }
}

impl RestPath<String> for EventList {
    fn get_path(bucket: String) -> Result<String,Error> { Ok(format!("/api/0/buckets/{}/events", bucket)) }
}

impl RestPath<()> for Info {
    fn get_path(_: ()) -> Result<String,Error> { Ok(String::from("/api/0/info")) }
}

pub struct AwClient {
    client: RestClient,
    pub name: String,
    pub hostname: String
}

impl AwClient {
    pub fn new(ip: String, port: String, name: String) -> AwClient {
        let ipport = String::from(format!("http://{}:{}", ip, port));
        let client = RestClient::new(&ipport).unwrap();
        let hostname = String::from("Unknown"); // TODO: Implement this
        return AwClient {
            client: client,
            name: name,
            hostname: hostname
        };
    }

    pub fn get_bucket(client: &mut AwClient, bucketname: &String) -> Bucket {
        return client.client.get(bucketname.clone()).unwrap();
    }

    pub fn get_buckets(client: &mut AwClient) -> Result<BucketList, Error> {
        return client.client.get(());
    }

    pub fn create_bucket(client: &mut AwClient, bucketname: &String, buckettype: &String) -> Result<(), Error> {
        let hostname = String::from("Unknown"); // TODO: Implement this
        let data = CreateBucket {
            client: client.name.clone(),
            _type: buckettype.clone(),
            hostname: hostname.clone()
        };
        // Ignore 304 responses (if bucket already exists)
        if let Err(e) = client.client.post(bucketname.clone(), &data) {
            match e {
                Error::HttpError(304) => (),
                _ => return Err(e),
            };
        }
        Ok(())
    }

    pub fn delete_bucket(client: &mut AwClient, bucketname: &String) -> Result<(), Error> {
        /* NOTE: needs restson git master to work */
        return client.client.delete::<String, DeleteBucket>(bucketname.clone());
    }

    pub fn get_events(client: &mut AwClient, bucketname: &String) -> Result<EventList, Error> {
        return client.client.get(bucketname.clone());
    }

    pub fn insert_event(client: &mut AwClient, bucketname: &String, event: &Event) -> Result<(), Error> {
        return client.client.post(bucketname.clone(), event.clone());
    }

    pub fn get_info(client: &mut AwClient) -> Info {
        return client.client.get(()).unwrap();
    }
}
