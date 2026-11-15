//! The console listener: a JSON admin API and the static web console.
//!
//! This runs on its own port, not a path prefix on the S3 API. In path-style addressing
//! the URL root is the bucket namespace, so a console mounted at `/console` would make a
//! bucket named `console` unreachable.
//!
//! Everything here goes through the same [`Engine`] the S3 API uses, so the console can
//! only do what a client could already do over S3. It exists because the console cannot
//! call the S3 port itself: that is a different origin, and asking someone to configure
//! CORS before they can look at their own test fixtures would be absurd.
//!
//! The admin API is unauthenticated, like the rest of mock-s3. It can create and delete
//! buckets and objects, which is no more than `POST /_admin/reset` could already do.
//! See `SECURITY.md`: bind this to localhost or a container network.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use http::StatusCode;
use mock_s3_core::{Engine, ListRequest};
use mock_s3_proto::S3Error;
use serde::{Deserialize, Serialize};

pub mod assets;

/// An admin API failure, as JSON.
///
/// The status and code come from [`S3Error`] rather than a second table, so the console
/// and an SDK are told the same thing about the same failure. Only the shape differs:
/// the console reads JSON, and making it parse XML to find out a bucket name was taken
/// would be a silly tax.
pub struct AdminError(S3Error);

#[derive(Serialize)]
struct AdminErrorBody {
    error: &'static str,
    message: String,
}

impl From<mock_s3_core::Error> for AdminError {
    fn from(error: mock_s3_core::Error) -> Self {
        Self(S3Error::from(error))
    }
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        (
            self.0.status,
            Json(AdminErrorBody {
                error: self.0.code,
                message: self.0.message,
            }),
        )
            .into_response()
    }
}

type AdminResult<T> = Result<T, AdminError>;

#[derive(Serialize)]
pub struct Health {
    status: &'static str,
    region: String,
    /// `memory`, or the data directory this instance is writing to. A developer asking
    /// why their objects vanished on restart is asking this question.
    backend: String,
    buckets: usize,
}

#[derive(Serialize)]
pub struct BucketSummary {
    name: String,
    created_at: String,
    objects: usize,
    bytes: u64,
}

#[derive(Serialize)]
pub struct BucketDetail {
    name: String,
    created_at: String,
    objects: usize,
    bytes: u64,
    /// `Unversioned`, `Enabled` or `Suspended`, spelled as S3 spells it.
    versioning: String,
}

#[derive(Serialize)]
pub struct ObjectSummary {
    key: String,
    size: u64,
    etag: String,
    last_modified: String,
    content_type: String,
}

#[derive(Serialize)]
pub struct ObjectListing {
    /// Keys that roll up under the delimiter, so the console can show folders.
    prefixes: Vec<String>,
    objects: Vec<ObjectSummary>,
    is_truncated: bool,
}

#[derive(Serialize)]
pub struct ObjectVersionSummary {
    version_id: String,
    is_latest: bool,
    is_delete_marker: bool,
    last_modified: String,
    size: u64,
    etag: String,
}

#[derive(Serialize)]
pub struct ObjectDetail {
    key: String,
    size: u64,
    etag: String,
    last_modified: String,
    content_type: String,
    metadata: std::collections::BTreeMap<String, String>,
    /// Newest first. Empty in a bucket that was never versioned.
    versions: Vec<ObjectVersionSummary>,
}

#[derive(Serialize)]
pub struct ResetReport {
    reset: &'static str,
    objects_removed: Option<usize>,
}

#[derive(Deserialize)]
pub struct CreateBucket {
    name: String,
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    prefix: Option<String>,
    delimiter: Option<String>,
    limit: Option<usize>,
}

/// An object key, as a query parameter rather than a path segment.
///
/// A key may contain `/`, `?`, `#` and anything else, so routing on it would mean
/// wildcard paths and a second round of escaping. One query parameter sidesteps all of
/// it.
#[derive(Deserialize)]
pub struct KeyQuery {
    key: String,
}

#[derive(Deserialize)]
pub struct RequestQuery {
    limit: Option<usize>,
}

/// What the console listener needs: the engine, the request log the S3 listener writes
/// to, and a label for the backend in use.
#[derive(Clone)]
pub struct ConsoleState {
    pub engine: Arc<Engine>,
    pub request_log: Arc<mock_s3_proto::RequestLog>,
    pub backend: String,
}

