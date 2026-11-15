use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use http::{HeaderMap, StatusCode, header};
use mock_s3_core::{Engine, ListRequest, VersionListRequest};

use crate::error::S3Error;
use crate::query::Query;
use crate::xml::{ListingEcho, VersionListingEcho};
use crate::{response, xml};

/// `PUT /{bucket}` — CreateBucket.
///
/// The body may carry a `CreateBucketConfiguration` naming a region. mock-s3 serves one
/// region, so it is accepted and ignored rather than rejected.
pub async fn create(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
    RawQuery(raw): RawQuery,
    body: bytes::Bytes,
) -> Result<Response, S3Error> {
    let query = Query::parse(raw.as_deref());

    // Stored rather than ignored: preflight has to evaluate these.
    if query.has("cors") {
        // Parsed before storing, so a malformed document fails here rather than on the
        // first browser request.
        crate::cors::parse(&body)?;
        engine
            .put_bucket_config(&bucket, mock_s3_core::bucket_config::CORS, body)
            .await?;
        return Ok(response::empty(StatusCode::OK, HeaderMap::new()));
    }

    if query.has("versioning") {
        let status = crate::parse::versioning_configuration(&body)?;
        engine.put_bucket_versioning(&bucket, status).await?;
        return Ok(response::empty(StatusCode::OK, HeaderMap::new()));
    }

    // `PUT /{bucket}?acl` configures a bucket; it must not create one.
    if let Some(name) = configurable(&query) {
        engine.head_bucket(&bucket).await?;
        tracing::debug!(bucket, subresource = name, "bucket configuration accepted");
        return Ok(response::empty(StatusCode::OK, HeaderMap::new()));
    }

    // Every subresource this handler covers returned early above, so reaching here with
    // one named means it is not implemented — and treating it as CreateBucket instead
    // would answer a request for something else with a success.
    crate::ops::refuse_unhandled(&query, crate::ops::BUCKET_SUBRESOURCES, &[])?;

    match engine.create_bucket(&bucket).await {
        Ok(()) => {}
        // us-east-1 is the exception, and it is the region mock-s3 reports by default:
        // re-creating a bucket you already own succeeds there, while every other region
        // answers `BucketAlreadyOwnedByYou`. A client written against us-east-1 treats
        // the conflict as fatal, so getting this wrong breaks setup code that creates a
        // bucket unconditionally.
        Err(mock_s3_core::Error::BucketAlreadyOwnedByYou(_)) if engine.region() == "us-east-1" => {}
        Err(other) => return Err(other.into()),
    }

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

    if query.has("cors") {
        engine
            .delete_bucket_config(&bucket, mock_s3_core::bucket_config::CORS)
            .await?;
        return Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()));
    }

    // There is no request that un-versions a bucket, so this is not a setting that can
    // be cleared. Answering it is what keeps it from falling through to DeleteBucket and
    // destroying the bucket on a request that asked for nothing of the kind.
    if query.has("versioning") {
        return Err(S3Error::not_implemented(
            "clearing versioning, which S3 has no request for either",
        ));
    }

    // As with PUT: `DELETE /{bucket}?cors` clears a setting, it does not delete the
    // bucket. Getting this wrong would destroy data on a request meant to be harmless.
    if let Some(name) = configurable(&query) {
        engine.head_bucket(&bucket).await?;
        tracing::debug!(bucket, subresource = name, "bucket configuration cleared");
        return Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()));
    }

    // As above, and the stakes here are the highest in the file: falling through means
    // deleting the bucket on a request that asked for something else entirely.
    crate::ops::refuse_unhandled(&query, crate::ops::BUCKET_SUBRESOURCES, &[])?;

    engine.delete_bucket(&bucket).await?;

    Ok(response::empty(StatusCode::NO_CONTENT, HeaderMap::new()))
}

/// `OPTIONS /{bucket}` — a CORS preflight.
pub async fn options(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    crate::ops::preflight(&engine, &bucket, &headers).await
}

