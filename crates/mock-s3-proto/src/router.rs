use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, put};
use mock_s3_core::Engine;

use crate::ops;

/// The S3 API, in path-style addressing.
///
/// Path-style (`/{bucket}/{key}`) is what a client pointed at localhost uses, because
/// virtual-hosted style would need wildcard DNS. It is also why the console is on its
/// own port: here, the URL root *is* the bucket namespace.
pub fn router(engine: Arc<Engine>) -> Router {
    let routes = Router::new()
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
        .fallback(unmatched)
        .with_state(engine);

    // The rewrite below has to happen *before* the route table is consulted, and a layer
    // added to a router runs after its routing. Wrapping the real router as the fallback
    // of an empty one puts the layer outside the matching it needs to precede.
    Router::new()
        .fallback_service(routes)
        .layer(axum::middleware::from_fn(normalize_bucket_path))
}

/// Strip the trailing slash from a bucket-level path.
///
/// The AWS SDKs address a bucket in path-style as `PUT /my-bucket/`, with a trailing
/// slash, which does not match a one-segment route. Only the single-segment case is
/// rewritten: a deeper path keeps its trailing slash, because a key is allowed to end in
/// `/` — that is exactly how a folder marker is stored.
async fn normalize_bucket_path(mut request: Request, next: Next) -> Response {
    let path = request.uri().path();

    let is_bucket_with_slash =
        path.len() > 2 && path.ends_with('/') && !path[1..path.len() - 1].contains('/');

    if is_bucket_with_slash {
        let trimmed = path[..path.len() - 1].to_string();
        let rebuilt = match request.uri().query() {
            Some(query) => format!("{trimmed}?{query}"),
            None => trimmed,
        };

        // A failure here means a path we just shortened stopped parsing, which should be
        // impossible; leaving the request untouched beats failing it.
        let mut parts = request.uri().clone().into_parts();
        match rebuilt.parse() {
            Ok(path_and_query) => {
                parts.path_and_query = Some(path_and_query);
                match http::Uri::from_parts(parts) {
                    Ok(uri) => *request.uri_mut() = uri,
                    Err(error) => tracing::debug!(%error, "could not rebuild the request URI"),
                }
            }
            Err(error) => tracing::debug!(%error, "could not parse the trimmed path"),
        }
    }

    next.run(request).await
}

/// Anything the route table does not cover.
///
/// Without this the framework answers with a bare `404` and an empty body, which a client
/// reports as a missing object and which tells whoever is debugging nothing. An S3 error
/// naming the method and path is the difference between a five-minute diagnosis and a
/// five-hour one.
async fn unmatched(method: http::Method, uri: http::Uri) -> crate::S3Error {
    tracing::warn!(%method, %uri, "no route matched");

    crate::S3Error::not_implemented(&format!("{method} {}", uri.path()))
}
