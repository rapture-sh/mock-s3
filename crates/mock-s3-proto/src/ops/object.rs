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

/// `PUT /{bucket}/{key}`: PutObject.
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
    // UploadPart: same method and path as PutObject, told apart by the query.
    if let (Some(upload_id), Some(raw_number)) =
        (query.non_empty("uploadId"), query.non_empty("partNumber"))
    {
        let number: u32 = raw_number.parse().map_err(|_| {
            S3Error::invalid_argument(format!("partNumber is not a number: {raw_number}"))
        })?;
        let body = crate::chunked::decode(body, &headers)?;

        let part = engine.upload_part(&bucket, upload_id, number, body).await?;

        let mut out = HeaderMap::new();
        response::set(&mut out, header::ETAG, &quoted(&part.etag));
        return Ok(response::empty(StatusCode::OK, out));
    }
    // Accepted and ignored: mock-s3 has no ACL or tag model, and a 501 here stops tools
    // that set them as a matter of course.
    if query.has("acl") || query.has("tagging") {
        engine.head_object(&bucket, &key).await?;
        return Ok(response::empty(StatusCode::OK, HeaderMap::new()));
    }

    // CopyObject: a PUT that names a source instead of carrying a body.
    if let Some(source) = headers
        .get(COPY_SOURCE)
        .and_then(|value| value.to_str().ok())
    {
        return copy_object(&engine, &bucket, &key, source, &headers).await;
    }

    // Anything else that names a subresource is refused here. Falling through would make
    // `PUT /{bucket}/{key}?retention` store the retention document *as the object*.
    ops::refuse_unhandled(
        &query,
        ops::OBJECT_SUBRESOURCES,
        &["acl", "tagging", "uploadId"],
    )?;

    // A streaming-signed upload arrives wrapped in chunk framing. Without this the frame
    // headers would be stored as part of the object.
    let body = crate::chunked::decode(body, &headers)?;

    let options = mock_s3_core::PutOptions {
        content_type: ops::content_type(&headers),
        user_metadata: ops::user_metadata(&headers),
        system_headers: ops::system_headers(&headers),
        etag: None,
    };

    let meta = engine.put_object(&bucket, &key, body, options).await?;

    let mut out = HeaderMap::new();
    response::set(&mut out, header::ETAG, &quoted(&meta.etag));
    set_version_id(&mut out, meta.version_id.as_deref());
    Ok(response::empty(StatusCode::OK, out))
}

/// Report which version a response is about.
///
/// Left off entirely in a bucket that was never versioned, because S3 sends it only when
/// there is a version to name, a client that sees the header at all concludes the bucket
/// is versioned.
fn set_version_id(headers: &mut HeaderMap, version_id: Option<&str>) {
    if let Some(version_id) = version_id {
        response::set(
            headers,
            HeaderName::from_static(crate::error::VERSION_ID),
            version_id,
        );
    }
}

