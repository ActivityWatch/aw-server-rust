use aw_models::Event;
use serde_json::value::Value;

pub fn split_url_event(event: &mut Event) {
    use rocket::http::uri::Absolute;
    let uri_str = match event.data.get("url") {
        None => return,
        Some(val) => match val {
            Value::String(s) => s.clone(),
            _ => return,
        },
    };
    let uri = match Absolute::parse(&uri_str) {
        Ok(uri) => uri,
        Err(_) => return,
    };
    // Protocol
    let protocol = uri.scheme().to_string();
    event
        .data
        .insert("$protocol".to_string(), Value::String(protocol));
    // Domain
    let domain = match uri.authority() {
        Some(authority) => authority.host().trim_start_matches("www.").to_string(),
        None => "".to_string(),
    };
    event
        .data
        .insert("$domain".to_string(), Value::String(domain));
    // Path
    let path = match uri.origin() {
        Some(origin) => origin.path().to_string(),
        None => "".to_string(),
    };
    event.data.insert("$path".to_string(), Value::String(path));
    // Params
    // TODO: What's the difference between params and query?
    let params = match uri.origin() {
        Some(origin) => match origin.query() {
            Some(query) => query.to_string(),
            None => "".to_string(),
        },
        None => "".to_string(),
    };
    event
        .data
        .insert("$params".to_string(), Value::String(params));

    // TODO: aw-server-python also has options and identifier
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::DateTime;
    use chrono::Duration;
    use serde_json::json;

    use aw_models::Event;

    use super::split_url_event;

    #[test]
    fn test_split_url_events() {
        let mut e1 = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"url": "http://www.google.com/path?query=1"},
        };
        split_url_event(&mut e1);
        assert_eq!(
            e1.data,
            json_map! {
                "url": json!("http://www.google.com/path?query=1"),
                "$protocol": json!("http"),
                "$domain": json!("google.com"),
                "$path": json!("/path"),
                "$params": json!("query=1")
            }
        );
    }
}
