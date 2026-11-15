//! Decoding `aws-chunked` request bodies.
//!
//! When a client signs an upload with streaming SigV4, the payload does not arrive as
//! the object's bytes. It arrives wrapped in chunk framing:
//!
//! ```text
//! 10000;chunk-signature=<64 hex>\r\n
//! <65536 bytes>\r\n
//! 0;chunk-signature=<64 hex>\r\n
//! \r\n
//! ```
//!
//! `aws s3 cp` signs this way, so without this module the frame headers are stored as
//! part of the object: the stored size is larger than the source and the ETag does not
//! match. That failure looks like corruption rather than a missing feature, which is
//! what makes it worth a module of its own.
//!
//! Signatures are parsed off and discarded — mock-s3 never verifies them — and the
//! unsigned variant, whose chunk header is a bare hex size, is accepted too.

use bytes::Bytes;
use http::{HeaderMap, StatusCode};

use crate::error::S3Error;

const DECODED_LENGTH: &str = "x-amz-decoded-content-length";
const CONTENT_SHA256: &str = "x-amz-content-sha256";

/// Does this request carry a chunk-framed body?
///
/// Either marker is enough on its own: the `STREAMING-` sha256 values are what the AWS
/// SDKs set, while `Content-Encoding: aws-chunked` is what the specification describes
/// and what some other clients send.
pub fn is_chunked(headers: &HeaderMap) -> bool {
    let streaming_sha = headers
        .get(CONTENT_SHA256)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("STREAMING-"));

    let chunked_encoding = headers
        .get(http::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("aws-chunked"))
        });

    streaming_sha || chunked_encoding
}

/// Unwrap a request body, returning the object's real bytes.
///
/// A body that is not chunk-framed is returned untouched, so this is safe to call on
/// every upload.
pub fn decode(body: Bytes, headers: &HeaderMap) -> Result<Bytes, S3Error> {
    if !is_chunked(headers) {
        return Ok(body);
    }

    // S3 requires the decoded length for a chunked upload, and so does any
    // implementation that wants to notice a truncated body.
    let declared = headers
        .get(DECODED_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .ok_or_else(|| {
            S3Error::new(
                StatusCode::LENGTH_REQUIRED,
                "MissingContentLength",
                "A chunked upload must carry a valid x-amz-decoded-content-length header.",
            )
        })?;

    let decoded = decode_frames(&body).map_err(|reason| {
        S3Error::new(
            StatusCode::BAD_REQUEST,
            "IncompleteBody",
            format!("The chunked request body could not be decoded: {reason}"),
        )
    })?;

    if decoded.len() != declared {
        return Err(S3Error::new(
            StatusCode::BAD_REQUEST,
            "IncompleteBody",
            format!(
                "The chunked body decoded to {} bytes, but x-amz-decoded-content-length \
                 said {declared}.",
                decoded.len()
            ),
        ));
    }

    Ok(Bytes::from(decoded))
}

/// Walk the frames, concatenating their payloads.
///
/// Stops at the zero-length chunk; anything after it is trailing headers, which carry
/// checksums and signatures rather than object data.
fn decode_frames(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(body.len());
    let mut rest = body;

    loop {
        let line_end = find_crlf(rest).ok_or("a chunk header had no line ending")?;
        let header = &rest[..line_end];
        rest = &rest[line_end + 2..];

        // `<hex-size>` on its own, or `<hex-size>;chunk-signature=…`. Any extension
        // after the first semicolon is metadata we do not need.
        let size_field = header
            .split(|byte| *byte == b';')
            .next()
            .unwrap_or_default();
        let size_text = std::str::from_utf8(size_field)
            .map_err(|_| "a chunk size was not valid UTF-8".to_string())?
            .trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("a chunk size was not hexadecimal: {size_text:?}"))?;

        if size == 0 {
            return Ok(out);
        }

        if rest.len() < size {
            return Err(format!(
                "a chunk claimed {size} bytes but only {} remain",
                rest.len()
            ));
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];

        if !rest.starts_with(b"\r\n") {
            return Err("a chunk was not followed by a line ending".to_string());
        }
        rest = &rest[2..];
    }
}