/// Bucket settings mock-s3 accepts and ignores.
///
/// It stores none of them, because nothing here acts on a lifecycle rule or a bucket
/// policy. Accepting them keeps tools working; pretending to enforce them would be
/// worse than refusing.
const CONFIGURABLE: &[&str] = &[
    "accelerate",
    "acl",
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

    // CORS is stored for real, because preflight has to evaluate it.
    if query.has("cors") {
        engine.head_bucket(bucket).await?;

        return match engine
            .get_bucket_config(bucket, mock_s3_core::bucket_config::CORS)
            .await?
        {
            Some(stored) => {
                let rules = crate::cors::parse(&stored)?;
                Ok(Some(response::xml(
                    StatusCode::OK,
                    xml::cors_configuration(&rules),
                )))
            }
            None => Err(S3Error::new(
                StatusCode::NOT_FOUND,
                "NoSuchCORSConfiguration",
                "The CORS configuration does not exist.",
            )),
        };
    }

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
        xml::versioning_configuration(engine.bucket_versioning(bucket).await?)
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

    if query.has("versions") {
        return list_versions(&engine, &bucket, &query, max_keys).await;
    }

    // `subresource` answered everything it recognises, so a subresource still named here
    // is one mock-s3 does not implement. Falling through would answer `?analytics` and
    // the rest with an ordinary bucket listing, which parses cleanly and means nothing.
    crate::ops::refuse_unhandled(&query, crate::ops::BUCKET_SUBRESOURCES, &["versions"])?;

    // Echoed back as the client sent it, while the engine gets the cursor inside it.
    //
    // `continuation-token=` with no value is echoed as an empty element rather than
    // omitted: S3 reports the parameter it was given, and a client that sent one expects
    // to read it back. Only a non-empty token becomes a cursor, so an empty one still
    // starts from the beginning.
    let sent_token = if query.has("continuation-token") {
        Some(query.get("continuation-token").unwrap_or_default())
    } else {
        None
    };
    let cursor = match (v2, sent_token) {
        // An empty token is not an invalid one; it just names no position.
        (true, Some("")) | (false, _) | (_, None) => None,
        (true, Some(raw)) => Some(crate::token::decode(raw).ok_or_else(|| {
            S3Error::invalid_argument(format!("continuation-token is not valid: {raw}"))
        })?),
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

/// `GET /{bucket}?versions` — ListObjectVersions.
///
/// Split out because a versions page resumes from a key *and* a version of that key,
/// which neither of the object listings has to deal with.
async fn list_versions(
    engine: &Engine,
    bucket: &str,
    query: &Query,
    max_keys: usize,
) -> Result<Response, S3Error> {
    let key_marker = query.non_empty("key-marker").map(str::to_string);
    let version_id_marker = query.non_empty("version-id-marker").map(str::to_string);

    // A version id is only meaningful against a key. Accepting it alone would let a
    // client with that bug pass here and fail against AWS, which is the one thing a mock
    // must not do.
    if key_marker.is_none() && version_id_marker.is_some() {
        return Err(S3Error::invalid_argument(
            "A version-id marker cannot be specified without a key marker.",
        ));
    }

    let request = VersionListRequest {
        prefix: query.get("prefix").unwrap_or_default().to_string(),
        delimiter: query.non_empty("delimiter").map(str::to_string),
        max_keys,
        key_marker,
        version_id_marker,
    };

    let result = engine.list_object_versions(bucket, &request).await?;

    let echo = VersionListingEcho {
        bucket,
        prefix: &request.prefix,
        delimiter: request.delimiter.as_deref(),
        max_keys: request.max_keys,
        key_marker: request.key_marker.as_deref(),
        version_id_marker: request.version_id_marker.as_deref(),
        encoding_type: query.non_empty("encoding-type"),
    };

    Ok(response::xml(
        StatusCode::OK,
        xml::list_versions_result(&echo, &result),
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
    let mut deleted: Vec<xml::Deleted> = Vec::new();
    let mut errors: Vec<(String, &'static str, String)> = Vec::new();
    for target in request.objects {
        match engine
            .delete_object_version(&bucket, &target.key, target.version_id.as_deref())
            .await
        {
            Ok(outcome) => deleted.push(xml::Deleted {
                key: target.key,
                version_id: target.version_id,
                delete_marker: outcome.delete_marker,
                delete_marker_version_id: outcome.version_id,
            }),
            Err(error) => {
                let mapped = S3Error::from(error);
                errors.push((target.key, mapped.code, mapped.message));
            }
        }
    }

    Ok(response::xml(
        StatusCode::OK,
        xml::delete_result(&deleted, &errors, request.quiet),
    ))
}
