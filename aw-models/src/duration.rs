use serde::{Deserialize, Serialize};

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
        chrono::Duration::nanoseconds(seconds_to_nanos(def.0))
    }
}

/// Converts a duration in seconds to nanoseconds, rounding to the nearest nanosecond.
///
/// Truncating would lose a nanosecond for most durations, since their decimal representation
/// can't be represented exactly as f64 (515.968 * 1e9 is 515967999999.99994), which then shows up
/// as totals 1 ms too low and 1 ns gaps between touching events (ActivityWatch/aw-server-rust#745).
pub fn seconds_to_nanos(seconds: f64) -> i64 {
    (seconds * 1_000_000_000.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::seconds_to_nanos;
    use crate::Event;

    #[test]
    fn test_seconds_to_nanos_rounds() {
        assert_eq!(seconds_to_nanos(515.968), 515_968_000_000);
        assert_eq!(seconds_to_nanos(16.964), 16_964_000_000);
        assert_eq!(seconds_to_nanos(0.0), 0);
        assert_eq!(seconds_to_nanos(1.5e-9), 2);
        assert_eq!(seconds_to_nanos(-2.25), -2_250_000_000);
    }

    #[test]
    fn test_deserialize_duration_exact() {
        // Repro from ActivityWatch/aw-server-rust#745
        let event: Event = serde_json::from_str(
            r#"{"timestamp": "2026-01-01T10:00:00Z", "duration": 515.968, "data": {}}"#,
        )
        .unwrap();
        assert_eq!(event.duration.num_nanoseconds(), Some(515_968_000_000));
        // and it serializes back to the same number
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["duration"], serde_json::json!(515.968));
    }
}
