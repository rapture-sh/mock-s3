use crate::model::ObjectMeta;
use crate::versioning::ObjectVersion;

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

/// Which common prefix a key rolls up into, if the delimiter puts it in one.
///
/// Shared by both listings so a key groups the same way whichever one asked.
fn rolled_up(key: &str, prefix: &str, delimiter: Option<&str>) -> Option<String> {
    let delimiter = delimiter?;
    if delimiter.is_empty() {
        return None;
    }

    let rest = key.get(prefix.len()..)?;
    rest.find(delimiter)
        .map(|at| key[..prefix.len() + at + delimiter.len()].to_string())
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
        let group = rolled_up(&meta.key, &request.prefix, request.delimiter.as_deref());

        match group {
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

#[derive(Debug, Clone)]
pub struct VersionListRequest {
    pub prefix: String,
    pub delimiter: Option<String>,
    pub max_keys: usize,
    /// Resume after this key, or — with `version_id_marker` — after one version of it.
    pub key_marker: Option<String>,
    pub version_id_marker: Option<String>,
}

impl Default for VersionListRequest {
    fn default() -> Self {
        Self {
            prefix: String::new(),
            delimiter: None,
            max_keys: MAX_KEYS_LIMIT,
            key_marker: None,
            version_id_marker: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct VersionListResult {
    pub versions: Vec<ObjectVersion>,
    pub common_prefixes: Vec<String>,
    pub is_truncated: bool,
    pub next_key_marker: Option<String>,
    pub next_version_id_marker: Option<String>,
}

/// One entry of a versions page. As with objects, versions and common prefixes share one
/// `max-keys` budget and one sorted sequence, so they paginate together.
// Boxing the version would trade one allocation per entry, on the common path, for
// memory saved only on the rare one: a page holds at most `MAX_KEYS_LIMIT` entries and
// almost all of them are versions, not prefixes.
#[allow(clippy::large_enum_variant)]
enum VersionEntry {
    Version(ObjectVersion),
    Prefix(String),
}

impl VersionEntry {
    fn key(&self) -> &str {
        match self {
            Self::Version(version) => &version.key,
            Self::Prefix(prefix) => prefix,
        }
    }

    /// `None` for a common prefix, which has no version to resume from — and that is
    /// what S3 reports as the next version id marker when a page ends on one.
    fn version_id(&self) -> Option<&str> {
        match self {
            Self::Version(version) => Some(&version.version_id),
            Self::Prefix(_) => None,
        }
    }
}

/// Apply prefix, delimiter and pagination to every version in a bucket.
///
/// `all` must be ordered by key and then newest version first, which is what
/// [`collect_versions`](crate::versioning::collect_versions) produces. A versions page
/// resumes from a key *and* a version id, because one key can hold more versions than a
/// page can carry.
pub fn list_versions(all: Vec<ObjectVersion>, request: &VersionListRequest) -> VersionListResult {
    let max_keys = request.max_keys.min(MAX_KEYS_LIMIT);

    let mut entries: Vec<VersionEntry> = Vec::new();
    let mut last_prefix: Option<String> = None;

    for version in all {
        if !version.key.starts_with(&request.prefix) {
            continue;
        }

        match rolled_up(&version.key, &request.prefix, request.delimiter.as_deref()) {
            Some(prefix) => {
                // Keys arrive sorted, so a group is contiguous and one comparison
                // against the previous prefix is enough to dedupe — every version of
                // every key in the group collapses into the one entry.
                if last_prefix.as_deref() != Some(prefix.as_str()) {
                    last_prefix = Some(prefix.clone());
                    entries.push(VersionEntry::Prefix(prefix));
                }
            }
            None => entries.push(VersionEntry::Version(version)),
        }
    }

    let after_key = |key_marker: &str| {
        entries
            .iter()
            .position(|entry| entry.key() > key_marker)
            .unwrap_or(entries.len())
    };

    let start = match (
        request.key_marker.as_deref(),
        request.version_id_marker.as_deref(),
    ) {
        (None, _) => 0,
        (Some(key_marker), None) => after_key(key_marker),
        (Some(key_marker), Some(version_marker)) => entries
            .iter()
            .position(|entry| {
                entry.key() == key_marker && entry.version_id() == Some(version_marker)
            })
            .map(|at| at + 1)
            // The version the client is resuming from has since been deleted. Moving on
            // to the next key is what keeps it going forward instead of looping on a
            // page it can never reach.
            .unwrap_or_else(|| after_key(key_marker)),
    };

    entries.drain(..start);

    let is_truncated = entries.len() > max_keys;
    entries.truncate(max_keys);

    let (next_key_marker, next_version_id_marker) = match entries.last() {
        Some(entry) if is_truncated => (
            Some(entry.key().to_string()),
            entry.version_id().map(str::to_string),
        ),
        _ => (None, None),
    };

    let mut result = VersionListResult {
        is_truncated,
        next_key_marker,
        next_version_id_marker,
        ..Default::default()
    };
    for entry in entries {
        match entry {
            VersionEntry::Version(version) => result.versions.push(version),
            VersionEntry::Prefix(prefix) => result.common_prefixes.push(prefix),
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
            version_id: None,
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

    mod list_versions {
        use super::*;

        /// Entries in the order `collect_versions` produces: key ascending, newest
        /// version of each key first.
        fn versions(entries: &[(&str, &str)]) -> Vec<ObjectVersion> {
            entries
                .iter()
                .map(|(key, version_id)| ObjectVersion {
                    key: (*key).to_string(),
                    version_id: (*version_id).to_string(),
                    is_latest: false,
                    last_modified: Utc::now(),
                    meta: Some(meta(key)),
                })
                .collect()
        }

        fn marker(key: &str, version_id: &str) -> ObjectVersion {
            ObjectVersion {
                key: key.to_string(),
                version_id: version_id.to_string(),
                is_latest: false,
                last_modified: Utc::now(),
                meta: None,
            }
        }

        fn listed(result: &VersionListResult) -> Vec<(&str, &str)> {
            result
                .versions
                .iter()
                .map(|version| (version.key.as_str(), version.version_id.as_str()))
                .collect()
        }

        #[test]
        fn should_return_every_version_when_nothing_narrows_it() {
            let all = versions(&[("a", "v2"), ("a", "v1"), ("b", "v1")]);

            let result = list_versions(all, &VersionListRequest::default());

            assert_eq!(listed(&result), vec![("a", "v2"), ("a", "v1"), ("b", "v1")]);
            assert!(!result.is_truncated);
            assert_eq!(result.next_key_marker, None);
        }

        #[test]
        fn should_keep_only_the_versions_under_a_prefix() {
            let all = versions(&[("photos/one", "v1"), ("notes", "v1")]);
            let request = VersionListRequest {
                prefix: "photos/".into(),
                ..Default::default()
            };

            let result = list_versions(all, &request);

            assert_eq!(listed(&result), vec![("photos/one", "v1")]);
        }

        /// Every version of every key in a group collapses into one common prefix, not
        /// one per version.
        #[test]
        fn should_roll_up_all_versions_of_a_group_into_one_common_prefix() {
            let all = versions(&[
                ("photos/one", "v2"),
                ("photos/one", "v1"),
                ("photos/two", "v1"),
                ("notes", "v1"),
            ]);
            let request = VersionListRequest {
                delimiter: Some("/".into()),
                ..Default::default()
            };

            let result = list_versions(all, &request);

            assert_eq!(result.common_prefixes, vec!["photos/"]);
            assert_eq!(listed(&result), vec![("notes", "v1")]);
        }

        #[test]
        fn should_report_both_markers_when_a_page_is_truncated() {
            let all = versions(&[("a", "v2"), ("a", "v1"), ("b", "v1")]);
            let request = VersionListRequest {
                max_keys: 2,
                ..Default::default()
            };

            let result = list_versions(all, &request);

            assert!(result.is_truncated);
            assert_eq!(result.next_key_marker.as_deref(), Some("a"));
            assert_eq!(result.next_version_id_marker.as_deref(), Some("v1"));
        }

        /// A key can hold more versions than a page can carry, so resuming has to land
        /// inside the key rather than skipping the rest of it.
        #[test]
        fn should_resume_inside_a_key_from_both_markers() {
            let all = versions(&[("a", "v3"), ("a", "v2"), ("a", "v1"), ("b", "v1")]);
            let request = VersionListRequest {
                key_marker: Some("a".into()),
                version_id_marker: Some("v2".into()),
                ..Default::default()
            };

            let result = list_versions(all, &request);

            assert_eq!(listed(&result), vec![("a", "v1"), ("b", "v1")]);
        }

        #[test]
        fn should_skip_a_whole_key_when_only_a_key_marker_is_given() {
            let all = versions(&[("a", "v2"), ("a", "v1"), ("b", "v1")]);
            let request = VersionListRequest {
                key_marker: Some("a".into()),
                ..Default::default()
            };

            let result = list_versions(all, &request);

            assert_eq!(listed(&result), vec![("b", "v1")]);
        }

        /// The version a client is resuming from can be deleted between pages. Moving on
        /// to the next key keeps it going forward instead of looping on a page it can
        /// never reach.
        #[test]
        fn should_move_on_to_the_next_key_when_the_marker_version_is_gone() {
            let all = versions(&[("a", "v3"), ("a", "v1"), ("b", "v1")]);
            let request = VersionListRequest {
                key_marker: Some("a".into()),
                version_id_marker: Some("v2".into()),
                ..Default::default()
            };

            let result = list_versions(all, &request);

            assert_eq!(listed(&result), vec![("b", "v1")]);
        }

        #[test]
        fn should_page_through_every_version_without_repeating_one() {
            let all = versions(&[("a", "v2"), ("a", "v1"), ("b", "v2"), ("b", "v1")]);

            let mut seen: Vec<(String, String)> = Vec::new();
            let mut request = VersionListRequest {
                max_keys: 1,
                ..Default::default()
            };
            for _ in 0..10 {
                let page = list_versions(all.clone(), &request);
                for version in &page.versions {
                    seen.push((version.key.clone(), version.version_id.clone()));
                }
                if !page.is_truncated {
                    break;
                }
                request.key_marker = page.next_key_marker;
                request.version_id_marker = page.next_version_id_marker;
            }

            assert_eq!(
                seen,
                vec![
                    ("a".to_string(), "v2".to_string()),
                    ("a".to_string(), "v1".to_string()),
                    ("b".to_string(), "v2".to_string()),
                    ("b".to_string(), "v1".to_string()),
                ]
            );
        }

        #[test]
        fn should_keep_a_delete_marker_in_the_sequence() {
            let mut all = versions(&[("a", "v1")]);
            all.insert(0, marker("a", "v2"));

            let result = list_versions(all, &VersionListRequest::default());

            assert_eq!(listed(&result), vec![("a", "v2"), ("a", "v1")]);
            assert!(result.versions[0].is_delete_marker());
            assert!(!result.versions[1].is_delete_marker());
        }

        /// A page that ends on a common prefix has no version to resume from, which is
        /// why the version id marker is an `Option` rather than always a version.
        #[test]
        fn should_report_no_version_marker_when_a_page_ends_on_a_prefix() {
            let all = versions(&[("photos/one", "v1"), ("zzz", "v1")]);
            let request = VersionListRequest {
                delimiter: Some("/".into()),
                max_keys: 1,
                ..Default::default()
            };

            let result = list_versions(all, &request);

            assert!(result.is_truncated);
            assert_eq!(result.next_key_marker.as_deref(), Some("photos/"));
            assert_eq!(result.next_version_id_marker, None);
        }
    }
}
