use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::{HeaderMap, HeaderName, StatusCode, header};
use mock_s3_core::{Engine, ObjectMeta};

use crate::error::S3Error;
use crate::query::Query;
use crate::{ops, response, xml};

const COPY_SOURCE: &str = "x-amz-copy-source";

/// `PUT /{bucket}/{key}` — PutObject.
pub async fn put(
    State(engine): State<Arc<Engine>>,
    Path((bucket, key)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    // Both of these are a PUT to the same path, so they have to be turned away here
    // rather than silently stored as an object.
    if query.has("uploadId") || query.has("partNumber") {
        return Err(S3Error::not_implemented("UploadPart"));
    }
    if headers.contains_key(COPY_SOURCE) {
        return Err(S3Error::not_implemented("CopyObject"));
    }

    // A streaming-signed upload arrives wrapped in chunk framing. Without this the frame
    // headers would be stored as part of the object.
    let body = crate::chunked::decode(body, &headers)?;

    let options = mock_s3_core::PutOptions {
        content_type: ops::content_type(&headers),
        user_metadata: ops::user_metadata(&headers),
        system_headers: ops::system_headers(&headers),
    };

    let meta = engine.put_object(&bucket, &key, body, options).await?;

    let mut out = HeaderMap::new();
    response::set(&mut out, header::ETAG, &quoted(&meta.etag));
    Ok(response::empty(StatusCode::OK, out))
}

/// `GET /{bucket}/{key}` — GetObject, with single-range support.
pub async fn get(
    State(engine): State<Arc<Engine>>,
    Path((bucket, key)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());
    if query.has("uploadId") {
        return Err(S3Error::not_implemented("ListParts"));
    }

    let object = engine.get_object(&bucket, &key).await?;
    let size = object.meta.size;

    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());

    match range {
        None => {
            let mut out = object_headers(&object.meta, Checksums::Include);
            response::set(&mut out, header::CONTENT_LENGTH, &size.to_string());
            Ok((StatusCode::OK, out, object.body).into_response())
        }
        Some(spec) => {
            let Some((start, end)) = parse_range(spec, size) else {
                return Err(S3Error::new(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "InvalidRange",
                    "The requested range is not satisfiable.",
                ));
            };

            // Inclusive on both ends, which is how HTTP ranges work.
            let slice = object.body.slice(start as usize..=end as usize);

            // A stored checksum covers the whole object. A client that validates it
            // against a partial body rejects the response, so S3 leaves it off a range
            // and so do we.
            let mut out = object_headers(&object.meta, Checksums::Omit);
            response::set(&mut out, header::CONTENT_LENGTH, &slice.len().to_string());
            response::set(
                &mut out,
                header::CONTENT_RANGE,
                &format!("bytes {start}-{end}/{size}"),
            );
            Ok((StatusCode::PARTIAL_CONTENT, out, slice).into_response())
        }
    }
}

/// `HEAD /{bucket}/{key}` — HeadObject.
pub async fn head(
    State(engine): State<Arc<Engine>>,
    Path((bucket, key)): Path<(String, String)>,
) -> Result<Response, S3Error> {
    let meta = engine.head_object(&bucket, &key).await?;

    let mut out = object_headers(&meta, Checksums::Include);
    response::set(&mut out, header::CONTENT_LENGTH, &meta.size.to_string());
    Ok(response::empty(StatusCode::OK, out))
}

/// `DELETE /{bucket}/{key}` — DeleteObject.
pub async fn delete(
    State(engine): State<Arc<Engine>>,
    Path((bucket, key)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());
    if query.has("uploadId") {
        return Err(S3Error::not_implemented("AbortMultipartUpload"));
    }

    engine.delete_object(&bucket, &key).await?;
    Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()))
}

/// `POST /{bucket}/{key}` — the multipart lifecycle.
pub async fn post(RawQuery(raw): RawQuery) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    if query.has("uploads") {
        return Err(S3Error::not_implemented("CreateMultipartUpload"));
    }
    if query.has("uploadId") {
        return Err(S3Error::not_implemented("CompleteMultipartUpload"));
    }
    Err(S3Error::not_implemented("this object operation"))
}

