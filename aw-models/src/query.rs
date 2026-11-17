use serde::{Deserialize, Serialize};

use crate::TimeInterval;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Query {
    //#[serde(with = "DurationSerialization")]
    pub timeperiods: Vec<TimeInterval>,
    pub query: Vec<String>,
}

#[test]
fn test_query_serialization_roundtrip() {
    for payload in [
        serde_json::json!({
            "timeperiods": [
                "2000-01-01T00:00:00+00:00/2000-01-02T00:00:00+00:00",
                "2000-01-02T00:00:00.123456789+00:00/2000-01-03T00:00:00+00:00"
            ],
            "query": ["events = query_bucket(\"test\");\nRETURN = events;", "RETURN = [];" ]
        }),
        serde_json::json!({"timeperiods": [], "query": []}),
    ] {
        let query: Query = serde_json::from_value(payload.clone()).unwrap();
        let encoded = serde_json::to_string(&query).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
            payload
        );
        let decoded: Query = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.query, query.query);
        assert_eq!(decoded.timeperiods.len(), query.timeperiods.len());
        for (actual, expected) in decoded.timeperiods.iter().zip(&query.timeperiods) {
            assert_eq!(actual.start(), expected.start());
            assert_eq!(actual.end(), expected.end());
        }
    }
}
