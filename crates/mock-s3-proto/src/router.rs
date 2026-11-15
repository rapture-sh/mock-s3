use std::sync::Arc;

use axum::Router;
use axum::routing::{get, put};
use mock_s3_core::Engine;

use crate::ops;

/// The S3 API, in path-style addressing.
///
/// Path-style (`/{bucket}/{key}`) is what a client pointed at localhost uses, because
/// virtual-hosted style would need wildcard DNS. It is also why the console is on its
/// own port: here, the URL root *is* the bucket namespace.
pub fn router(engine: Arc<Engine>) -> Router {
    Router::new()
        .route("/", get(ops::service::list_buckets))
        .route(
            "/:bucket",
            put(ops::bucket::create)
                .get(ops::bucket::get)
                .head(ops::bucket::head)
                .delete(ops::bucket::delete)
                .post(ops::bucket::post),
        )
        .route(
            "/:bucket/*key",
            put(ops::object::put)
                .get(ops::object::get)
                .head(ops::object::head)
                .delete(ops::object::delete)
                .post(ops::object::post),
        )
        .with_state(engine)
}
