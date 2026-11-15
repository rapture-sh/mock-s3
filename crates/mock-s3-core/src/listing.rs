use crate::model::ObjectMeta;

/// S3 caps a single listing page at 1000 entries.
pub const MAX_KEYS_LIMIT: usize = 1000;

#[derive(Debug, Clone)]
pub struct ListRequest {
    pub prefix: String,
    pub delimiter: Option<String>,
    pub max_keys: usize,
    pub start_after: Option<String>,
    pub continuation_token: Option<String>,
}

impl Default for ListRequest {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            delimiter: None,
            max_keys: MAX_KEYS_LIMIT,
            start_after: None,
            continuation_token: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct ListResult {
    pub objects: Vec<ObjectMeta>,
    pub common_prefixes: Vec<String>,
    pub is_truncated: bool,
    pub next_continuation_token: Option<String>,
    pub key_count: usize,
}

/// One page entry, before it is split into objects and common prefixes. Both kinds count
/// towards `max-keys`, and they paginate in one sorted sequence, which is why they are
/// merged here rather than handled separately.
enum Entry {
    Object(ObjectMeta),
    Prefix(String),
}

impl Entry {
    fn sort_key(&self) -> &str {
        match self {
            Entry::Object(meta) => &meta.key,
            Entry::Prefix(prefix) => prefix,
        }
    }
}

/// Apply prefix, delimiter and pagination to a bucket's keys.
///
/// `all` must be sorted by key, which is how every backend returns it.
pub fn list(all: Vec<ObjectMeta>, request: &ListRequest) -> ListResult {
    let max_keys = request.max_keys.min(MAX_KEYS_LIMIT);

    // Where this page starts. A continuation token wins over start-after, matching S3.
    let after = request
        .continuation_token
        .as_deref()
        .or(request.start_after.as_deref());

    let mut entries: Vec<Entry> = Vec::new();
    let mut last_prefix: Option<String> = None;

    for meta in all {
        if !meta.key.starts_with(&request.prefix) {
            continue;
        }

        // Owned rather than borrowed, so `meta` can still be moved in the other arm.
        let rolled_up: Option<String> = request.delimiter.as_deref().and_then(|delimiter| {
            if delimiter.is_empty() {
                return None;
            }
            let rest = &meta.key[request.prefix.len()..];
            rest.find(delimiter)
                .map(|at| meta.key[..request.prefix.len() + at + delimiter.len()].to_string())
        });

        match rolled_up {
            Some(prefix) => {
                // Keys arrive sorted, so every member of a group is contiguous and one
                // comparison against the previous prefix is enough to dedupe.
                if last_prefix.as_deref() != Some(prefix.as_str()) {
                    last_prefix = Some(prefix.clone());
                    entries.push(Entry::Prefix(prefix));
                }
            }
            None => entries.push(Entry::Object(meta)),
        }
    }

    if let Some(after) = after {
        entries.retain(|entry| entry.sort_key() > after);
    }

    let is_truncated = entries.len() > max_keys;
    // `max-keys=0` is legal and returns an empty, truncated page, so this has to cope
    // with there being no last entry to build a token from.
    let next_continuation_token = if is_truncated {
        max_keys
            .checked_sub(1)
            .and_then(|last| entries.get(last))
            .map(|entry| entry.sort_key().to_string())
    } else {
        None
    };
    entries.truncate(max_keys);

    let key_count = entries.len();
    let mut result = ListResult {
        key_count,
        is_truncated,
        next_continuation_token,
        ..Default::default()
    };
    for entry in entries {
        match entry {
            Entry::Object(meta) => result.objects.push(meta),
            Entry::Prefix(prefix) => result.common_prefixes.push(prefix),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn meta(key: &str) -> ObjectMeta {
        ObjectMeta {
            key: key.to_string(),
            size: 0,
            etag: String::new(),
            last_modified: Utc::now(),
            content_type: "binary/octet-stream".into(),
            user_metadata: Default::default(),
            system_headers: Default::default(),
        }
    }

    fn keys(names: &[&str]) -> Vec<ObjectMeta> {
        names.iter().map(|name| meta(name)).collect()
    }

    mod list {
        use super::*;

        #[test]
        fn should_return_every_key_when_no_prefix_or_delimiter_is_given() {
            let result = list(keys(&["a", "b", "c"]), &ListRequest::default());

            assert_eq!(result.objects.len(), 3);
            assert!(result.common_prefixes.is_empty());
            assert!(!result.is_truncated);
        }

        #[test]
        fn should_return_nothing_when_the_prefix_matches_no_key() {
            let request = ListRequest {
                prefix: "zz/".into(),
                ..Default::default()
            };

            let result = list(keys(&["a", "b"]), &request);

            assert_eq!(result.key_count, 0);
        }

        #[test]
        fn should_roll_keys_up_into_common_prefixes_when_a_delimiter_is_given() {
            let request = ListRequest {
                delimiter: Some("/".into()),
                ..Default::default()
            };

            let result = list(keys(&["a/1", "a/2", "b/1", "top"]), &request);

            assert_eq!(result.common_prefixes, vec!["a/", "b/"]);
            assert_eq!(result.objects.len(), 1);
            assert_eq!(result.objects[0].key, "top");
        }

        #[test]
        fn should_count_common_prefixes_towards_max_keys() {
            let request = ListRequest {
                delimiter: Some("/".into()),
                max_keys: 2,
                ..Default::default()
            };

            let result = list(keys(&["a/1", "b/1", "c/1"]), &request);

            assert_eq!(result.key_count, 2);
            assert!(result.is_truncated);
        }

        #[test]
        fn should_resume_after_the_continuation_token() {
            let first = ListRequest {
                max_keys: 2,
                ..Default::default()
            };
            let page_one = list(keys(&["a", "b", "c", "d"]), &first);
            assert!(page_one.is_truncated);

            let second = ListRequest {
                max_keys: 2,
                continuation_token: page_one.next_continuation_token.clone(),
                ..Default::default()
            };
            let page_two = list(keys(&["a", "b", "c", "d"]), &second);

            assert_eq!(page_two.objects.len(), 2);
            assert_eq!(page_two.objects[0].key, "c");
            assert!(!page_two.is_truncated);
        }

        #[test]
        fn should_not_truncate_when_the_page_exactly_fills_max_keys() {
            let request = ListRequest {
                max_keys: 3,
                ..Default::default()
            };

            let result = list(keys(&["a", "b", "c"]), &request);

            assert!(!result.is_truncated);
            assert!(result.next_continuation_token.is_none());
        }

        #[test]
        fn should_treat_a_key_that_is_a_prefix_of_another_key_as_its_own_object() {
            let request = ListRequest {
                delimiter: Some("/".into()),
                ..Default::default()
            };

            let result = list(keys(&["photos", "photos/a.jpg"]), &request);

            assert_eq!(result.objects.len(), 1);
            assert_eq!(result.objects[0].key, "photos");
            assert_eq!(result.common_prefixes, vec!["photos/"]);
        }

        #[test]
        fn should_skip_keys_up_to_start_after() {
            let request = ListRequest {
                start_after: Some("b".into()),
                ..Default::default()
            };

            let result = list(keys(&["a", "b", "c"]), &request);

            assert_eq!(result.objects.len(), 1);
            assert_eq!(result.objects[0].key, "c");
        }
    }
}
