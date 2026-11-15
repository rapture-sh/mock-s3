use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use http::StatusCode;
use mock_s3_core::Engine;

use crate::error::S3Error;
use crate::{response, xml};

/// `GET /` — ListBuckets.
pub async fn list_buckets(State(engine): State<Arc<Engine>>) -> Result<Response, S3Error> {
    let buckets = engine.list_buckets().await?;

    Ok(response::xml(
        StatusCode::OK,
        xml::list_all_my_buckets(&buckets),
    ))
}
