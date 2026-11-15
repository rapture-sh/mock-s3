use std::collections::BTreeMap;

use http::HeaderMap;
use mock_s3_core::model::DEFAULT_CONTENT_TYPE;

pub mod bucket;
pub mod object;
pub mod service;

/// Answer a CORS preflight for a bucket.
///
/// A browser sends this before the real request and refuses to continue unless the
/// response allows the origin, method and headers it names. With no CORS configuration
/// stored, S3 refuses — and so does this, because a mock that always allows would let a
/// web app pass here and fail against AWS.
pub(crate) async fn preflight(
    engine: &mock_s3_core::Engine,
    bucket: &str,
    headers: &HeaderMap,
) -> Result<axum::response::Response, crate::S3Error> {
    use crate::{cors, response};

    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
    };

    let Some(origin) = text("origin") else {
        return Err(crate::S3Error::invalid_argument(
            "a CORS preflight must carry an Origin header",
        ));
    };
    let method = text("access-control-request-method").unwrap_or("GET");
    let requested = text("access-control-request-headers").unwrap_or_default();

    engine.head_bucket(bucket).await?;

    let Some(stored) = engine
        .get_bucket_config(bucket, mock_s3_core::bucket_config::CORS)
        .await?
    else {
        return Err(cors::forbidden());
    };

    let rules = cors::parse(&stored)?;
    let Some(allowed) = cors::evaluate(&rules, origin, method, requested) else {
        return Err(cors::forbidden());
    };

    let mut out = HeaderMap::new();
    for (name, value) in cors_response_headers(&allowed) {
        if let Ok(name) = http::HeaderName::try_from(name) {
            crate::response::set(&mut out, name, &value);
        }
    }
    Ok(response::empty(http::StatusCode::OK, out))
}

/// The `Access-Control-*` headers an allowance turns into.
pub(crate) fn cors_response_headers(
    allowed: &crate::cors::Allowance,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("access-control-allow-origin", allowed.origin.clone()),
        ("access-control-allow-methods", allowed.methods.clone()),
        // Caches must not serve one origin's response to another.
        ("vary", "Origin".to_string()),
    ];
    if let Some(value) = &allowed.headers {
        headers.push(("access-control-allow-headers", value.clone()));
    }
    if let Some(value) = &allowed.expose_headers {
        headers.push(("access-control-expose-headers", value.clone()));
    }
    if let Some(seconds) = allowed.max_age_seconds {
        headers.push(("access-control-max-age", seconds.to_string()));
    }
    headers
}

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
            Some((name.to_string(), metadata_text(value.as_bytes())))
        })
        .collect()
}

/// Read a user metadata value as text.
///
/// A value may legally hold a byte above `0x7F` — `x-amz-meta-x: café` is a real thing
/// clients send — and the old code ran it through `to_str`, which rejects those bytes, so
/// the metadata was dropped without a word.
///
/// Clients send such a value as UTF-8, and S3 answers with it as Latin-1, so metadata is
/// text rather than bytes: `é` arrives as `C3 A9` and comes back as `E9`. ceph/s3-tests
/// pins that in `test_object_set_get_unicode_metadata`, which fails against a server that
/// echoes the bytes it was given. A value that is *not* valid UTF-8 is kept byte for byte
/// instead, since there is nothing better to do with it than hand it back unchanged.
fn metadata_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|byte| *byte as char).collect(),
    }
}

/// Headers S3 stores with an object and returns on the way out.
///
/// Anything here is kept verbatim and echoed back; mock-s3 does not act on any of them.
/// That is enough for the clients that set them, and it keeps the engine out of the
/// business of knowing what a storage class is.
const PRESERVED_HEADERS: &[&str] = &[
    "cache-control",
    "content-disposition",
    "content-language",
    "expires",
    "x-amz-server-side-encryption",
    "x-amz-server-side-encryption-aws-kms-key-id",
    "x-amz-storage-class",
    "x-amz-website-redirect-location",
];

/// Checksum headers are a family rather than a fixed list, so they match by prefix.
const CHECKSUM_PREFIX: &str = "x-amz-checksum-";

/// Collect the headers worth keeping with an object.
///
/// `Content-Encoding` needs care: a streaming-signed upload arrives with `aws-chunked`
/// in it, describing the transfer rather than the object. Real S3 strips that, and so
/// must this — otherwise every client that downloads the object is told to un-chunk
/// bytes that were already un-chunked on the way in.
pub(crate) fn system_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut kept = BTreeMap::new();

    for (name, value) in headers {
        let name = name.as_str();
        let Ok(value) = value.to_str() else { continue };

        let worth_keeping = PRESERVED_HEADERS.contains(&name) || name.starts_with(CHECKSUM_PREFIX);
        if worth_keeping {
            kept.insert(name.to_string(), value.to_string());
        }
    }

    if let Some(encoding) = headers
        .get(http::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
    {
        let remaining: Vec<&str> = encoding
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty() && !part.eq_ignore_ascii_case("aws-chunked"))
            .collect();

        // Removed first, then put back only if something survived. `aws-chunked` is
        // framing this server has already undone, so it must not be reported as part of
        // the object's encoding — and when it was the *only* encoding, the loop above
        // has already stored the raw value, so dropping the key is the whole fix.
        kept.remove("content-encoding");
        if !remaining.is_empty() {
            kept.insert("content-encoding".to_string(), remaining.join(", "));
        }
    }

    kept
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