/// `GET /{bucket}/{key}`: GetObject, with single-range support.
pub async fn get(
    State(engine): State<Arc<Engine>>,
    Path((bucket, key)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    if let Some(upload_id) = query.non_empty("uploadId") {
        let upload_key = engine.upload_key(&bucket, upload_id).await?;
        let parts = engine.list_parts(&bucket, upload_id).await?;

        return Ok(response::xml(
            StatusCode::OK,
            xml::list_parts_result(&bucket, &upload_key, upload_id, &parts),
        ));
    }

    if query.has("acl") {
        engine.head_object(&bucket, &key).await?;
        return Ok(response::xml(StatusCode::OK, xml::access_control_policy()));
    }
    if query.has("tagging") {
        engine.head_object(&bucket, &key).await?;
        return Ok(response::xml(StatusCode::OK, xml::object_tagging()));
    }

    // Refused rather than answered with the object's bytes, which is what a caller of
    // `GetObjectAttributes` or `SelectObjectContent` would otherwise get.
    ops::refuse_unhandled(
        &query,
        ops::OBJECT_SUBRESOURCES,
        &["acl", "tagging", "uploadId"],
    )?;

    let object = engine
        .get_object_version(&bucket, &key, query.non_empty("versionId"))
        .await?;

    match preconditions(&headers, &object.meta) {
        Precondition::Failed => return Err(precondition_failed()),
        Precondition::NotModified => {
            let mut out = object_headers(&object.meta, Checksums::Omit);
            apply_response_overrides(&mut out, &query);
            out.remove(header::CONTENT_TYPE);
            return Ok(response::empty(StatusCode::NOT_MODIFIED, out));
        }
        Precondition::Proceed => {}
    }

    let size = object.meta.size;

    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());

    match range {
        None => {
            let mut out = object_headers(&object.meta, Checksums::Include);
            apply_response_overrides(&mut out, &query);
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
            apply_response_overrides(&mut out, &query);
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

/// `HEAD /{bucket}/{key}`: HeadObject.
pub async fn head(
    State(engine): State<Arc<Engine>>,
    Path((bucket, key)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    ops::refuse_unhandled(&query, ops::OBJECT_SUBRESOURCES, &[])?;

    let meta = engine
        .head_object_version(&bucket, &key, query.non_empty("versionId"))
        .await?;

    match preconditions(&headers, &meta) {
        Precondition::Failed => return Err(precondition_failed()),
        Precondition::NotModified => {
            return Ok(response::empty(
                StatusCode::NOT_MODIFIED,
                object_headers(&meta, Checksums::Omit),
            ));
        }
        Precondition::Proceed => {}
    }

    let mut out = object_headers(&meta, Checksums::Include);
    apply_response_overrides(&mut out, &query);
    response::set(&mut out, header::CONTENT_LENGTH, &meta.size.to_string());
    Ok(response::empty(StatusCode::OK, out))
}

/// `DELETE /{bucket}/{key}`: DeleteObject.
pub async fn delete(
    State(engine): State<Arc<Engine>>,
    Path((bucket, key)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());
    if let Some(upload_id) = query.non_empty("uploadId") {
        engine.abort_upload(&bucket, upload_id).await?;
        return Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()));
    }
    if query.has("tagging") {
        engine.head_object(&bucket, &key).await?;
        return Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()));
    }

    ops::refuse_unhandled(&query, ops::OBJECT_SUBRESOURCES, &["tagging", "uploadId"])?;

    let outcome = engine
        .delete_object_version(&bucket, &key, query.non_empty("versionId"))
        .await?;

    let mut out = HeaderMap::new();
    set_version_id(&mut out, outcome.version_id.as_deref());
    if outcome.delete_marker {
        response::set(
            &mut out,
            HeaderName::from_static(crate::error::DELETE_MARKER),
            "true",
        );
    }
    Ok(response::empty(StatusCode::NO_CONTENT, out))
}

/// `POST /{bucket}/{key}`: the multipart lifecycle.
pub async fn post(
    State(engine): State<Arc<Engine>>,
    Path((bucket, key)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    if query.has("uploads") {
        let options = mock_s3_core::PutOptions {
            content_type: ops::content_type(&headers),
            user_metadata: ops::user_metadata(&headers),
            system_headers: ops::system_headers(&headers),
            etag: None,
        };
        let upload_id = engine.create_upload(&bucket, &key, options).await?;

        return Ok(response::xml(
            StatusCode::OK,
            xml::initiate_multipart_upload(&bucket, &key, &upload_id),
        ));
    }

    if let Some(upload_id) = query.non_empty("uploadId") {
        let listed: Vec<(u32, String)> = crate::parse::complete_multipart_upload(&body)?
            .into_iter()
            .map(|part| (part.number, part.etag))
            .collect();

        let meta = engine.complete_upload(&bucket, upload_id, &listed).await?;

        let mut out = HeaderMap::new();
        response::set(&mut out, header::CONTENT_TYPE, "application/xml");
        set_version_id(&mut out, meta.version_id.as_deref());

        return Ok((
            StatusCode::OK,
            out,
            xml::complete_multipart_upload(&bucket, &meta.key, &meta.etag),
        )
            .into_response());
    }

    Err(S3Error::not_implemented("this object operation"))
}

/// Copy one object onto another key.
///
/// `x-amz-copy-source` is `/bucket/key` or `bucket/key`, percent-encoded. By default the
/// copy keeps the source's content type and metadata; `x-amz-metadata-directive:
/// REPLACE` takes them from this request instead.
async fn copy_object(
    engine: &Engine,
    bucket: &str,
    key: &str,
    source: &str,
    headers: &HeaderMap,
) -> Result<Response, S3Error> {
    // A source may name a version: `/bucket/key?versionId=...`. Without this, copying
    // from a versioned bucket would silently take the current version instead of the one
    // asked for, which is the kind of divergence nothing visibly fails on.
    //
    // Split before decoding, never after. `?` is a legal character in an object key and
    // arrives percent-encoded; decoding first would turn the key `weird?versionId=x` into
    // a copy of `weird` at version `x`.
    let (raw_path, raw_version) = match source.split_once("?versionId=") {
        Some((path, version)) => (path, Some(version)),
        None => (source, None),
    };

    let decode = |value: &str| {
        percent_encoding::percent_decode_str(value)
            .decode_utf8_lossy()
            .into_owned()
    };
    let path = decode(raw_path);
    let source_version = raw_version.map(decode);

    let (source_bucket, source_key) =
        path.trim_start_matches('/')
            .split_once('/')
            .ok_or_else(|| {
                S3Error::invalid_argument(format!(
                    "x-amz-copy-source must be /bucket/key, got {source}"
                ))
            })?;

    let object = engine
        .get_object_version(source_bucket, source_key, source_version.as_deref())
        .await?;

    let replacing = headers
        .get("x-amz-metadata-directive")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("REPLACE"));

    // Copying a key onto itself only means something if the metadata is being replaced,
    // otherwise it is a request to change nothing, and S3 refuses it rather than
    // rewriting the object. Allowing it hides a client bug, usually a sync that has
    // computed the wrong destination.
    if source_bucket == bucket && source_key == key && source_version.is_none() && !replacing {
        return Err(S3Error::new(
            StatusCode::BAD_REQUEST,
            "InvalidRequest",
            "This copy request is illegal because it is trying to copy an object to \
             itself without changing the object's metadata, storage class, website \
             redirect location or encryption attributes.",
        ));
    }

    let options = if replacing {
        mock_s3_core::PutOptions {
            content_type: ops::content_type(headers),
            user_metadata: ops::user_metadata(headers),
            system_headers: ops::system_headers(headers),
            etag: None,
        }
    } else {
        mock_s3_core::PutOptions {
            content_type: object.meta.content_type.clone(),
            user_metadata: object.meta.user_metadata.clone(),
            system_headers: object.meta.system_headers.clone(),
            etag: None,
        }
    };

    let source_version_id = object.meta.version_id.clone();
    let meta = engine.put_object(bucket, key, object.body, options).await?;

    let mut out = HeaderMap::new();
    response::set(&mut out, header::CONTENT_TYPE, "application/xml");
    set_version_id(&mut out, meta.version_id.as_deref());
    if let Some(version_id) = &source_version_id {
        response::set(
            &mut out,
            HeaderName::from_static("x-amz-copy-source-version-id"),
            version_id,
        );
    }

    Ok((
        StatusCode::OK,
        out,
        xml::copy_object_result(&meta.etag, &meta.last_modified),
    )
        .into_response())
}

