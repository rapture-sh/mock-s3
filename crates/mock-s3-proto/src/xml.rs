//! The S3 wire format.
//!
//! This is the only module allowed to produce XML. Handlers call these functions; they
//! never format a response body themselves, so the exact shape S3 uses lives in one
//! place and can be tested on its own.

use std::fmt::Write as _;

use chrono::{DateTime, SecondsFormat, Utc};
use mock_s3_core::{BucketInfo, ListResult};

const DECLARATION: &str = r#"<?xml version="1.0" encoding="UTF-8"?>"#;
const NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The owner S3 reports. mock-s3 has no notion of accounts, so every object has the
/// same one; a fixed value is more useful to a client than an empty element.
const OWNER_ID: &str = "mock-s3";
const OWNER_DISPLAY_NAME: &str = "mock-s3";

/// Escape text for an XML text node. Object keys are arbitrary strings, so this is not
/// optional: a key containing `&` or `<` would otherwise produce a malformed document.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
    out
}

/// The timestamp format S3 uses in XML bodies: ISO 8601, milliseconds, UTC.
pub fn xml_timestamp(at: &DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The timestamp format S3 uses in the `Last-Modified` header: an HTTP date.
pub fn http_timestamp(at: &DateTime<Utc>) -> String {
    at.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

pub fn error_document(
    code: &str,
    message: &str,
    resource: Option<&str>,
    request_id: &str,
) -> String {
    let mut out = String::new();
    let _ = write!(out, "{DECLARATION}<Error>");
    let _ = write!(out, "<Code>{}</Code>", escape(code));
    let _ = write!(out, "<Message>{}</Message>", escape(message));
    if let Some(resource) = resource {
        let _ = write!(out, "<Resource>{}</Resource>", escape(resource));
    }
    let _ = write!(out, "<RequestId>{}</RequestId>", escape(request_id));
    out.push_str("</Error>");
    out
}

pub fn list_all_my_buckets(buckets: &[BucketInfo]) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{DECLARATION}<ListAllMyBucketsResult xmlns=\"{NAMESPACE}\">"
    );
    let _ = write!(
        out,
        "<Owner><ID>{OWNER_ID}</ID><DisplayName>{OWNER_DISPLAY_NAME}</DisplayName></Owner>"
    );
    out.push_str("<Buckets>");
    for bucket in buckets {
        let _ = write!(
            out,
            "<Bucket><Name>{}</Name><CreationDate>{}</CreationDate></Bucket>",
            escape(&bucket.name),
            xml_timestamp(&bucket.created_at)
        );
    }
    out.push_str("</Buckets></ListAllMyBucketsResult>");
    out
}

/// Arguments echoed back in a listing response. S3 repeats the request parameters, and
/// SDKs read some of them back, so they are carried through rather than dropped.
pub struct ListingEcho<'a> {
    pub bucket: &'a str,
    pub prefix: &'a str,
    pub delimiter: Option<&'a str>,
    pub max_keys: usize,
    pub continuation_token: Option<&'a str>,
    pub start_after: Option<&'a str>,
    /// `encoding-type`, as the caller sent it. `url` means keys come back
    /// percent-encoded, which is the only way a client can tell a key containing `+`
    /// from one containing a space.
    pub encoding_type: Option<&'a str>,
    /// `list-type=2` responses carry `KeyCount` and continuation tokens; version 1
    /// responses carry `Marker` instead.
    pub v2: bool,
}

/// Everything outside the unreserved set, which is how S3 encodes a key.
const URL_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn wants_url_encoding(encoding_type: Option<&str>) -> bool {
    encoding_type.is_some_and(|value| value.eq_ignore_ascii_case("url"))
}

/// A key, prefix or delimiter on its way into a listing response.
fn listing_value(value: &str, encoding_type: Option<&str>) -> String {
    if wants_url_encoding(encoding_type) {
        let encoded = percent_encoding::utf8_percent_encode(value, URL_ENCODE_SET).to_string();
        escape(&encoded)
    } else {
        escape(value)
    }
}

