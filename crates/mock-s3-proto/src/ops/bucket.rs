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
    RawQuery(raw): RawQuery,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    // `PUT /{bucket}?versioning` configures a bucket; it must not create one.
    if let Some(name) = configurable(&query) {
        engine.head_bucket(&bucket).await?;
        tracing::debug!(bucket, subresource = name, "bucket configuration accepted");
        return Ok(response::empty(StatusCode::OK, HeaderMap::new()));
    }

    engine.create_bucket(&bucket).await?;

    let mut headers = HeaderMap::new();
    response::set(&mut headers, header::LOCATION, &format!("/{bucket}"));
    Ok(response::empty(StatusCode::OK, headers))
}

/// `DELETE /{bucket}` — DeleteBucket, or clearing a configuration.
pub async fn delete(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    // As with PUT: `DELETE /{bucket}?cors` clears a setting, it does not delete the
    // bucket. Getting this wrong would destroy data on a request meant to be harmless.
    if let Some(name) = configurable(&query) {
        engine.head_bucket(&bucket).await?;
        tracing::debug!(bucket, subresource = name, "bucket configuration cleared");
        return Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()));
    }

    engine.delete_bucket(&bucket).await?;

    Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()))
}

/// Bucket settings mock-s3 accepts and ignores.
///
/// It stores none of them, because nothing here acts on a lifecycle rule or a bucket
/// policy. Accepting them keeps tools working; pretending to enforce them would be
/// worse than refusing.
const CONFIGURABLE: &[&str] = &[
    "accelerate",
    "acl",
    "cors",
    "encryption",
    "lifecycle",
    "logging",
    "notification",
    "object-lock",
    "ownershipControls",
    "policy",
    "publicAccessBlock",
    "replication",
    "requestPayment",
    "tagging",
    "versioning",
    "website",
];

fn configurable(query: &Query) -> Option<&'static str> {
    CONFIGURABLE.iter().copied().find(|name| query.has(name))
}

/// What a `GET /{bucket}?<something>` asks for, other than a listing.
///
/// Clients and tools probe several of these the moment they connect, and treat an
/// unexpected status as fatal. Real S3 answers each with either a specific "not
/// configured" error or an empty document, so that is what this does. Returning
/// `501 NotImplemented` instead is what stops a tool from starting at all.
async fn subresource(
    engine: &Engine,
    bucket: &str,
    query: &Query,
) -> Result<Option<Response>, S3Error> {
    /// Codes S3 uses for a setting that was never configured. Each pairs with 404.
    const NOT_CONFIGURED: &[(&str, &str, &str)] = &[
        (
            "cors",
            "NoSuchCORSConfiguration",
            "The CORS configuration does not exist.",
        ),
        (
            "lifecycle",
            "NoSuchLifecycleConfiguration",
            "The lifecycle configuration does not exist.",
        ),
        (
            "policy",
            "NoSuchBucketPolicy",
            "The bucket policy does not exist.",
        ),
        (
            "tagging",
            "NoSuchTagSet",
            "There is no tag set associated with this bucket.",
        ),
        (
            "encryption",
            "ServerSideEncryptionConfigurationNotFoundError",
            "The server side encryption configuration was not found.",
        ),
        (
            "website",
            "NoSuchWebsiteConfiguration",
            "The specified bucket does not have a website configuration.",
        ),
        (
            "replication",
            "ReplicationConfigurationNotFoundError",
            "The replication configuration was not found.",
        ),
        (
            "object-lock",
            "ObjectLockConfigurationNotFoundError",
            "Object Lock configuration does not exist for this bucket.",
        ),
    ];

    /// Settings S3 answers with an empty document rather than an error.
    const EMPTY: &[(&str, &str)] = &[
        ("notification", "NotificationConfiguration"),
        ("logging", "BucketLoggingStatus"),
        ("accelerate", "AccelerateConfiguration"),
        ("ownershipControls", "OwnershipControls"),
        ("publicAccessBlock", "PublicAccessBlockConfiguration"),
    ];

    let known = query.has("location")
        || query.has("versioning")
        || query.has("uploads")
        || query.has("acl")
        || query.has("requestPayment")
        || NOT_CONFIGURED.iter().any(|(name, _, _)| query.has(name))
        || EMPTY.iter().any(|(name, _)| query.has(name));

    if !known {
        return Ok(None);
    }

    // All of them need the bucket to exist, and reporting that first matches S3.
    engine.head_bucket(bucket).await?;

    if let Some((_, code, message)) = NOT_CONFIGURED.iter().find(|(name, _, _)| query.has(name)) {
        return Err(S3Error::new(StatusCode::NOT_FOUND, code, *message));
    }
    if let Some((_, root)) = EMPTY.iter().find(|(name, _)| query.has(name)) {
        return Ok(Some(response::xml(
            StatusCode::OK,
            xml::empty_configuration(root),
        )));
    }

    let body = if query.has("location") {
        xml::location_constraint(engine.region())
    } else if query.has("versioning") {
        xml::versioning_configuration()
    } else if query.has("acl") {
        xml::access_control_policy()
    } else if query.has("requestPayment") {
        xml::request_payment_configuration()
    } else {
        let uploads = engine.list_uploads(bucket).await?;
        xml::list_multipart_uploads_result(bucket, &uploads)
    };

    Ok(Some(response::xml(StatusCode::OK, body)))
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

    if let Some(answer) = subresource(&engine, &bucket, &query).await? {
        return Ok(answer);
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
