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
            Some((name.to_string(), value.to_str().ok()?.to_string()))
        })
        .collect()
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
