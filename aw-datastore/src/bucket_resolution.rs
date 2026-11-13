use aw_models::Bucket;

use crate::DatastoreError;

/// Resolve an existing bucket without deriving device identity from its name.
/// Exact names take precedence over hostname compatibility aliases. A caller
/// that knows the device must supply it; an unscoped ambiguity never picks one.
pub(crate) fn resolve_bucket<'a>(
    buckets: impl IntoIterator<Item = &'a Bucket>,
    name: &str,
    device_id: Option<&str>,
) -> Result<Bucket, DatastoreError> {
    let mut exact = Vec::new();
    let mut aliases = Vec::new();
    for bucket in buckets {
        if device_id.is_some_and(|device| bucket.device_id != device) {
            continue;
        }
        if bucket.id == name {
            exact.push(bucket);
        } else if !bucket.hostname.is_empty()
            && bucket.hostname != "unknown"
            && bucket.hostname != "Unknown"
            && bucket.hostname != "!local"
            && (bucket.id == format!("{name}_{}", bucket.hostname)
                || name == format!("{}_{}", bucket.id, bucket.hostname))
        {
            aliases.push(bucket);
        }
    }
    let candidates = if exact.is_empty() { aliases } else { exact };
    match candidates.as_slice() {
        [] => Err(DatastoreError::NoSuchBucket(name.to_owned())),
        [bucket] => Ok((*bucket).clone()),
        _ => {
            let mut identities: Vec<_> = candidates
                .iter()
                .map(|bucket| (bucket.device_id.clone(), bucket.id.clone()))
                .collect();
            identities.sort();
            Err(DatastoreError::AmbiguousBucket(name.to_owned(), identities))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bucket(name: &str, hostname: &str, device: &str) -> Bucket {
        serde_json::from_value(json!({
            "id": name, "hostname": hostname, "device_id": device,
            "type": "test", "client": "test"
        }))
        .unwrap()
    }

    #[test]
    fn explicit_device_never_falls_back_to_a_peer() {
        let buckets = [bucket("window", "host", "peer")];
        assert!(matches!(
            resolve_bucket(&buckets, "window", Some("local")),
            Err(DatastoreError::NoSuchBucket(_))
        ));
        assert_eq!(
            resolve_bucket(&buckets, "window", Some("peer"))
                .unwrap()
                .device_id,
            "peer"
        );
    }

    #[test]
    fn duplicate_names_require_a_device_even_with_a_local_row() {
        let buckets = [
            bucket("window", "host", "local"),
            bucket("window", "host", "peer"),
        ];
        assert!(matches!(
            resolve_bucket(&buckets, "window", None),
            Err(DatastoreError::AmbiguousBucket(_, _))
        ));
        assert_eq!(
            resolve_bucket(&buckets, "window", Some("peer"))
                .unwrap()
                .device_id,
            "peer"
        );
    }

    #[test]
    fn exact_name_wins_and_unknown_hostnames_are_not_aliases() {
        let buckets = [
            bucket("window", "host", "local"),
            bucket("window_host", "host", "peer"),
        ];
        assert_eq!(
            resolve_bucket(&buckets, "window", None).unwrap().device_id,
            "local"
        );
        for host in ["", "unknown", "Unknown", "!local"] {
            let buckets = [bucket(&format!("window_{host}"), host, "local")];
            assert!(matches!(
                resolve_bucket(&buckets, "window", None),
                Err(DatastoreError::NoSuchBucket(_))
            ));
        }
    }
}