pub fn router(
    engine: Arc<Engine>,
    request_log: Arc<mock_s3_proto::RequestLog>,
    backend: String,
) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/_admin/health", get(health))
        .route("/_admin/buckets", get(buckets).post(create_bucket))
        .route(
            "/_admin/buckets/:bucket",
            get(bucket_detail).delete(delete_bucket),
        )
        .route("/_admin/buckets/:bucket/objects", get(browse_objects))
        .route(
            "/_admin/buckets/:bucket/object",
            get(object_detail)
                .put(put_object)
                .delete(delete_object)
                // An upload is as large as the file someone dragged in, and axum's
                // default cap is 2 MB.
                .layer(DefaultBodyLimit::disable()),
        )
        .route("/_admin/requests", get(requests))
        .route("/_admin/reset", post(reset))
        .route("/_admin/reset/:bucket", post(reset_bucket))
        .fallback(get(index))
        .with_state(ConsoleState {
            engine,
            request_log,
            backend,
        })
}

async fn index() -> Response {
    assets::index()
}

async fn health(State(state): State<ConsoleState>) -> AdminResult<Json<Health>> {
    let buckets = state.engine.list_buckets().await?;

    Ok(Json(Health {
        status: "ok",
        region: state.engine.region().to_string(),
        backend: state.backend.clone(),
        buckets: buckets.len(),
    }))
}

/// Object count and total size for one bucket, from one listing.
async fn measure(state: &ConsoleState, bucket: &str) -> AdminResult<(usize, u64)> {
    let listing = state
        .engine
        .list_objects(bucket, &ListRequest::default())
        .await?;

    Ok((
        listing.objects.len(),
        listing.objects.iter().map(|object| object.size).sum(),
    ))
}

async fn buckets(State(state): State<ConsoleState>) -> AdminResult<Json<Vec<BucketSummary>>> {
    let mut summaries = Vec::new();

    for bucket in state.engine.list_buckets().await? {
        let (objects, bytes) = measure(&state, &bucket.name).await?;
        summaries.push(BucketSummary {
            name: bucket.name,
            created_at: bucket.created_at.to_rfc3339(),
            objects,
            bytes,
        });
    }

    Ok(Json(summaries))
}

async fn create_bucket(
    State(state): State<ConsoleState>,
    Json(request): Json<CreateBucket>,
) -> AdminResult<(StatusCode, Json<BucketSummary>)> {
    // The engine validates the name against the real S3 rules, so a name the console
    // accepts is one AWS would accept too.
    state.engine.create_bucket(&request.name).await?;
    tracing::info!(bucket = %request.name, "admin: bucket created");

    let created = state
        .engine
        .list_buckets()
        .await?
        .into_iter()
        .find(|bucket| bucket.name == request.name)
        .map(|bucket| bucket.created_at.to_rfc3339())
        .unwrap_or_default();

    Ok((
        StatusCode::CREATED,
        Json(BucketSummary {
            name: request.name,
            created_at: created,
            objects: 0,
            bytes: 0,
        }),
    ))
}

async fn bucket_detail(
    State(state): State<ConsoleState>,
    Path(bucket): Path<String>,
) -> AdminResult<Json<BucketDetail>> {
    let info = state
        .engine
        .list_buckets()
        .await?
        .into_iter()
        .find(|candidate| candidate.name == bucket)
        .ok_or(mock_s3_core::Error::NoSuchBucket(bucket.clone()))?;

    let (objects, bytes) = measure(&state, &bucket).await?;
    let versioning = state.engine.bucket_versioning(&bucket).await?;

    Ok(Json(BucketDetail {
        name: info.name,
        created_at: info.created_at.to_rfc3339(),
        objects,
        bytes,
        versioning: match versioning {
            mock_s3_core::VersioningStatus::Unversioned => "Unversioned".to_string(),
            other => other.as_str().to_string(),
        },
    }))
}

/// Deleting a bucket that still holds objects fails, exactly as it does over S3. The
/// console offers purge separately rather than quietly emptying it first — losing
/// fixtures to a misclick is worse than a second click.
async fn delete_bucket(
    State(state): State<ConsoleState>,
    Path(bucket): Path<String>,
) -> AdminResult<StatusCode> {
    state.engine.delete_bucket(&bucket).await?;
    tracing::info!(bucket, "admin: bucket deleted");

    Ok(StatusCode::NO_CONTENT)
}

