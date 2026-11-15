//! The console listener: a JSON admin API and the static web console.
//!
//! This runs on its own port, not a path prefix on the S3 API. In path-style addressing
//! the URL root is the bucket namespace, so a console mounted at `/console` would make a
//! bucket named `console` unreachable.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use http::StatusCode;
use mock_s3_core::Engine;
use mock_s3_proto::S3Error;
use serde::Serialize;

pub mod assets;

#[derive(Serialize)]
pub struct Health {
    status: &'static str,
    region: String,
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
pub struct ResetReport {
    reset: &'static str,
    objects_removed: Option<usize>,
}

/// What the console listener needs: the engine, and the request log the S3 listener
/// writes to.
#[derive(Clone)]
pub struct ConsoleState {
    pub engine: Arc<Engine>,
    pub request_log: Arc<mock_s3_proto::RequestLog>,
}

pub fn router(engine: Arc<Engine>, request_log: Arc<mock_s3_proto::RequestLog>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/_admin/health", get(health))
        .route("/_admin/buckets", get(buckets))
        .route("/_admin/requests", get(requests))
        .route("/_admin/reset", post(reset))
        .route("/_admin/reset/:bucket", post(reset_bucket))
        .with_state(ConsoleState {
            engine,
            request_log,
        })
}

/// `GET /_admin/requests` — what clients have actually been sending, newest first.
async fn requests(
    State(state): State<ConsoleState>,
    axum::extract::Query(params): axum::extract::Query<RequestQuery>,
) -> Json<Vec<mock_s3_proto::RequestEntry>> {
    Json(
        state
            .request_log
            .recent(params.limit.unwrap_or(100).min(1000)),
    )
}

#[derive(serde::Deserialize)]
pub struct RequestQuery {
    limit: Option<usize>,
}

async fn index() -> axum::response::Response {
    assets::index()
}

async fn health(State(state): State<ConsoleState>) -> Result<Json<Health>, S3Error> {
    let buckets = state.engine.list_buckets().await?;

    Ok(Json(Health {
        status: "ok",
        region: state.engine.region().to_string(),
        buckets: buckets.len(),
    }))
}

async fn buckets(State(state): State<ConsoleState>) -> Result<Json<Vec<BucketSummary>>, S3Error> {
    let mut summaries = Vec::new();

    for bucket in state.engine.list_buckets().await? {
        let listing = state
            .engine
            .list_objects(&bucket.name, &mock_s3_core::ListRequest::default())
            .await?;
        summaries.push(BucketSummary {
            name: bucket.name,
            created_at: bucket.created_at.to_rfc3339(),
            objects: listing.objects.len(),
            bytes: listing.objects.iter().map(|object| object.size).sum(),
        });
    }

    Ok(Json(summaries))
}

async fn reset(State(state): State<ConsoleState>) -> Result<Json<ResetReport>, S3Error> {
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
) -> Result<(StatusCode, Json<ResetReport>), S3Error> {
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
