// Max duration of a i64 nanosecond is 2562047.7880152157 hours
// ((2**64)/2)/1000000000/60/60

fn get_nanos(duration: &chrono::Duration) -> f64 {
    (duration.num_nanoseconds().unwrap() as f64) / 1_000_000_000.0
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "chrono::Duration")]
pub struct DurationSerialization(#[serde(getter = "get_nanos")] f64);

// Provide a conversion to construct the remote type.
impl From<DurationSerialization> for chrono::Duration {
    fn from(def: DurationSerialization) -> chrono::Duration {
        chrono::Duration::nanoseconds((def.0 * 1_000_000_000.0) as i64)
    }
}
