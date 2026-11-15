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

pub fn router(engine: Arc<Engine>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/_admin/health", get(health))
        .route("/_admin/buckets", get(buckets))
        .route("/_admin/reset", post(reset))
        .route("/_admin/reset/:bucket", post(reset_bucket))
        .with_state(engine)
}

async fn index() -> axum::response::Response {
    assets::index()
}

async fn health(State(engine): State<Arc<Engine>>) -> Result<Json<Health>, S3Error> {
    let buckets = engine.list_buckets().await?;

    Ok(Json(Health {
        status: "ok",
        region: engine.region().to_string(),
        buckets: buckets.len(),
    }))
}

async fn buckets(State(engine): State<Arc<Engine>>) -> Result<Json<Vec<BucketSummary>>, S3Error> {
    let mut summaries = Vec::new();

    for bucket in engine.list_buckets().await? {
        let listing = engine
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

async fn reset(State(engine): State<Arc<Engine>>) -> Result<Json<ResetReport>, S3Error> {
    engine.reset().await?;
    tracing::info!("admin reset: all buckets dropped");

    Ok(Json(ResetReport {
        reset: "all",
        objects_removed: None,
    }))
}

async fn reset_bucket(
    State(engine): State<Arc<Engine>>,
    Path(bucket): Path<String>,
) -> Result<(StatusCode, Json<ResetReport>), S3Error> {
    let removed = engine.reset_bucket(&bucket).await?;
    tracing::info!(bucket, removed, "admin reset: bucket emptied");

    Ok((
        StatusCode::OK,
        Json(ResetReport {
            reset: "bucket",
            objects_removed: Some(removed),
        }),
    ))
}
