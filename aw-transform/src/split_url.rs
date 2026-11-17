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
        Some(Value::String(s)) => s.clone(),
        _ => return,
    };
    // Android browser toolbars omit http(s). Parse an authority only when it
    // looks like a host, not a relative path or an opaque URI such as about:blank.
    // A dotted host with a numeric port also parses as a scheme, so check it first.
    let normalized = uri_str
        .trim_matches(|c: char| c <= ' ')
        .replace(['\t', '\n', '\r'], "");
    let authority = normalized
        .strip_prefix("//")
        .unwrap_or(&normalized)
        .split(['/', '\\', '?', '#'])
        .next()
        .unwrap_or("");
    let (host, port) = if authority.starts_with('[') {
        authority
            .find(']')
            .map(|end| authority.split_at(end + 1))
            .unwrap_or(("", ""))
    } else {
        authority
            .find(':')
            .map(|index| authority.split_at(index))
            .unwrap_or((authority, ""))
    };
    let valid_port = port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let candidate = Url::parse(&format!(
        "http://{}",
        normalized.strip_prefix("//").unwrap_or(&normalized)
    ));
    let dotted_host = host.contains('.')
        && candidate
            .as_ref()
            .ok()
            .and_then(Url::host_str)
            .is_some_and(|host| {
                host.contains('.')
                    && host
                        .strip_suffix('.')
                        .unwrap_or(host)
                        .split('.')
                        .all(|label| {
                            !label.is_empty()
                                && label.len() <= 63
                                && label.starts_with(|c: char| c.is_ascii_alphanumeric())
                                && label.ends_with(|c: char| c.is_ascii_alphanumeric())
                                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                        })
            });
    let schemeless = valid_port
        && !authority.contains('@')
        && !authority.chars().any(char::is_whitespace)
        && (dotted_host || host.starts_with('['));
    let parsed = if schemeless {
        candidate
    } else {
        Url::parse(&uri_str)
    };
    let uri = match parsed {
        Ok(uri) => uri,
        Err(_) => return,
    };
    // The synthetic scheme is a parser aid, not evidence about the original URL.
    let protocol = if schemeless { "" } else { uri.scheme() }.to_string();
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
    fn test_split_schemeless_android_urls() {
        for (input, domain, path, params) in [
            (
                "www.example.com/path?query=1#fragment",
                "example.com",
                "/path",
                "query=1",
            ),
            ("example.com", "example.com", "/", ""),
            ("example.com:8080/path", "example.com", "/path", ""),
            ("//www.example.com/path", "example.com", "/path", ""),
            ("WWW.Example.COM/a/../b", "example.com", "/b", ""),
            ("example.com.", "example.com.", "/", ""),
            (" example.com/path ", "example.com", "/path", ""),
            ("exa\tmple.com/path", "example.com", "/path", ""),
            ("[2001:db8::1]:8080/path", "[2001:db8::1]", "/path", ""),
        ] {
            let mut e = Event {
                id: None,
                timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
                duration: Duration::seconds(1),
                data: json_map! {"url": input},
            };
            split_url_event(&mut e);
            assert_eq!(e.data.get("$domain"), Some(&json!(domain)), "{input}");
            assert_eq!(e.data.get("$protocol"), Some(&json!("")), "{input}");
            assert_eq!(e.data.get("$path"), Some(&json!(path)), "{input}");
            assert_eq!(e.data.get("$params"), Some(&json!(params)), "{input}");
            assert_eq!(e.data.get("url"), Some(&json!(input)));
        }
    }

    #[test]
    fn test_schemeless_does_not_reinterpret_relative_or_opaque_urls() {
        for input in [
            "/relative/path",
            "",
            "not a hostname",
            "folder/file.txt",
            "foo..bar/path",
            "example.com../path",
            "foo!.bar/path",
        ] {
            let mut e = Event {
                id: None,
                timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
                duration: Duration::seconds(1),
                data: json_map! {"url": input},
            };
            split_url_event(&mut e);
            assert!(!e.data.contains_key("$domain"), "{input}");
        }
        for input in [
            "about:blank",
            "mailto:user@example.com",
            "custom:foo.bar",
            "com.example://host/path",
        ] {
            let mut e = Event {
                id: None,
                timestamp: DateTime::from_str("2000-01-01T00:00:01Z").unwrap(),
                duration: Duration::seconds(1),
                data: json_map! {"url": input},
            };
            split_url_event(&mut e);
            assert_eq!(
                e.data.get("$domain"),
                Some(&json!(if input == "com.example://host/path" {
                    "host"
                } else {
                    input.split(':').next().unwrap()
                }))
            );
        }
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