async fn browse_objects(
    State(state): State<ConsoleState>,
    Path(bucket): Path<String>,
    Query(query): Query<BrowseQuery>,
) -> AdminResult<Json<ObjectListing>> {
    let request = ListRequest {
        prefix: query.prefix.unwrap_or_default(),
        // Defaults to folder-style browsing, which is how people read a key space.
        delimiter: Some(query.delimiter.unwrap_or_else(|| "/".to_string()))
            .filter(|delimiter| !delimiter.is_empty()),
        max_keys: query.limit.unwrap_or(1000).min(1000),
        start_after: None,
        continuation_token: None,
    };

    let listing = state.engine.list_objects(&bucket, &request).await?;

    Ok(Json(ObjectListing {
        prefixes: listing.common_prefixes,
        objects: listing
            .objects
            .into_iter()
            .map(|object| ObjectSummary {
                key: object.key,
                size: object.size,
                etag: object.etag,
                last_modified: object.last_modified.to_rfc3339(),
                content_type: object.content_type,
            })
            .collect(),
        is_truncated: listing.is_truncated,
    }))
}

async fn object_detail(
    State(state): State<ConsoleState>,
    Path(bucket): Path<String>,
    Query(query): Query<KeyQuery>,
) -> AdminResult<Json<ObjectDetail>> {
    let meta = state.engine.head_object(&bucket, &query.key).await?;

    let versions = state
        .engine
        .object_versions(&bucket, &query.key)
        .await?
        .into_iter()
        .map(|version| ObjectVersionSummary {
            version_id: version.version_id.clone(),
            is_latest: version.is_latest,
            is_delete_marker: version.is_delete_marker(),
            last_modified: version.last_modified.to_rfc3339(),
            size: version.meta.as_ref().map(|meta| meta.size).unwrap_or(0),
            etag: version
                .meta
                .as_ref()
                .map(|meta| meta.etag.clone())
                .unwrap_or_default(),
        })
        .collect();

    Ok(Json(ObjectDetail {
        key: meta.key,
        size: meta.size,
        etag: meta.etag,
        last_modified: meta.last_modified.to_rfc3339(),
        content_type: meta.content_type,
        metadata: meta.user_metadata,
        versions,
    }))
}

async fn put_object(
    State(state): State<ConsoleState>,
    Path(bucket): Path<String>,
    Query(query): Query<KeyQuery>,
    headers: http::HeaderMap,
    body: Bytes,
) -> AdminResult<(StatusCode, Json<ObjectSummary>)> {
    let content_type = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or(mock_s3_core::model::DEFAULT_CONTENT_TYPE)
        .to_string();

    let options = mock_s3_core::PutOptions::with_content_type(content_type);
    let meta = state
        .engine
        .put_object(&bucket, &query.key, body, options)
        .await?;

    tracing::info!(bucket, key = %query.key, size = meta.size, "admin: object uploaded");

    Ok((
        StatusCode::CREATED,
        Json(ObjectSummary {
            key: meta.key,
            size: meta.size,
            etag: meta.etag,
            last_modified: meta.last_modified.to_rfc3339(),
            content_type: meta.content_type,
        }),
    ))
}

async fn delete_object(
    State(state): State<ConsoleState>,
    Path(bucket): Path<String>,
    Query(query): Query<KeyQuery>,
) -> AdminResult<StatusCode> {
    state.engine.delete_object(&bucket, &query.key).await?;
    tracing::info!(bucket, key = %query.key, "admin: object deleted");

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /_admin/requests` — what clients have actually been sending, newest first.
async fn requests(
    State(state): State<ConsoleState>,
    Query(params): Query<RequestQuery>,
) -> Json<Vec<mock_s3_proto::RequestEntry>> {
    Json(
        state
            .request_log
            .recent(params.limit.unwrap_or(100).min(1000)),
    )
}

async fn reset(State(state): State<ConsoleState>) -> AdminResult<Json<ResetReport>> {
    state.engine.reset().await?;
    tracing::info!("admin reset: all buckets dropped");

    Ok(Json(ResetReport {
        reset: "all",
        objects_removed: None,
    }))
}

async fn reset_bucket(
    State(state): State<ConsoleState>,
    Path(bucket): Path<String>,
) -> AdminResult<(StatusCode, Json<ResetReport>)> {
    let removed = state.engine.reset_bucket(&bucket).await?;
    tracing::info!(bucket, removed, "admin reset: bucket emptied");

    Ok((
        StatusCode::OK,
        Json(ResetReport {
            reset: "bucket",
            objects_removed: Some(removed),
        }),
    ))
}