/// `OPTIONS /{bucket}/{key}`: a CORS preflight. The key plays no part: CORS is
/// configured per bucket.
pub async fn options(
    State(engine): State<Arc<Engine>>,
    Path((bucket, _key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    crate::ops::preflight(&engine, &bucket, &headers).await
}

/// What the conditional headers on a read say to do.
#[derive(Debug, PartialEq, Eq)]
enum Precondition {
    Proceed,
    /// The client already has this version: 304, no body.
    NotModified,
    /// The client's assumption is wrong: 412.
    Failed,
}

fn precondition_failed() -> S3Error {
    S3Error::new(
        StatusCode::PRECONDITION_FAILED,
        "PreconditionFailed",
        "At least one of the preconditions you specified did not hold.",
    )
}

/// Evaluate `If-Match`, `If-None-Match`, `If-Modified-Since` and
/// `If-Unmodified-Since`.
///
/// Precedence follows HTTP: an entity-tag condition wins over the matching date
/// condition, so a client sending both gets the answer the stronger validator implies.
fn preconditions(headers: &HeaderMap, meta: &ObjectMeta) -> Precondition {
    let header = |name: http::HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
    };

    let matches_etag = |candidates: &str| {
        candidates == "*"
            || candidates
                .split(',')
                .map(|tag| tag.trim().trim_start_matches("W/").trim_matches('"'))
                .any(|tag| tag == meta.etag)
    };

    if let Some(candidates) = header(header::IF_MATCH) {
        return if matches_etag(candidates) {
            Precondition::Proceed
        } else {
            Precondition::Failed
        };
    }
    if let Some(candidates) = header(header::IF_NONE_MATCH) {
        return if matches_etag(candidates) {
            Precondition::NotModified
        } else {
            Precondition::Proceed
        };
    }

    // Last-Modified goes on the wire with second precision, so compare at that
    // granularity or a sub-second difference looks like a change.
    let modified_at = meta.last_modified.timestamp();

    if let Some(since) = header(header::IF_UNMODIFIED_SINCE).and_then(parse_http_date) {
        return if modified_at > since {
            Precondition::Failed
        } else {
            Precondition::Proceed
        };
    }
    if let Some(since) = header(header::IF_MODIFIED_SINCE).and_then(parse_http_date) {
        return if modified_at <= since {
            Precondition::NotModified
        } else {
            Precondition::Proceed
        };
    }

    Precondition::Proceed
}

/// An HTTP date, as a Unix timestamp.
fn parse_http_date(value: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(value, "%a, %d %b %Y %H:%M:%S GMT")
        .ok()
        .map(|naive| naive.and_utc().timestamp())
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
    set_version_id(&mut headers, meta.version_id.as_deref());
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

/// Apply the `response-*` query parameters, which override the headers the object was
/// stored with for this one response.
///
/// This is how a presigned URL serves an object under a different name or content type,
/// the stored object is untouched. Ignoring them answers with the stored values, which
/// looks right and silently defeats a download link's filename.
fn apply_response_overrides(headers: &mut HeaderMap, query: &Query) {
    const OVERRIDES: &[(&str, http::HeaderName)] = &[
        ("response-content-type", header::CONTENT_TYPE),
        ("response-content-language", header::CONTENT_LANGUAGE),
        ("response-content-encoding", header::CONTENT_ENCODING),
        ("response-cache-control", header::CACHE_CONTROL),
        ("response-expires", header::EXPIRES),
        ("response-content-disposition", header::CONTENT_DISPOSITION),
    ];

    for (parameter, name) in OVERRIDES {
        if let Some(value) = query.non_empty(parameter) {
            response::set(headers, name.clone(), value);
        }
    }
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

    mod parse_http_date {
        use super::*;

        #[test]
        fn should_read_an_http_date() {
            assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        }

        #[test]
        fn should_reject_anything_else() {
            assert!(parse_http_date("1970-01-01").is_none());
            assert!(parse_http_date("not a date").is_none());
        }
    }

    mod preconditions {
        use super::*;

        fn meta_at(seconds: i64) -> ObjectMeta {
            ObjectMeta {
                key: "k".into(),
                size: 1,
                etag: "abc".into(),
                last_modified: chrono::DateTime::from_timestamp(seconds, 0)
                    .expect("a valid test timestamp"),
                content_type: "text/plain".into(),
                user_metadata: Default::default(),
                version_id: None,
                system_headers: Default::default(),
            }
        }

        fn with(name: &'static str, value: &str) -> HeaderMap {
            let mut headers = HeaderMap::new();
            headers.insert(name, value.parse().expect("a valid test header"));
            headers
        }

        #[test]
        fn should_proceed_when_no_condition_is_given() {
            assert_eq!(
                preconditions(&HeaderMap::new(), &meta_at(0)),
                Precondition::Proceed
            );
        }

        #[test]
        fn should_proceed_when_if_match_matches() {
            let headers = with("if-match", "\"abc\"");

            assert_eq!(preconditions(&headers, &meta_at(0)), Precondition::Proceed);
        }

        #[test]
        fn should_fail_when_if_match_does_not_match() {
            let headers = with("if-match", "\"other\"");

            assert_eq!(preconditions(&headers, &meta_at(0)), Precondition::Failed);
        }

        #[test]
        fn should_accept_a_star_for_if_match() {
            let headers = with("if-match", "*");

            assert_eq!(preconditions(&headers, &meta_at(0)), Precondition::Proceed);
        }

        #[test]
        fn should_report_not_modified_when_if_none_match_matches() {
            let headers = with("if-none-match", "\"abc\"");

            assert_eq!(
                preconditions(&headers, &meta_at(0)),
                Precondition::NotModified
            );
        }

        #[test]
        fn should_match_one_etag_out_of_a_list() {
            let headers = with("if-match", "\"nope\", \"abc\"");

            assert_eq!(preconditions(&headers, &meta_at(0)), Precondition::Proceed);
        }

        #[test]
        fn should_ignore_a_weak_validator_prefix() {
            let headers = with("if-match", "W/\"abc\"");

            assert_eq!(preconditions(&headers, &meta_at(0)), Precondition::Proceed);
        }

        #[test]
        fn should_report_not_modified_when_the_object_is_older_than_if_modified_since() {
            let headers = with("if-modified-since", "Thu, 01 Jan 1970 00:00:10 GMT");

            assert_eq!(
                preconditions(&headers, &meta_at(5)),
                Precondition::NotModified
            );
        }

        #[test]
        fn should_proceed_when_the_object_is_newer_than_if_modified_since() {
            let headers = with("if-modified-since", "Thu, 01 Jan 1970 00:00:10 GMT");

            assert_eq!(preconditions(&headers, &meta_at(20)), Precondition::Proceed);
        }

        #[test]
        fn should_fail_when_the_object_changed_after_if_unmodified_since() {
            let headers = with("if-unmodified-since", "Thu, 01 Jan 1970 00:00:10 GMT");

            assert_eq!(preconditions(&headers, &meta_at(20)), Precondition::Failed);
        }

        #[test]
        fn should_let_an_entity_tag_condition_win_over_a_date_one() {
            let mut headers = with("if-match", "\"abc\"");
            headers.insert(
                "if-unmodified-since",
                "Thu, 01 Jan 1970 00:00:00 GMT".parse().unwrap(),
            );

            assert_eq!(
                preconditions(&headers, &meta_at(500)),
                Precondition::Proceed
            );
        }
    }
}
