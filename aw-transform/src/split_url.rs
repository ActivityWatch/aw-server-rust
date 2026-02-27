use aw_models::Event;
use serde_json::value::Value;

/// Adds $protocol, $domain, $path and $params keys for events with an "url" key
///
/// But it only adds the generated field if it exists, for example if a url does not have a path
/// the path value will not be set at all.
///
/// # Example
/// ```ignore
/// input:  {
///           "data": {
///             "url": "http://google.com/test"
///           }
///         }
/// output: {
///           "data": {
///             "$domain": "google.com",
///             "$path": "/test",
///             "$protocol": "http"
///           }
///         }
/// ```
pub fn split_url_event(event: &mut Event) {
    use url::Url;

    let uri_str = match event.data.get("url") {
        None => return,
        Some(val) => match val {
            Value::String(s) => s.clone(),
            _ => return,
        },
    };
    let uri = match Url::parse(&uri_str) {
        Ok(uri) => uri,
        Err(_) => return,
    };
    // Protocol
    let protocol = uri.scheme().to_string();
    event
        .data
        .insert("$protocol".to_string(), Value::String(protocol));
    // Domain
    // For URLs without a host (e.g. file://, about:), fall back to the scheme
    // so they don't all cluster as an empty string in "Top Browser Domains".
    let domain = match uri.host_str() {
        Some(domain) => domain.trim_start_matches("www.").to_string(),
        None => uri.scheme().to_string(),
    };
    event
        .data
        .insert("$domain".to_string(), Value::String(domain));

    // Path
    let path = uri.path().to_string();
    event.data.insert("$path".to_string(), Value::String(path));

    // Params
    let params = match uri.query() {
        Some(query) => query.to_string(),
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

    #[test]
    fn test_split_url_file_scheme() {
        let mut e = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"url": "file:///home/user/document.pdf"},
        };
        split_url_event(&mut e);
        assert_eq!(e.data.get("$protocol"), Some(&json!("file")));
        assert_eq!(e.data.get("$domain"), Some(&json!("file")));
        assert_eq!(e.data.get("$path"), Some(&json!("/home/user/document.pdf")));
    }

    #[test]
    fn test_split_url_about_scheme() {
        let mut e = Event {
            id: None,
            timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
            duration: Duration::seconds(1),
            data: json_map! {"url": "about:blank"},
        };
        split_url_event(&mut e);
        assert_eq!(e.data.get("$protocol"), Some(&json!("about")));
        assert_eq!(e.data.get("$domain"), Some(&json!("about")));
    }
}
