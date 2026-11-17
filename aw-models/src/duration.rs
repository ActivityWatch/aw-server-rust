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
/// i64, rather than saturating or returning 0 like an `as` cast would. The one exception is 2^63
/// ns, the f64 nearest to i64::MAX ns, which maps to i64::MAX so that every duration round-trips
/// through its f64 serialization.
///
/// Rounds rather than truncates, since truncating would lose a nanosecond for most durations: their
/// decimal representation can't be represented exactly as f64 (515.968 * 1e9 is
/// 515967999999.99994), which then shows up as totals 1 ms too low and 1 ns gaps between touching
/// events (ActivityWatch/aw-server-rust#745).
pub fn seconds_to_nanos(seconds: f64) -> Option<i64> {
    let nanos = (seconds * 1_000_000_000.0).round();
    // i64::MIN (-2^63) is exactly representable as f64. i64::MAX is not: the nearest f64 is 2^63,
    // which is what a duration of i64::MAX (or close to it) serializes as, so accept it and let the
    // cast map it to i64::MAX. Anything beyond is out of range. NaN fails both comparisons.
    if nanos >= i64::MIN as f64 && nanos <= i64::MAX as f64 {
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

        // Exact boundaries: -2^63 ns is the smallest i64, and 2^63 ns is the f64 nearest to the
        // largest; the next f64 above it is out of range
        let two_63 = 2f64.powi(63) / 1e9;
        assert_eq!(seconds_to_nanos(-two_63), Some(i64::MIN));
        assert_eq!(seconds_to_nanos(two_63), Some(i64::MAX));
        let above = f64::from_bits(2f64.powi(63).to_bits() + 1);
        assert_eq!(seconds_to_nanos(above / 1e9), None);
        let below_min = f64::from_bits((-(2f64.powi(63))).to_bits() + 1);
        assert_eq!(seconds_to_nanos(below_min / 1e9), None);
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
    fn test_duration_round_trip_near_limits() {
        // Every duration that can be represented must deserialize from its own serialization
        let mut nanos: Vec<i64> = vec![i64::MAX, i64::MIN, i64::MIN + 1, 0, 1, -1];
        for k in 0..2000 {
            nanos.push(i64::MAX - k * 997);
            nanos.push(i64::MIN + k * 997);
        }
        for ns in nanos {
            let event = Event {
                id: None,
                timestamp: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                duration: chrono::Duration::nanoseconds(ns),
                data: serde_json::Map::new(),
            };
            let json = serde_json::to_string(&event).unwrap();
            let parsed: Event =
                serde_json::from_str(&json).unwrap_or_else(|err| panic!("{ns} ns: {json}: {err}"));
            // f64 has 53 bits of precision, so near the limits (2^63 ns) one ulp is 1024-2048 ns,
            // and converting to seconds and back rounds a couple of times
            let diff = (parsed.duration - event.duration)
                .num_nanoseconds()
                .unwrap();
            assert!(
                diff.abs() <= 4096,
                "{ns} ns came back as {:?}",
                parsed.duration
            );
        }
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
