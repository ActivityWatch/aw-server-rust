use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// Max duration of a i64 nanosecond is 2562047.7880152157 hours
// ((2**64)/2)/1000000000/60/60

fn get_nanos(duration: &chrono::Duration) -> f64 {
    (duration.num_nanoseconds().unwrap() as f64) / 1_000_000_000.0
}

/// (De)serializes a `chrono::Duration` as a floating point number of seconds.
///
/// Used with `#[serde(with = "DurationSerialization")]`. The wire format is the same as that of a
/// newtype struct around the f64 (a plain number in JSON).
pub struct DurationSerialization;

#[derive(Serialize, Deserialize)]
#[serde(rename = "DurationSerialization")]
struct Seconds(f64);

impl DurationSerialization {
    pub fn serialize<S: Serializer>(
        duration: &chrono::Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Seconds(get_nanos(duration)).serialize(serializer)
    }

    /// Fails for durations that are not finite or don't fit in i64 nanoseconds (about ±292
    /// years), instead of silently turning them into a different duration.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<chrono::Duration, D::Error> {
        let Seconds(seconds) = Seconds::deserialize(deserializer)?;
        seconds_to_nanos(seconds)
            .map(chrono::Duration::nanoseconds)
            .ok_or_else(|| {
                D::Error::custom(format!(
                    "duration {seconds} s is not finite or out of range (at most ±{MAX_SECONDS:.0} s)"
                ))
            })
    }
}

/// Approximate largest duration in seconds that fits in i64 nanoseconds, for error messages.
const MAX_SECONDS: f64 = i64::MAX as f64 / 1_000_000_000.0;

/// Converts a duration in seconds to nanoseconds, rounding to the nearest nanosecond.
///
/// Returns `None` if `seconds` is not finite (NaN or infinite) or the result doesn't fit in an
/// i64, rather than saturating or returning 0 like an `as` cast would.
///
/// Rounds rather than truncates, since truncating would lose a nanosecond for most durations: their
/// decimal representation can't be represented exactly as f64 (515.968 * 1e9 is
/// 515967999999.99994), which then shows up as totals 1 ms too low and 1 ns gaps between touching
/// events (ActivityWatch/aw-server-rust#745).
pub fn seconds_to_nanos(seconds: f64) -> Option<i64> {
    let nanos = (seconds * 1_000_000_000.0).round();
    // i64::MIN (-2^63) is exactly representable as f64 and in range. i64::MAX as f64 rounds up to
    // 2^63, which is out of range, so the upper bound is exclusive. NaN fails both comparisons.
    if nanos >= i64::MIN as f64 && nanos < i64::MAX as f64 {
        Some(nanos as i64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::seconds_to_nanos;
    use crate::Event;

    #[test]
    fn test_seconds_to_nanos_rounds() {
        assert_eq!(seconds_to_nanos(515.968), Some(515_968_000_000));
        assert_eq!(seconds_to_nanos(16.964), Some(16_964_000_000));
        assert_eq!(seconds_to_nanos(0.0), Some(0));
        assert_eq!(seconds_to_nanos(-0.0), Some(0));
        assert_eq!(seconds_to_nanos(1.5e-9), Some(2));
        assert_eq!(seconds_to_nanos(-2.25), Some(-2_250_000_000));
    }

    #[test]
    fn test_seconds_to_nanos_out_of_range() {
        assert_eq!(seconds_to_nanos(f64::NAN), None);
        assert_eq!(seconds_to_nanos(f64::INFINITY), None);
        assert_eq!(seconds_to_nanos(f64::NEG_INFINITY), None);
        assert_eq!(seconds_to_nanos(1e12), None);
        assert_eq!(seconds_to_nanos(-1e12), None);

        // Exact boundaries: -2^63 ns is the smallest i64, 2^63 ns is one past the largest
        let two_63 = 2f64.powi(63) / 1e9;
        assert_eq!(seconds_to_nanos(-two_63), Some(i64::MIN));
        assert_eq!(seconds_to_nanos(two_63), None);
        // The largest f64 below 2^63 ns still converts, to an exactly representable i64
        let below = f64::from_bits((i64::MAX as f64).to_bits() - 1);
        assert_eq!(seconds_to_nanos(below / 1e9), Some(below as i64));
        // About 292 years is fine
        assert_eq!(
            seconds_to_nanos(9_000_000_000.0),
            Some(9_000_000_000_000_000_000)
        );
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

    #[test]
    fn test_deserialize_duration_integer_and_default() {
        let event: Event = serde_json::from_str(
            r#"{"timestamp": "2026-01-01T10:00:00Z", "duration": 5, "data": {}}"#,
        )
        .unwrap();
        assert_eq!(event.duration.num_seconds(), 5);
        let event: Event =
            serde_json::from_str(r#"{"timestamp": "2026-01-01T10:00:00Z", "data": {}}"#).unwrap();
        assert_eq!(event.duration.num_seconds(), 0);
    }

    #[test]
    fn test_deserialize_duration_out_of_range() {
        for duration in ["1e12", "-1e12", "1e400"] {
            let json = format!(
                r#"{{"timestamp": "2026-01-01T10:00:00Z", "duration": {duration}, "data": {{}}}}"#
            );
            let err = serde_json::from_str::<Event>(&json).unwrap_err();
            assert!(
                err.to_string().contains("out of range"),
                "{duration}: {err}"
            );
        }
    }
}
