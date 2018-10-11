use chrono;

// TODO: What is the max duration of a i64 nanosecond?

fn get_nanos(duration: &chrono::Duration) -> f64 {
    return (duration.num_nanoseconds().unwrap() as f64)/1000000000.0
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "chrono::Duration")]
pub struct DurationSerialization (
    #[serde(getter = "get_nanos")]
    f64
);

// Provide a conversion to construct the remote type.
impl From<DurationSerialization> for chrono::Duration {
    fn from(def: DurationSerialization) -> chrono::Duration {
        chrono::Duration::nanoseconds((def.0/1000000000.0) as i64)
    }
}
