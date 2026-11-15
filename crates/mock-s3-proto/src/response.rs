use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};

pub fn xml(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/xml")], body).into_response()
}

pub fn empty(status: StatusCode, headers: HeaderMap) -> Response {
    (status, headers).into_response()
}

/// Set a header, dropping it if the value cannot go on the wire.
///
/// Object metadata is caller-controlled, so a value with a newline or a non-ASCII byte
/// in it is possible. Dropping one header beats failing the whole response.
pub fn set(headers: &mut HeaderMap, name: HeaderName, value: &str) {
    match HeaderValue::from_str(value) {
        Ok(value) => {
            headers.insert(name, value);
        }
        Err(_) => tracing::debug!(%name, "dropping a header with an unrepresentable value"),
    }
}

/// Put one user metadata entry back on the wire as `x-amz-meta-<name>`.
///
/// The value goes back as the bytes it arrived as, not as UTF-8: it was decoded from
/// Latin-1 on the way in, so each `char` is one byte again here. Without that, a value
/// holding a byte above `0x7F` is dropped on the way out even though it was stored.
pub fn set_user_metadata(headers: &mut HeaderMap, key: &str, value: &str) {
    let Ok(name) = HeaderName::try_from(format!("x-amz-meta-{key}")) else {
        tracing::debug!(key, "dropping user metadata with an unrepresentable name");
        return;
    };

    let bytes: Vec<u8> = value
        .chars()
        .map(|character| u8::try_from(character as u32).unwrap_or(b'?'))
        .collect();

    match HeaderValue::from_bytes(&bytes) {
        Ok(value) => {
            headers.insert(name, value);
        }
        Err(_) => tracing::debug!(%name, "dropping user metadata with an unsendable value"),
    }
}
