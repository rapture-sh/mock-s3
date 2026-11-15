use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use http::{HeaderMap, StatusCode, header};
use mock_s3_core::{Engine, ListRequest};

use crate::error::S3Error;
use crate::query::Query;
use crate::xml::ListingEcho;
use crate::{response, xml};

/// `PUT /{bucket}` — CreateBucket.
///
/// The body may carry a `CreateBucketConfiguration` naming a region. mock-s3 serves one
/// region, so it is accepted and ignored rather than rejected.
pub async fn create(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
) -> Result<Response, S3Error> {
    engine.create_bucket(&bucket).await?;

    let mut headers = HeaderMap::new();
    response::set(&mut headers, header::LOCATION, &format!("/{bucket}"));
    Ok(response::empty(StatusCode::OK, headers))
}

/// `DELETE /{bucket}` — DeleteBucket.
pub async fn delete(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
) -> Result<Response, S3Error> {
    engine.delete_bucket(&bucket).await?;

    Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()))
}

/// `HEAD /{bucket}` — HeadBucket.
pub async fn head(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
) -> Result<Response, S3Error> {
    engine.head_bucket(&bucket).await?;

    let mut headers = HeaderMap::new();
    response::set(
        &mut headers,
        http::HeaderName::from_static("x-amz-bucket-region"),
        engine.region(),
    );
    Ok(response::empty(StatusCode::OK, headers))
}

/// `GET /{bucket}` — ListObjectsV2, ListObjects, or a bucket subresource.
pub async fn get(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    if query.has("location") {
        engine.head_bucket(&bucket).await?;
        return Ok(response::xml(
            StatusCode::OK,
            xml::location_constraint(engine.region()),
        ));
    }
    if query.has("versioning") {
        engine.head_bucket(&bucket).await?;
        return Ok(response::xml(
            StatusCode::OK,
            xml::versioning_configuration(),
        ));
    }
    if query.has("uploads") {
        let uploads = engine.list_uploads(&bucket).await?;
        return Ok(response::xml(
            StatusCode::OK,
            xml::list_multipart_uploads_result(&bucket, &uploads),
        ));
    }

    let v2 = query.get("list-type") == Some("2");

    let max_keys = match query.non_empty("max-keys") {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| S3Error::invalid_argument(format!("max-keys is not a number: {raw}")))?,
        None => mock_s3_core::listing::MAX_KEYS_LIMIT,
    };

    // Echoed back as the client sent it, while the engine gets the cursor inside it.
    let sent_token = query.non_empty("continuation-token");
    let cursor = match (v2, sent_token) {
        (true, Some(raw)) => Some(crate::token::decode(raw).ok_or_else(|| {
            S3Error::invalid_argument(format!("continuation-token is not valid: {raw}"))
        })?),
        _ => None,
    };

    let request = ListRequest {
        prefix: query.get("prefix").unwrap_or_default().to_string(),
        delimiter: query.non_empty("delimiter").map(str::to_string),
        max_keys,
        start_after: if v2 {
            query.non_empty("start-after").map(str::to_string)
        } else {
            query.non_empty("marker").map(str::to_string)
        },
        continuation_token: cursor,
    };

    let result = engine.list_objects(&bucket, &request).await?;

    let echo = ListingEcho {
        bucket: &bucket,
        prefix: &request.prefix,
        delimiter: request.delimiter.as_deref(),
        max_keys: request.max_keys,
        continuation_token: sent_token,
        start_after: request.start_after.as_deref(),
        encoding_type: query.non_empty("encoding-type"),
        v2,
    };
    Ok(response::xml(
        StatusCode::OK,
        xml::list_bucket_result(&echo, &result),
    ))
}

/// `POST /{bucket}` — DeleteObjects and the other bucket-level posts.
pub async fn post(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
    RawQuery(raw): RawQuery,
    body: bytes::Bytes,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    if !query.has("delete") {
        return Err(S3Error::not_implemented("this bucket operation"));
    }

    // A missing bucket fails the whole request rather than every key in it.
    engine.head_bucket(&bucket).await?;

    let request = crate::parse::delete_objects(&body)?;

    // S3 reports per-key outcomes with a 200, so one bad key does not fail the batch.
    let mut deleted: Vec<String> = Vec::new();
    let mut errors: Vec<(String, &'static str, String)> = Vec::new();
    for key in request.keys {
        match engine.delete_object(&bucket, &key).await {
            Ok(()) => deleted.push(key),
            Err(error) => {
                let mapped = S3Error::from(error);
                errors.push((key, mapped.code, mapped.message));
            }
        }
    }

    Ok(response::xml(
        StatusCode::OK,
        xml::delete_result(&deleted, &errors, request.quiet),
    ))
}