/// An ETag goes on the wire quoted, and SDKs compare it including the quotes.
fn quoted(etag: &str) -> String {
    format!("\"{etag}\"")
}

/// Whether a response should carry the object's stored checksums.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Checksums {
    Include,
    Omit,
}

fn object_headers(meta: &ObjectMeta, checksums: Checksums) -> HeaderMap {
    let mut headers = HeaderMap::new();

    response::set(&mut headers, header::CONTENT_TYPE, &meta.content_type);
    response::set(&mut headers, header::ETAG, &quoted(&meta.etag));
    response::set(
        &mut headers,
        header::LAST_MODIFIED,
        &xml::http_timestamp(&meta.last_modified),
    );
    response::set(
        &mut headers,
        HeaderName::from_static("accept-ranges"),
        "bytes",
    );
    // Kept verbatim on the way in, handed back verbatim on the way out.
    for (name, value) in &meta.system_headers {
        if checksums == Checksums::Omit && name.starts_with("x-amz-checksum-") {
            continue;
        }
        match HeaderName::try_from(name.as_str()) {
            Ok(name) => response::set(&mut headers, name, value),
            Err(_) => tracing::debug!(name, "dropping a stored header with an invalid name"),
        }
    }
    for (name, value) in &meta.user_metadata {
        response::set_user_metadata(&mut headers, name, value);
    }
    headers
}

/// Parse a single HTTP byte range against a known object size.
///
/// Returns inclusive bounds, or `None` when the range is unsatisfiable, which is a 416.
/// Multi-range requests return `None` too: S3 answers them with the whole object, but
/// answering with an error is the honest response until this supports them.
fn parse_range(spec: &str, size: u64) -> Option<(u64, u64)> {
    let spec = spec.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }

    let (start, end) = spec.split_once('-')?;

    match (start.trim(), end.trim()) {
        ("", "") => None,
        // `bytes=-N`: the last N bytes.
        ("", suffix) => {
            let length: u64 = suffix.parse().ok()?;
            if length == 0 {
                return None;
            }
            let length = length.min(size);
            Some((size - length, size - 1))
        }
        // `bytes=N-`: from N to the end.
        (start, "") => {
            let start: u64 = start.parse().ok()?;
            if start >= size {
                return None;
            }
            Some((start, size - 1))
        }
        (start, end) => {
            let start: u64 = start.parse().ok()?;
            let end: u64 = end.parse().ok()?;
            if start > end || start >= size {
                return None;
            }
            Some((start, end.min(size - 1)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_range {
        use super::*;

        #[test]
        fn should_read_a_closed_range() {
            assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        }

        #[test]
        fn should_clamp_a_range_that_runs_past_the_end() {
            assert_eq!(parse_range("bytes=990-2000", 1000), Some((990, 999)));
        }

        #[test]
        fn should_read_an_open_ended_range() {
            assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        }

        #[test]
        fn should_read_a_suffix_range_as_the_last_bytes() {
            assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        }

        #[test]
        fn should_clamp_a_suffix_range_longer_than_the_object() {
            assert_eq!(parse_range("bytes=-5000", 1000), Some((0, 999)));
        }

        #[test]
        fn should_reject_a_start_at_or_past_the_end() {
            assert!(parse_range("bytes=1000-", 1000).is_none());
            assert!(parse_range("bytes=1000-1001", 1000).is_none());
        }

        #[test]
        fn should_reject_an_inverted_range() {
            assert!(parse_range("bytes=500-100", 1000).is_none());
        }

        #[test]
        fn should_reject_any_range_against_an_empty_object() {
            assert!(parse_range("bytes=0-0", 0).is_none());
        }

        #[test]
        fn should_reject_a_multi_range_request() {
            assert!(parse_range("bytes=0-10,20-30", 1000).is_none());
        }

        #[test]
        fn should_reject_a_unit_other_than_bytes() {
            assert!(parse_range("items=0-10", 1000).is_none());
        }
    }

    mod quoted {
        use super::*;

        #[test]
        fn should_wrap_the_etag_in_quotes() {
            assert_eq!(quoted("abc"), "\"abc\"");
        }
    }
}