/// Every query parameter S3 treats as naming an operation on a bucket, rather than as
/// modifying one.
///
/// The list is the whole set S3 defines, implemented or not, which is what makes
/// [`refuse_unhandled`] safe: adding a name here makes every handler refuse it until one
/// of them declares it handled.
pub(crate) const BUCKET_SUBRESOURCES: &[&str] = &[
    "accelerate",
    "acl",
    "analytics",
    "cors",
    "delete",
    "encryption",
    "intelligent-tiering",
    "inventory",
    "lifecycle",
    "location",
    "logging",
    "metadataTable",
    "metrics",
    "notification",
    "object-lock",
    "ownershipControls",
    "policy",
    "policyStatus",
    "publicAccessBlock",
    "replication",
    "requestPayment",
    "tagging",
    "uploads",
    "versioning",
    "versions",
    "website",
];

/// The same, for an operation on an object.
///
/// `versionId` and `partNumber` are deliberately absent: they modify an operation rather
/// than naming one, so a request carrying either still means the operation its method and
/// path say.
pub(crate) const OBJECT_SUBRESOURCES: &[&str] = &[
    "acl",
    "attributes",
    "legal-hold",
    "restore",
    "retention",
    "select",
    "tagging",
    "torrent",
    "uploadId",
    "uploads",
];

/// Refuse a request that names a subresource this handler does not implement.
///
/// Without this a handler falls through to whatever its method and path mean on their
/// own, which is a *different operation* than the caller asked for, and it looks like
/// success:
///
/// - `PUT /{bucket}/{key}?retention` became `PutObject` and overwrote the object with the
///   retention document.
/// - `GET /{bucket}/{key}?attributes` returned the object's bytes where the caller
///   expected a `GetObjectAttributes` document.
/// - `GET /{bucket}?analytics` answered with a bucket listing.
///
/// `handled` is what the calling handler deals with itself, so this is called last, once
/// those branches have been taken.
pub(crate) fn refuse_unhandled(
    query: &crate::query::Query,
    defined: &[&str],
    handled: &[&str],
) -> Result<(), crate::S3Error> {
    let unhandled = defined
        .iter()
        .find(|name| query.has(name) && !handled.contains(*name));

    match unhandled {
        Some(name) => Err(crate::S3Error::not_implemented(&format!("?{name}"))),
        None => Ok(()),
    }
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

    mod system_headers {
        use super::*;

        fn encoding(value: &str) -> Option<String> {
            let mut headers = HeaderMap::new();
            headers.insert("content-encoding", value.parse().unwrap());

            super::super::system_headers(&headers)
                .get("content-encoding")
                .cloned()
        }

        #[test]
        fn should_keep_a_content_encoding_that_is_not_framing() {
            assert_eq!(encoding("gzip").as_deref(), Some("gzip"));
            assert_eq!(encoding("deflate, gzip").as_deref(), Some("deflate, gzip"));
        }

        /// `aws-chunked` is framing this server has already undone, so it is not part of
        /// the object's encoding and must not be reported as if it were.
        #[test]
        fn should_strip_aws_chunked_from_a_content_encoding() {
            assert_eq!(encoding("gzip, aws-chunked").as_deref(), Some("gzip"));
            assert_eq!(encoding("aws-chunked, gzip").as_deref(), Some("gzip"));
        }

        /// The case that was wrong: with nothing left after stripping, the header has to
        /// go entirely rather than keep the raw value.
        #[test]
        fn should_drop_the_header_when_only_framing_was_sent() {
            assert_eq!(encoding("aws-chunked"), None);
            assert_eq!(encoding("aws-chunked, aws-chunked"), None);
        }
    }

    mod user_metadata_bytes {
        use super::*;

        /// A metadata value may hold a byte above 0x7F, and S3 hands back what it was
        /// given. Treating the value as UTF-8 dropped it silently.
        #[test]
        fn should_round_trip_a_value_that_is_not_ascii() {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-amz-meta-meta1",
                http::HeaderValue::from_bytes(b"Hello World\xe9").unwrap(),
            );

            let stored = user_metadata(&headers);
            assert_eq!(
                stored.get("meta1").map(String::as_str),
                Some("Hello Worldé")
            );

            let mut out = HeaderMap::new();
            crate::response::set_user_metadata(&mut out, "meta1", &stored["meta1"]);

            assert_eq!(
                out.get("x-amz-meta-meta1").map(|value| value.as_bytes()),
                Some(&b"Hello World\xe9"[..]),
                "the bytes that went in have to be the bytes that come out"
            );
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