pub fn list_bucket_result(echo: &ListingEcho<'_>, result: &ListResult) -> String {
    let mut out = String::new();
    let _ = write!(out, "{DECLARATION}<ListBucketResult xmlns=\"{NAMESPACE}\">");
    let _ = write!(out, "<Name>{}</Name>", escape(echo.bucket));
    let _ = write!(
        out,
        "<Prefix>{}</Prefix>",
        listing_value(echo.prefix, echo.encoding_type)
    );
    let _ = write!(out, "<MaxKeys>{}</MaxKeys>", echo.max_keys);
    let _ = write!(
        out,
        "<IsTruncated>{}</IsTruncated>",
        if result.is_truncated { "true" } else { "false" }
    );
    if let Some(delimiter) = echo.delimiter {
        let _ = write!(
            out,
            "<Delimiter>{}</Delimiter>",
            listing_value(delimiter, echo.encoding_type)
        );
    }
    if wants_url_encoding(echo.encoding_type) {
        out.push_str("<EncodingType>url</EncodingType>");
    }

    if echo.v2 {
        let _ = write!(out, "<KeyCount>{}</KeyCount>", result.key_count);
        if let Some(token) = echo.continuation_token {
            let _ = write!(
                out,
                "<ContinuationToken>{}</ContinuationToken>",
                escape(token)
            );
        }
        // The engine paginates on a key; the wire gets an opaque token, so a key
        // containing `+` survives the round trip through a query string.
        if let Some(cursor) = &result.next_continuation_token {
            let _ = write!(
                out,
                "<NextContinuationToken>{}</NextContinuationToken>",
                escape(&crate::token::encode(cursor))
            );
        }
        if let Some(start_after) = echo.start_after {
            let _ = write!(
                out,
                "<StartAfter>{}</StartAfter>",
                listing_value(start_after, echo.encoding_type)
            );
        }
    } else {
        let _ = write!(
            out,
            "<Marker>{}</Marker>",
            listing_value(echo.start_after.unwrap_or_default(), echo.encoding_type)
        );
        // Version 1 pagination resumes from NextMarker, which is a key rather than an
        // opaque token, so it is encoded the same way as any other key.
        if let Some(cursor) = &result.next_continuation_token {
            let _ = write!(
                out,
                "<NextMarker>{}</NextMarker>",
                listing_value(cursor, echo.encoding_type)
            );
        }
    }

    for object in &result.objects {
        let _ = write!(out, "<Contents>");
        let _ = write!(
            out,
            "<Key>{}</Key>",
            listing_value(&object.key, echo.encoding_type)
        );
        let _ = write!(
            out,
            "<LastModified>{}</LastModified>",
            xml_timestamp(&object.last_modified)
        );
        let _ = write!(out, "<ETag>&quot;{}&quot;</ETag>", escape(&object.etag));
        let _ = write!(out, "<Size>{}</Size>", object.size);
        out.push_str("<StorageClass>STANDARD</StorageClass>");
        let _ = write!(
            out,
            "<Owner><ID>{OWNER_ID}</ID><DisplayName>{OWNER_DISPLAY_NAME}</DisplayName></Owner>"
        );
        out.push_str("</Contents>");
    }

    for prefix in &result.common_prefixes {
        let _ = write!(
            out,
            "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
            listing_value(prefix, echo.encoding_type)
        );
    }

    out.push_str("</ListBucketResult>");
    out
}

pub fn location_constraint(region: &str) -> String {
    // us-east-1 is reported as an empty constraint, which is what real S3 does.
    let body = if region == "us-east-1" { "" } else { region };
    format!(
        "{DECLARATION}<LocationConstraint xmlns=\"{NAMESPACE}\">{}</LocationConstraint>",
        escape(body)
    )
}

