use std::collections::BTreeMap;

use http::HeaderMap;
use mock_s3_core::model::DEFAULT_CONTENT_TYPE;

pub mod bucket;
pub mod object;
pub mod service;

const USER_METADATA_PREFIX: &str = "x-amz-meta-";

/// Pull `x-amz-meta-*` headers out of a request.
///
/// S3 lowercases metadata names, and so does HTTP header handling, so the name stored is
/// already the name that will come back.
pub(crate) fn user_metadata(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str().strip_prefix(USER_METADATA_PREFIX)?;
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), value.to_str().ok()?.to_string()))
        })
        .collect()
}

/// The content type a caller sent, or what S3 assigns when they sent none.
pub(crate) fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_CONTENT_TYPE)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    mod user_metadata {
        use super::*;

        #[test]
        fn should_collect_metadata_headers_without_their_prefix() {
            let mut headers = HeaderMap::new();
            headers.insert("x-amz-meta-owner", "abi".parse().unwrap());
            headers.insert("x-amz-meta-purpose", "test".parse().unwrap());
            headers.insert("content-type", "text/plain".parse().unwrap());

            let found = user_metadata(&headers);

            assert_eq!(found.len(), 2);
            assert_eq!(found.get("owner").map(String::as_str), Some("abi"));
            assert!(!found.contains_key("content-type"));
        }

        #[test]
        fn should_ignore_a_bare_prefix_with_no_name() {
            let mut headers = HeaderMap::new();
            headers.insert("x-amz-meta-", "nothing".parse().unwrap());

            assert!(user_metadata(&headers).is_empty());
        }
    }

    mod content_type {
        use super::*;

        #[test]
        fn should_fall_back_to_the_s3_default_when_none_is_sent() {
            assert_eq!(content_type(&HeaderMap::new()), DEFAULT_CONTENT_TYPE);
        }

        #[test]
        fn should_use_the_header_when_one_is_sent() {
            let mut headers = HeaderMap::new();
            headers.insert("content-type", "image/png".parse().unwrap());

            assert_eq!(content_type(&headers), "image/png");
        }
    }
}
