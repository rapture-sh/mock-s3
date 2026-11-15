//! Reading the caller out of a SigV4 `Authorization` header.
//!
//! Signatures are parsed, never verified: mock-s3 accepts any credentials by design. The
//! access key id is still worth having, because it identifies the caller in logs and is
//! what a future tenant model would key on.

use http::HeaderMap;

/// The access key id from an `AWS4-HMAC-SHA256` header, if there is one.
///
/// The header looks like:
///
/// ```text
/// AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260101/us-east-1/s3/aws4_request, \
///   SignedHeaders=host;x-amz-date, Signature=abc123
/// ```
pub fn access_key_id(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;

    let credential = value
        .split(',')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("Credential="))
        .or_else(|| {
            // Some clients put Credential first, right after the algorithm name.
            value
                .split_whitespace()
                .find_map(|part| part.trim_end_matches(',').strip_prefix("Credential="))
        })?;

    let key = credential.split('/').next()?;
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(authorization: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            authorization.parse().expect("test header is valid"),
        );
        headers
    }

    mod access_key_id {
        use super::*;

        #[test]
        fn should_read_the_key_from_a_sigv4_header() {
            let headers = headers_with(
                "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260101/us-east-1/s3/aws4_request, \
                 SignedHeaders=host, Signature=abc",
            );

            assert_eq!(access_key_id(&headers).as_deref(), Some("AKIAEXAMPLE"));
        }

        #[test]
        fn should_return_nothing_when_there_is_no_authorization_header() {
            assert!(access_key_id(&HeaderMap::new()).is_none());
        }

        #[test]
        fn should_return_nothing_for_an_unrecognised_scheme() {
            let headers = headers_with("Basic dXNlcjpwYXNz");

            assert!(access_key_id(&headers).is_none());
        }
    }
}
