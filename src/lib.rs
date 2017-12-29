extern crate restson;

#[macro_use]
extern crate serde_derive;

use restson::{RestClient,RestPath,Error};

#[derive(Deserialize)]
struct BucketList {}

#[derive(Deserialize)]
pub struct Bucket {
    #[serde(default)]
    pub id: String,
    #[serde(skip)]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub _type: String,
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub created: String,
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

#[derive(Deserialize)]
pub struct Info {
  pub hostname: String,
  pub testing: bool,
}

impl RestPath<()> for BucketList {
    fn get_path(_: ()) -> Result<String,Error> { Ok(String::from("/api/0/buckets/")) }
}

impl RestPath<String> for Bucket {
    fn get_path(bucket: String) -> Result<String,Error> { Ok(format!("/api/0/buckets/{}", bucket)) }
}

impl RestPath<String> for CreateBucket {
    fn get_path(bucket: String) -> Result<String,Error> { Ok(format!("/api/0/buckets/{}", bucket)) }
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

    pub fn create_bucket(client: &mut AwClient, bucketname: &String, buckettype: &String) -> bool {
        let hostname = String::from("Unknown"); // TODO: Implement this
        let data = CreateBucket {
            client: client.name.clone(),
            _type: buckettype.clone(),
            hostname: hostname.clone()
        };
        client.client.post(bucketname.clone(), &data);
        return true;
    }

    pub fn get_info(client: &mut AwClient) -> Info {
        return client.client.get(()).unwrap();
    }
}
