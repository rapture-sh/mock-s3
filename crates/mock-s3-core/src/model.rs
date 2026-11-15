use std::collections::BTreeMap;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The default content type S3 assigns when a caller sends none.
pub const DEFAULT_CONTENT_TYPE: &str = "binary/octet-stream";

/// S3 caps object keys at 1024 bytes of UTF-8.
pub const MAX_KEY_LEN: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketInfo {
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub key: String,
    pub size: u64,
    /// Hex MD5 of the payload, unquoted. Callers that put it on the wire add the quotes.
    pub etag: String,
    pub last_modified: DateTime<Utc>,
    pub content_type: String,
    #[serde(default)]
    pub user_metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ObjectData {
    pub meta: ObjectMeta,
    pub body: Bytes,
}

/// Hex MD5 of a payload, which is what S3 reports as the ETag for a single-part upload.
pub fn compute_etag(body: &[u8]) -> String {
    use md5::{Digest, Md5};

    let mut hasher = Md5::new();
    hasher.update(body);
    hex::encode(hasher.finalize())
}

/// Validate a bucket name against the real S3 rules, so a name that fails against AWS
/// also fails here instead of passing locally.
pub fn validate_bucket_name(name: &str) -> Result<()> {
    let invalid = |why: &str| Err(Error::InvalidBucketName(format!("{name}: {why}")));

    if name.len() < 3 || name.len() > 63 {
        return invalid("must be between 3 and 63 characters");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
    {
        return invalid("may only contain lowercase letters, digits, hyphens and dots");
    }
    let first = name.as_bytes()[0];
    let last = name.as_bytes()[name.len() - 1];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || !(last.is_ascii_lowercase() || last.is_ascii_digit())
    {
        return invalid("must start and end with a letter or digit");
    }
    if name.contains("..") {
        return invalid("may not contain two adjacent dots");
    }
    if name
        .split('.')
        .all(|part| !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit()))
        && name.split('.').count() == 4
    {
        return invalid("may not be formatted as an IP address");
    }
    if name.starts_with("xn--") || name.ends_with("-s3alias") {
        return invalid("uses a reserved prefix or suffix");
    }
    Ok(())
}

/// Validate an object key. S3 accepts almost anything here, so this only rejects what
/// is genuinely illegal rather than what is merely awkward.
pub fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(Error::InvalidKey("key may not be empty".into()));
    }
    if key.len() > MAX_KEY_LEN {
        return Err(Error::InvalidKey(format!(
            "key is {} bytes, the limit is {MAX_KEY_LEN}",
            key.len()
        )));
    }
    if key.contains('\0') {
        return Err(Error::InvalidKey("key may not contain a NUL byte".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    mod validate_bucket_name {
        use super::*;

        #[test]
        fn should_accept_a_plain_lowercase_name() {
            assert!(validate_bucket_name("my-bucket").is_ok());
        }

        #[test]
        fn should_reject_a_name_shorter_than_three_characters() {
            assert!(validate_bucket_name("ab").is_err());
        }

        #[test]
        fn should_reject_uppercase_letters() {
            assert!(validate_bucket_name("MyBucket").is_err());
        }

        #[test]
        fn should_reject_a_name_that_looks_like_an_ip_address() {
            assert!(validate_bucket_name("192.168.0.1").is_err());
        }

        #[test]
        fn should_reject_a_name_ending_in_a_hyphen() {
            assert!(validate_bucket_name("my-bucket-").is_err());
        }

        #[test]
        fn should_accept_a_name_with_dots_that_is_not_an_ip_address() {
            assert!(validate_bucket_name("my.bucket.name").is_ok());
        }
    }

    mod validate_key {
        use super::*;

        #[test]
        fn should_reject_an_empty_key() {
            assert!(validate_key("").is_err());
        }

        #[test]
        fn should_accept_a_key_with_slashes_and_unicode() {
            assert!(validate_key("photos/2024/caf\u{e9}.jpg").is_ok());
        }

        #[test]
        fn should_reject_a_key_over_the_length_limit() {
            let key = "a".repeat(MAX_KEY_LEN + 1);
            assert!(validate_key(&key).is_err());
        }
    }

    mod compute_etag {
        use super::*;

        #[test]
        fn should_return_the_known_md5_of_an_empty_payload() {
            assert_eq!(compute_etag(b""), "d41d8cd98f00b204e9800998ecf8427e");
        }

        #[test]
        fn should_return_the_known_md5_of_a_short_payload() {
            assert_eq!(compute_etag(b"hello"), "5d41402abc4b2a76b9719d911017c592");
        }
    }
}