pub fn versioning_configuration() -> String {
    format!(
        "{DECLARATION}<VersioningConfiguration xmlns=\"{NAMESPACE}\"></VersioningConfiguration>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use mock_s3_core::ObjectMeta;

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).unwrap()
    }

    mod escape {
        use super::*;

        #[test]
        fn should_escape_a_key_containing_xml_syntax() {
            assert_eq!(escape("a&b<c>"), "a&amp;b&lt;c&gt;");
        }

        #[test]
        fn should_leave_a_plain_key_untouched() {
            assert_eq!(escape("photos/cat.jpg"), "photos/cat.jpg");
        }
    }

    mod xml_timestamp {
        use super::*;

        #[test]
        fn should_render_milliseconds_and_a_zulu_suffix() {
            assert_eq!(xml_timestamp(&at(0)), "1970-01-01T00:00:00.000Z");
        }
    }

    mod http_timestamp {
        use super::*;

        #[test]
        fn should_render_an_http_date() {
            assert_eq!(http_timestamp(&at(0)), "Thu, 01 Jan 1970 00:00:00 GMT");
        }
    }

    mod error_document {
        use super::*;

        #[test]
        fn should_carry_the_code_and_message() {
            let body = error_document("NoSuchKey", "nope", Some("k"), "ID");

            assert!(body.contains("<Code>NoSuchKey</Code>"));
            assert!(body.contains("<Message>nope</Message>"));
            assert!(body.contains("<Resource>k</Resource>"));
        }

        #[test]
        fn should_omit_the_resource_when_there_is_none() {
            let body = error_document("InternalError", "boom", None, "ID");

            assert!(!body.contains("<Resource>"));
        }
    }

    mod list_bucket_result {
        use super::*;

        fn echo<'a>(bucket: &'a str) -> ListingEcho<'a> {
            ListingEcho {
                bucket,
                prefix: "",
                delimiter: None,
                max_keys: 1000,
                continuation_token: None,
                start_after: None,
                encoding_type: None,
                v2: true,
            }
        }

        #[test]
        fn should_leave_keys_unencoded_when_no_encoding_type_is_asked_for() {
            let result = ListResult {
                objects: vec![object("a b+c.txt")],
                key_count: 1,
                ..Default::default()
            };

            let body = list_bucket_result(&echo("b"), &result);

            assert!(body.contains("<Key>a b+c.txt</Key>"));
            assert!(!body.contains("<EncodingType>"));
        }

        #[test]
        fn should_percent_encode_keys_when_encoding_type_is_url() {
            let mut echo = echo("b");
            echo.encoding_type = Some("url");
            let result = ListResult {
                objects: vec![object("a b+c.txt")],
                key_count: 1,
                ..Default::default()
            };

            let body = list_bucket_result(&echo, &result);

            // Without this a client cannot tell a key holding `+` from one holding a
            // space, because it percent-decodes whatever it is given.
            assert!(body.contains("<Key>a%20b%2Bc.txt</Key>"));
            assert!(body.contains("<EncodingType>url</EncodingType>"));
        }

        #[test]
        fn should_percent_encode_the_delimiter_and_common_prefixes_too() {
            let mut echo = echo("b");
            echo.encoding_type = Some("url");
            echo.delimiter = Some("/");
            let result = ListResult {
                common_prefixes: vec!["a b/".into()],
                key_count: 1,
                ..Default::default()
            };

            let body = list_bucket_result(&echo, &result);

            assert!(body.contains("<Delimiter>%2F</Delimiter>"));
            assert!(body.contains("<Prefix>a%20b%2F</Prefix>"));
        }

        #[test]
        fn should_hand_out_an_opaque_next_continuation_token() {
            let result = ListResult {
                is_truncated: true,
                next_continuation_token: Some("a+b".into()),
                key_count: 1,
                ..Default::default()
            };

            let body = list_bucket_result(&echo("b"), &result);

            let encoded = crate::token::encode("a+b");
            assert!(body.contains(&format!(
                "<NextContinuationToken>{encoded}</NextContinuationToken>"
            )));
            assert!(!body.contains("<NextContinuationToken>a+b<"));
        }

        fn object(key: &str) -> ObjectMeta {
            ObjectMeta {
                key: key.to_string(),
                size: 5,
                etag: "abc".into(),
                last_modified: at(0),
                content_type: "text/plain".into(),
                user_metadata: Default::default(),
                system_headers: Default::default(),
            }
        }

        #[test]
        fn should_quote_the_etag_the_way_s3_does() {
            let result = ListResult {
                objects: vec![object("a.txt")],
                key_count: 1,
                ..Default::default()
            };

            let body = list_bucket_result(&echo("b"), &result);

            assert!(body.contains("<ETag>&quot;abc&quot;</ETag>"));
        }

        #[test]
        fn should_include_key_count_for_a_v2_listing() {
            let result = ListResult {
                key_count: 0,
                ..Default::default()
            };

            let body = list_bucket_result(&echo("b"), &result);

            assert!(body.contains("<KeyCount>0</KeyCount>"));
            assert!(!body.contains("<Marker>"));
        }

        #[test]
        fn should_use_marker_instead_of_key_count_for_a_v1_listing() {
            let mut echo = echo("b");
            echo.v2 = false;
            let result = ListResult::default();

            let body = list_bucket_result(&echo, &result);

            assert!(body.contains("<Marker></Marker>"));
            assert!(!body.contains("<KeyCount>"));
        }

        #[test]
        fn should_render_common_prefixes_as_their_own_elements() {
            let result = ListResult {
                common_prefixes: vec!["a/".into(), "b/".into()],
                key_count: 2,
                ..Default::default()
            };

            let body = list_bucket_result(&echo("b"), &result);

            assert_eq!(body.matches("<CommonPrefixes>").count(), 2);
        }
    }

    mod location_constraint {
        use super::*;

        #[test]
        fn should_be_empty_for_us_east_1() {
            assert!(location_constraint("us-east-1").contains("><"));
        }

        #[test]
        fn should_name_any_other_region() {
            assert!(location_constraint("eu-west-2").contains(">eu-west-2<"));
        }
    }
}
