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
/// Wire-level settings that are not the engine's business.
#[derive(Clone)]
pub struct ProtoConfig {
    /// Host suffix under which `<bucket>.<suffix>` addresses a bucket.
    ///
    /// Off by default: with it unset, a request to `localhost` or an IP can never be
    /// misread as naming a bucket, which matters because the first label of any hostname
    /// would otherwise be treated as one.
    pub base_domain: Option<String>,

    /// Where recent requests are recorded for the admin API. Shared with the console.
    pub request_log: Arc<crate::RequestLog>,
}

impl Default for ProtoConfig {
    fn default() -> Self {
        Self {
            base_domain: None,
            request_log: Arc::new(crate::RequestLog::default()),
        }
    }
}

pub fn router(engine: Arc<Engine>, config: ProtoConfig) -> Router {
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
        // The framework caps request bodies at 2 MB by default, which answers any
        // ordinary upload with a 413. S3 accepts 5 GB in a single PUT, so the cap has to
        // go: an object storage server that cannot take a 3 MB file is not one.
        //
        // That does mean a large upload is held in memory. For a mock bound to localhost
        // that is the intended trade — see SECURITY.md.
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(engine);

    // The rewrite below has to happen *before* the route table is consulted, and a layer
    // added to a router runs after its routing. Wrapping the real router as the fallback
    // of an empty one puts the layer outside the matching it needs to precede.
    let vhost = config.clone();
    let log = config.request_log.clone();

    Router::new()
        .fallback_service(routes)
        .layer(axum::middleware::from_fn(attach_request_id))
        .layer(axum::middleware::from_fn(normalize_bucket_path))
        .layer(axum::middleware::from_fn(move |request, next| {
            let config = vhost.clone();
            async move { rewrite_virtual_host(request, next, config).await }
        }))
        // Outermost, so it records the request as the client sent it and the response
        // as the client will see it — which is the question this log exists to answer.
        .layer(axum::middleware::from_fn(move |request, next| {
            let log = log.clone();
            async move { observe(request, next, log).await }
        }))
}

/// Log a request and record it for the admin API.
async fn observe(request: Request, next: Next, log: Arc<crate::RequestLog>) -> Response {
    let started = std::time::Instant::now();

    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(str::to_string);
    let access_key_id = crate::sigv4::access_key_id(request.headers());

    let response = next.run(request).await;

    let status = response.status();
    let duration_ms = started.elapsed().as_millis() as u64;
    let request_id = response
        .headers()
        .get(crate::request_id::HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (bucket, key) = crate::request_log::split_path(&path);

    tracing::info!(
        method = %method,
        path = %path,
        status = status.as_u16(),
        duration_ms,
        bucket = bucket.as_deref().unwrap_or("-"),
        caller = access_key_id.as_deref().unwrap_or("-"),
        "request"
    );

    log.record(crate::RequestEntry {
        at: chrono::Utc::now(),
        method,
        path,
        query,
        status: status.as_u16(),
        duration_ms,
        bucket,
        key,
        access_key_id,
        request_id,
    });

    response
}

/// Turn a virtual-hosted request into a path-style one.
///
/// `GET /key` with `Host: my-bucket.s3.local` becomes `GET /my-bucket/key`, so the route
/// table only ever has to describe one addressing style. Clients that cannot be put into
/// path-style mode need this; it does nothing unless `--base-domain` is set.
async fn rewrite_virtual_host(mut request: Request, next: Next, config: ProtoConfig) -> Response {
    let Some(base_domain) = config.base_domain.as_deref() else {
        return next.run(request).await;
    };

    let host = request
        .headers()
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());

    if let Some(host) = host
        && let Some(bucket) = bucket_from_host(&host, base_domain)
    {
        let path = request.uri().path();
        let rebuilt = match request.uri().query() {
            Some(query) => format!("/{bucket}{path}?{query}"),
            None => format!("/{bucket}{path}"),
        };

        let mut parts = request.uri().clone().into_parts();
        match rebuilt.parse() {
            Ok(path_and_query) => {
                parts.path_and_query = Some(path_and_query);
                match http::Uri::from_parts(parts) {
                    Ok(uri) => {
                        tracing::debug!(%host, bucket, "virtual-hosted request rewritten");
                        *request.uri_mut() = uri;
                    }
                    Err(error) => tracing::debug!(%error, "could not rebuild the request URI"),
                }
            }
            Err(error) => tracing::debug!(%error, "could not parse the rewritten path"),
        }
    }

    next.run(request).await
}

/// The bucket named by a `Host` header, if it names one.
fn bucket_from_host(host: &str, base_domain: &str) -> Option<String> {
    // The header carries the port; the base domain does not.
    let hostname = host.split(':').next()?.trim_end_matches('.');
    let suffix = format!(".{}", base_domain.trim_start_matches('.'));

    let bucket = hostname.strip_suffix(&suffix)?;
    if bucket.is_empty() {
        return None;
    }
    Some(bucket.to_string())
}

/// Give every response a request id, the way S3 does.
///
/// An error response has already set one — it has to match the id inside the XML body —
/// so this only fills in the gap for successful responses.
async fn attach_request_id(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;

    let name = http::HeaderName::from_static(crate::request_id::HEADER);
    let headers = response.headers_mut();
    if !headers.contains_key(&name)
        && let Ok(value) = http::HeaderValue::from_str(&crate::request_id::next())
    {
        headers.insert(name, value);
    }

    response
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

#[cfg(test)]
mod tests {
    use super::*;

    mod bucket_from_host {
        use super::*;

        #[test]
        fn should_read_the_bucket_from_a_matching_host() {
            let found = bucket_from_host("my-bucket.s3.local", "s3.local");

            assert_eq!(found.as_deref(), Some("my-bucket"));
        }

        #[test]
        fn should_ignore_the_port() {
            let found = bucket_from_host("my-bucket.s3.local:9000", "s3.local");

            assert_eq!(found.as_deref(), Some("my-bucket"));
        }

        #[test]
        fn should_return_nothing_for_the_bare_base_domain() {
            assert!(bucket_from_host("s3.local", "s3.local").is_none());
            assert!(bucket_from_host("s3.local:9000", "s3.local").is_none());
        }

        #[test]
        fn should_return_nothing_for_an_unrelated_host() {
            assert!(bucket_from_host("localhost:9000", "s3.local").is_none());
            assert!(bucket_from_host("127.0.0.1:9000", "s3.local").is_none());
        }

        #[test]
        fn should_accept_a_bucket_name_containing_dots() {
            // Legal in S3, and only awkward because of TLS, which does not apply here.
            let found = bucket_from_host("my.bucket.s3.local", "s3.local");

            assert_eq!(found.as_deref(), Some("my.bucket"));
        }

        #[test]
        fn should_tolerate_a_base_domain_written_with_a_leading_dot() {
            let found = bucket_from_host("my-bucket.s3.local", ".s3.local");

            assert_eq!(found.as_deref(), Some("my-bucket"));
        }
    }
}