fn find_crlf(haystack: &[u8]) -> Option<usize> {
    haystack.windows(2).position(|pair| pair == b"\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNATURE: &str =
        "chunk-signature=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn headers_for(decoded_length: Option<usize>, streaming: bool) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if streaming {
            headers.insert(
                CONTENT_SHA256,
                "STREAMING-AWS4-HMAC-SHA256-PAYLOAD".parse().unwrap(),
            );
        }
        if let Some(length) = decoded_length {
            headers.insert(DECODED_LENGTH, length.to_string().parse().unwrap());
        }
        headers
    }

    /// Build a signed chunked body out of the given payload slices.
    fn frame(chunks: &[&[u8]]) -> Bytes {
        let mut body: Vec<u8> = Vec::new();
        for chunk in chunks {
            body.extend_from_slice(format!("{:x};{SIGNATURE}\r\n", chunk.len()).as_bytes());
            body.extend_from_slice(chunk);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("0;{SIGNATURE}\r\n\r\n").as_bytes());
        Bytes::from(body)
    }

    mod is_chunked {
        use super::*;

        #[test]
        fn should_detect_a_streaming_sha256_marker() {
            assert!(is_chunked(&headers_for(None, true)));
        }

        #[test]
        fn should_detect_an_aws_chunked_content_encoding() {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::CONTENT_ENCODING,
                "aws-chunked".parse().unwrap(),
            );

            assert!(is_chunked(&headers));
        }

        #[test]
        fn should_detect_aws_chunked_alongside_another_encoding() {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::CONTENT_ENCODING,
                "gzip, aws-chunked".parse().unwrap(),
            );

            assert!(is_chunked(&headers));
        }

        #[test]
        fn should_not_detect_a_plain_request() {
            assert!(!is_chunked(&HeaderMap::new()));
        }

        #[test]
        fn should_not_mistake_another_encoding_for_chunking() {
            let mut headers = HeaderMap::new();
            headers.insert(http::header::CONTENT_ENCODING, "gzip".parse().unwrap());

            assert!(!is_chunked(&headers));
        }
    }

    mod decode {
        use super::*;

        #[test]
        fn should_return_a_plain_body_untouched() {
            let body = Bytes::from_static(b"not framed at all");

            let decoded = decode(body.clone(), &HeaderMap::new()).unwrap();

            assert_eq!(decoded, body);
        }

        #[test]
        fn should_unwrap_a_single_chunk() {
            let payload = b"hello from mock-s3";
            let body = frame(&[payload]);

            let decoded = decode(body, &headers_for(Some(payload.len()), true)).unwrap();

            assert_eq!(&decoded[..], payload);
        }

        #[test]
        fn should_join_several_chunks_in_order() {
            let body = frame(&[b"first ", b"second ", b"third"]);

            let decoded = decode(body, &headers_for(Some(18), true)).unwrap();

            assert_eq!(&decoded[..], b"first second third");
        }

        #[test]
        fn should_accept_the_unsigned_variant_with_a_bare_hex_size() {
            let mut body: Vec<u8> = Vec::new();
            body.extend_from_slice(b"5\r\nhello\r\n");
            body.extend_from_slice(b"0\r\n\r\n");

            let decoded = decode(Bytes::from(body), &headers_for(Some(5), true)).unwrap();

            assert_eq!(&decoded[..], b"hello");
        }

        #[test]
        fn should_ignore_trailing_headers_after_the_final_chunk() {
            let mut body: Vec<u8> = Vec::new();
            body.extend_from_slice(b"5;chunk-signature=abc\r\nhello\r\n");
            body.extend_from_slice(b"0;chunk-signature=abc\r\n");
            body.extend_from_slice(b"x-amz-checksum-crc32:tW4rCQ==\r\n");
            body.extend_from_slice(b"x-amz-trailer-signature:def\r\n\r\n");

            let decoded = decode(Bytes::from(body), &headers_for(Some(5), true)).unwrap();

            assert_eq!(&decoded[..], b"hello");
        }

        #[test]
        fn should_preserve_bytes_that_look_like_a_chunk_header() {
            // A payload containing CRLF and hex digits must not be re-parsed as framing.
            let payload = b"0\r\nnot-a-header\r\n";
            let body = frame(&[payload]);

            let decoded = decode(body, &headers_for(Some(payload.len()), true)).unwrap();

            assert_eq!(&decoded[..], payload);
        }

        #[test]
        fn should_reject_a_chunked_body_with_no_decoded_length_header() {
            let body = frame(&[b"hello"]);

            let error = decode(body, &headers_for(None, true)).unwrap_err();

            assert_eq!(error.code, "MissingContentLength");
            assert_eq!(error.status, StatusCode::LENGTH_REQUIRED);
        }

        #[test]
        fn should_reject_a_body_that_decodes_to_the_wrong_length() {
            let body = frame(&[b"hello"]);

            let error = decode(body, &headers_for(Some(99), true)).unwrap_err();

            assert_eq!(error.code, "IncompleteBody");
        }

        #[test]
        fn should_reject_a_truncated_chunk() {
            let body = Bytes::from_static(b"10;chunk-signature=abc\r\nshort");

            let error = decode(body, &headers_for(Some(16), true)).unwrap_err();

            assert_eq!(error.code, "IncompleteBody");
        }

        #[test]
        fn should_reject_a_chunk_size_that_is_not_hexadecimal() {
            let body = Bytes::from_static(b"zz;chunk-signature=abc\r\nhello\r\n0\r\n\r\n");

            let error = decode(body, &headers_for(Some(5), true)).unwrap_err();

            assert_eq!(error.code, "IncompleteBody");
        }
    }
}
