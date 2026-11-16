//! The S3 wire format.
//!
//! This is the only module allowed to produce XML. Handlers call these functions; they
//! never format a response body themselves, so the exact shape S3 uses lives in one
//! place and can be tested on its own.

use std::fmt::Write as _;

use chrono::{DateTime, SecondsFormat, Utc};
use mock_s3_core::multipart::{PartInfo, UploadInfo};
use mock_s3_core::{BucketInfo, ListResult, VersionListResult, VersioningStatus};

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
    /// `fetch-owner=true`. A version 2 listing leaves `Owner` out unless it is asked
    /// for, that is the point of the parameter, while version 1 always includes it.
    pub fetch_owner: bool,
}

/// How S3 encodes a key for `encoding-type=url`: everything outside the unreserved set,
/// **except the path separator**.
///
/// `/` is left alone. It is structural in a key, the thing `delimiter` groups on, and
/// S3 preserves it, so `photos/a b.jpg` comes back as `photos/a%20b.jpg`. Encoding it as
/// `%2F` breaks clients in a way that is easy to miss: botocore decodes the nested `Key`
/// elements, so keys look right, but it does not decode the top-level `Prefix`, which
/// then comes back as `foo%2F` where the caller sent `foo/`.
const URL_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

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
        if !echo.v2 || echo.fetch_owner {
            let _ = write!(
                out,
                "<Owner><ID>{OWNER_ID}</ID><DisplayName>{OWNER_DISPLAY_NAME}</DisplayName></Owner>"
            );
        }
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

/// What a `ListObjectVersions` request asked for, echoed back in its answer.
///
/// Separate from [`ListingEcho`] because a versions page resumes from two markers rather
/// than one token: a key, and a version of that key.
pub struct VersionListingEcho<'request> {
    pub bucket: &'request str,
    pub prefix: &'request str,
    pub delimiter: Option<&'request str>,
    pub max_keys: usize,
    pub key_marker: Option<&'request str>,
    pub version_id_marker: Option<&'request str>,
    pub encoding_type: Option<&'request str>,
}

/// `ListObjectVersions`.
///
/// Versions and delete markers are different elements but one sequence: they are written
/// in listing order, interleaved, because that order is what a client paginates through.
///
/// A bucket that was never versioned still answers here, with one `null` version per
/// object. That is what real S3 does, and it is not cosmetic: clients use `?versions` to
/// enumerate a bucket before deleting it, the ceph/s3-tests cleanup does, and an answer
/// in the wrong document shape parses as *no objects*, so the caller deletes nothing and
/// then cannot delete the bucket.
pub fn list_versions_result(echo: &VersionListingEcho<'_>, result: &VersionListResult) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{DECLARATION}<ListVersionsResult xmlns=\"{NAMESPACE}\">"
    );
    let _ = write!(out, "<Name>{}</Name>", escape(echo.bucket));
    let _ = write!(
        out,
        "<Prefix>{}</Prefix>",
        listing_value(echo.prefix, echo.encoding_type)
    );
    let _ = write!(
        out,
        "<KeyMarker>{}</KeyMarker>",
        listing_value(echo.key_marker.unwrap_or_default(), echo.encoding_type)
    );
    let _ = write!(
        out,
        "<VersionIdMarker>{}</VersionIdMarker>",
        escape(echo.version_id_marker.unwrap_or_default())
    );
    if let Some(next) = &result.next_key_marker {
        let _ = write!(
            out,
            "<NextKeyMarker>{}</NextKeyMarker>",
            listing_value(next, echo.encoding_type)
        );
        // A client feeds both markers back into the next request, and some validate
        // their own parameters before sending, so a page that offers one without the
        // other cannot be followed. A page that ended on a common prefix has no version
        // to resume from, which S3 reports as `null`.
        let _ = write!(
            out,
            "<NextVersionIdMarker>{}</NextVersionIdMarker>",
            escape(
                result
                    .next_version_id_marker
                    .as_deref()
                    .unwrap_or(mock_s3_core::versioning::NULL_VERSION_ID)
            )
        );
    }
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

    for version in &result.versions {
        let element = if version.is_delete_marker() {
            "DeleteMarker"
        } else {
            "Version"
        };

        let _ = write!(out, "<{element}>");
        let _ = write!(
            out,
            "<Key>{}</Key>",
            listing_value(&version.key, echo.encoding_type)
        );
        let _ = write!(
            out,
            "<VersionId>{}</VersionId>",
            escape(&version.version_id)
        );
        let _ = write!(
            out,
            "<IsLatest>{}</IsLatest>",
            if version.is_latest { "true" } else { "false" }
        );
        let _ = write!(
            out,
            "<LastModified>{}</LastModified>",
            xml_timestamp(&version.last_modified)
        );
        // A delete marker has no payload, so it carries neither an ETag nor a size.
        if let Some(meta) = &version.meta {
            let _ = write!(out, "<ETag>&quot;{}&quot;</ETag>", escape(&meta.etag));
            let _ = write!(out, "<Size>{}</Size>", meta.size);
            out.push_str("<StorageClass>STANDARD</StorageClass>");
        }
        let _ = write!(
            out,
            "<Owner><ID>{OWNER_ID}</ID><DisplayName>{OWNER_DISPLAY_NAME}</DisplayName></Owner>"
        );
        let _ = write!(out, "</{element}>");
    }

    for prefix in &result.common_prefixes {
        let _ = write!(
            out,
            "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
            listing_value(prefix, echo.encoding_type)
        );
    }

    out.push_str("</ListVersionsResult>");
    out
}

pub fn copy_object_result(etag: &str, last_modified: &DateTime<Utc>) -> String {
    format!(
        "{DECLARATION}<CopyObjectResult xmlns=\"{NAMESPACE}\">\
         <LastModified>{}</LastModified><ETag>&quot;{}&quot;</ETag>\
         </CopyObjectResult>",
        xml_timestamp(last_modified),
        escape(etag)
    )
}

pub fn initiate_multipart_upload(bucket: &str, key: &str, upload_id: &str) -> String {
    format!(
        "{DECLARATION}<InitiateMultipartUploadResult xmlns=\"{NAMESPACE}\">\
         <Bucket>{}</Bucket><Key>{}</Key><UploadId>{}</UploadId>\
         </InitiateMultipartUploadResult>",
        escape(bucket),
        escape(key),
        escape(upload_id)
    )
}

pub fn complete_multipart_upload(bucket: &str, key: &str, etag: &str) -> String {
    format!(
        "{DECLARATION}<CompleteMultipartUploadResult xmlns=\"{NAMESPACE}\">\
         <Location>/{}/{}</Location><Bucket>{}</Bucket><Key>{}</Key>\
         <ETag>&quot;{}&quot;</ETag></CompleteMultipartUploadResult>",
        escape(bucket),
        escape(key),
        escape(bucket),
        escape(key),
        escape(etag)
    )
}

pub fn list_parts_result(bucket: &str, key: &str, upload_id: &str, parts: &[PartInfo]) -> String {
    let mut out = String::new();
    let _ = write!(out, "{DECLARATION}<ListPartsResult xmlns=\"{NAMESPACE}\">");
    let _ = write!(out, "<Bucket>{}</Bucket>", escape(bucket));
    let _ = write!(out, "<Key>{}</Key>", escape(key));
    let _ = write!(out, "<UploadId>{}</UploadId>", escape(upload_id));
    let _ = write!(out, "<PartNumberMarker>0</PartNumberMarker>");
    let _ = write!(out, "<MaxParts>1000</MaxParts>");
    out.push_str("<IsTruncated>false</IsTruncated>");
    out.push_str("<StorageClass>STANDARD</StorageClass>");

    for part in parts {
        let _ = write!(
            out,
            "<Part><PartNumber>{}</PartNumber><LastModified>{}</LastModified>\
             <ETag>&quot;{}&quot;</ETag><Size>{}</Size></Part>",
            part.number,
            xml_timestamp(&part.last_modified),
            escape(&part.etag),
            part.size
        );
    }

    out.push_str("</ListPartsResult>");
    out
}

pub fn list_multipart_uploads_result(bucket: &str, uploads: &[UploadInfo]) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{DECLARATION}<ListMultipartUploadsResult xmlns=\"{NAMESPACE}\">"
    );
    let _ = write!(out, "<Bucket>{}</Bucket>", escape(bucket));
    out.push_str("<KeyMarker></KeyMarker><UploadIdMarker></UploadIdMarker>");
    out.push_str("<MaxUploads>1000</MaxUploads><IsTruncated>false</IsTruncated>");

    for upload in uploads {
        let _ = write!(
            out,
            "<Upload><Key>{}</Key><UploadId>{}</UploadId><Initiated>{}</Initiated>\
             <StorageClass>STANDARD</StorageClass></Upload>",
            escape(&upload.key),
            escape(&upload.upload_id),
            xml_timestamp(&upload.initiated)
        );
    }

    out.push_str("</ListMultipartUploadsResult>");
    out
}

/// A `DeleteObjects` response. In quiet mode only failures are reported, which is what
/// `aws s3 rm --recursive` asks for.
/// One `<Deleted>` entry of a `DeleteObjects` answer.
pub struct Deleted {
    pub key: String,
    /// The version the request named, echoed back. Absent when it named none.
    pub version_id: Option<String>,
    pub delete_marker: bool,
    /// The version id of the delete marker created, or of the marker removed.
    pub delete_marker_version_id: Option<String>,
}

pub fn delete_result(
    deleted: &[Deleted],
    errors: &[(String, &'static str, String)],
    quiet: bool,
) -> String {
    let mut out = String::new();
    let _ = write!(out, "{DECLARATION}<DeleteResult xmlns=\"{NAMESPACE}\">");

    if !quiet {
        for entry in deleted {
            let _ = write!(out, "<Deleted><Key>{}</Key>", escape(&entry.key));
            if let Some(version_id) = &entry.version_id {
                let _ = write!(out, "<VersionId>{}</VersionId>", escape(version_id));
            }
            // Only ever reported when true, which is what S3 does: a batch delete in an
            // unversioned bucket answers with the key alone.
            if entry.delete_marker {
                out.push_str("<DeleteMarker>true</DeleteMarker>");
                if let Some(version_id) = &entry.delete_marker_version_id {
                    let _ = write!(
                        out,
                        "<DeleteMarkerVersionId>{}</DeleteMarkerVersionId>",
                        escape(version_id)
                    );
                }
            }
            out.push_str("</Deleted>");
        }
    }
    for (key, code, message) in errors {
        let _ = write!(
            out,
            "<Error><Key>{}</Key><Code>{}</Code><Message>{}</Message></Error>",
            escape(key),
            escape(code),
            escape(message)
        );
    }

    out.push_str("</DeleteResult>");
    out
}

/// An empty configuration document, which is what S3 returns for a bucket setting that
/// was never configured but which has no "not found" error of its own.
pub fn empty_configuration(root: &str) -> String {
    format!("{DECLARATION}<{root} xmlns=\"{NAMESPACE}\"></{root}>")
}

/// The owner has full control, which is the only answer a server with no ACL model can
/// honestly give. Tools read this to decide whether they may write.
pub fn access_control_policy() -> String {
    format!(
        "{DECLARATION}<AccessControlPolicy xmlns=\"{NAMESPACE}\">\
         <Owner><ID>{OWNER_ID}</ID><DisplayName>{OWNER_DISPLAY_NAME}</DisplayName></Owner>\
         <AccessControlList><Grant>\
         <Grantee xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:type=\"CanonicalUser\">\
         <ID>{OWNER_ID}</ID><DisplayName>{OWNER_DISPLAY_NAME}</DisplayName></Grantee>\
         <Permission>FULL_CONTROL</Permission>\
         </Grant></AccessControlList></AccessControlPolicy>"
    )
}

pub fn cors_configuration(rules: &[crate::cors::CorsRule]) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{DECLARATION}<CORSConfiguration xmlns=\"{NAMESPACE}\">"
    );

    for rule in rules {
        out.push_str("<CORSRule>");
        for origin in &rule.allowed_origins {
            let _ = write!(out, "<AllowedOrigin>{}</AllowedOrigin>", escape(origin));
        }
        for method in &rule.allowed_methods {
            let _ = write!(out, "<AllowedMethod>{}</AllowedMethod>", escape(method));
        }
        for header in &rule.allowed_headers {
            let _ = write!(out, "<AllowedHeader>{}</AllowedHeader>", escape(header));
        }
        for header in &rule.expose_headers {
            let _ = write!(out, "<ExposeHeader>{}</ExposeHeader>", escape(header));
        }
        if let Some(seconds) = rule.max_age_seconds {
            let _ = write!(out, "<MaxAgeSeconds>{seconds}</MaxAgeSeconds>");
        }
        out.push_str("</CORSRule>");
    }

    out.push_str("</CORSConfiguration>");
    out
}

/// An empty tag set. The `TagSet` element has to be there even when it holds nothing,
/// or an SDK reports a malformed response rather than "no tags".
pub fn object_tagging() -> String {
    format!("{DECLARATION}<Tagging xmlns=\"{NAMESPACE}\"><TagSet></TagSet></Tagging>")
}

pub fn request_payment_configuration() -> String {
    format!(
        "{DECLARATION}<RequestPaymentConfiguration xmlns=\"{NAMESPACE}\">\
         <Payer>BucketOwner</Payer></RequestPaymentConfiguration>"
    )
}

/// `GetBucketVersioning`.
///
/// A bucket that has never been configured answers with an empty document rather than
/// `Status: Suspended`. The two are not the same thing to S3, a suspended bucket still
/// holds the versions it collected, and an SDK reports the empty one as `None`.
pub fn versioning_configuration(status: VersioningStatus) -> String {
    let mut out = format!("{DECLARATION}<VersioningConfiguration xmlns=\"{NAMESPACE}\">");
    if status.is_configured() {
        let _ = write!(out, "<Status>{}</Status>", status.as_str());
    }
    out.push_str("</VersioningConfiguration>");
    out
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
                fetch_owner: false,
            }
        }

        /// A version 2 listing leaves `Owner` out unless `fetch-owner=true`: that is
        /// what the parameter is for, while a version 1 listing always carries it.
        #[test]
        fn should_report_the_owner_only_when_the_listing_asked_for_it() {
            let result = ListResult {
                objects: vec![object("notes.txt")],
                ..Default::default()
            };

            let mut v2 = echo("my-bucket");
            assert!(!list_bucket_result(&v2, &result).contains("<Owner>"));

            v2.fetch_owner = true;
            assert!(list_bucket_result(&v2, &result).contains("<Owner>"));

            let mut v1 = echo("my-bucket");
            v1.v2 = false;
            assert!(list_bucket_result(&v1, &result).contains("<Owner>"));
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

            // The separator survives encoding; the space does not.
            assert!(body.contains("<Delimiter>/</Delimiter>"));
            assert!(body.contains("<Prefix>a%20b/</Prefix>"));
        }

        /// The echoed `Prefix` has to come back as the caller sent it. botocore does not
        /// decode this element, so encoding the separator here is visible to every
        /// client that lists with a prefix.
        #[test]
        fn should_echo_a_prefix_unchanged_when_it_is_only_a_path() {
            let mut echo = echo("b");
            echo.encoding_type = Some("url");
            echo.prefix = "foo/";

            let body = list_bucket_result(&echo, &ListResult::default());

            assert!(body.contains("<Prefix>foo/</Prefix>"), "{body}");
        }

        #[test]
        fn should_still_encode_characters_that_are_ambiguous_in_a_url() {
            let mut echo = echo("b");
            echo.encoding_type = Some("url");
            let result = ListResult {
                objects: vec![object("photos/a+b c%d.jpg")],
                key_count: 1,
                ..Default::default()
            };

            let body = list_bucket_result(&echo, &result);

            // `+` would otherwise be read as a space, and a bare `%` is an invalid
            // escape, those are the reasons encoding-type exists.
            assert!(
                body.contains("<Key>photos/a%2Bb%20c%25d.jpg</Key>"),
                "{body}"
            );
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
                version_id: None,
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

    mod versioning_configuration {
        use super::*;

        /// An SDK reads the empty document as `None`, which is not `Suspended`: a
        /// suspended bucket still holds the versions it collected.
        #[test]
        fn should_omit_the_status_for_a_bucket_that_was_never_configured() {
            let body = versioning_configuration(VersioningStatus::Unversioned);

            assert!(!body.contains("<Status>"), "{body}");
        }

        #[test]
        fn should_name_each_status_a_bucket_can_be_set_to() {
            assert!(
                versioning_configuration(VersioningStatus::Enabled)
                    .contains("<Status>Enabled</Status>")
            );
            assert!(
                versioning_configuration(VersioningStatus::Suspended)
                    .contains("<Status>Suspended</Status>")
            );
        }
    }

    mod list_versions_result {
        use super::*;
        use mock_s3_core::ObjectVersion;

        fn echo<'a>(bucket: &'a str) -> VersionListingEcho<'a> {
            VersionListingEcho {
                bucket,
                prefix: "",
                delimiter: None,
                max_keys: 1000,
                key_marker: None,
                version_id_marker: None,
                encoding_type: None,
            }
        }

        fn version(key: &str, version_id: &str, is_latest: bool) -> ObjectVersion {
            ObjectVersion {
                key: key.to_string(),
                version_id: version_id.to_string(),
                is_latest,
                last_modified: at(0),
                meta: Some(ObjectMeta {
                    key: key.to_string(),
                    size: 5,
                    etag: "abc".into(),
                    last_modified: at(0),
                    content_type: "text/plain".into(),
                    user_metadata: Default::default(),
                    version_id: Some(version_id.to_string()),
                    system_headers: Default::default(),
                }),
            }
        }

        fn marker(key: &str, version_id: &str, is_latest: bool) -> ObjectVersion {
            ObjectVersion {
                key: key.to_string(),
                version_id: version_id.to_string(),
                is_latest,
                last_modified: at(0),
                meta: None,
            }
        }

        #[test]
        fn should_write_a_version_with_the_elements_s3_sends() {
            let result = VersionListResult {
                versions: vec![version("notes.txt", "v1", true)],
                ..Default::default()
            };

            let body = list_versions_result(&echo("my-bucket"), &result);

            assert!(body.contains("<Key>notes.txt</Key>"), "{body}");
            assert!(body.contains("<VersionId>v1</VersionId>"));
            assert!(body.contains("<IsLatest>true</IsLatest>"));
            assert!(body.contains("<ETag>&quot;abc&quot;</ETag>"));
            assert!(body.contains("<Size>5</Size>"));
            assert!(body.contains("<StorageClass>STANDARD</StorageClass>"));
        }

        /// A delete marker has no payload, so S3 sends neither an ETag nor a size for
        /// one. An SDK that found a zero size would report the object as empty rather
        /// than deleted.
        #[test]
        fn should_write_a_delete_marker_without_an_etag_or_a_size() {
            let result = VersionListResult {
                versions: vec![marker("notes.txt", "v2", true)],
                ..Default::default()
            };

            let body = list_versions_result(&echo("my-bucket"), &result);

            assert!(body.contains("<DeleteMarker>"), "{body}");
            assert!(!body.contains("<ETag>"));
            assert!(!body.contains("<Size>"));
        }

        /// Versions and markers paginate as one sequence, so they have to be written in
        /// listing order rather than grouped by kind.
        #[test]
        fn should_interleave_markers_and_versions_in_listing_order() {
            let result = VersionListResult {
                versions: vec![
                    marker("notes.txt", "v3", true),
                    version("notes.txt", "v2", false),
                    marker("notes.txt", "v1", false),
                ],
                ..Default::default()
            };

            let body = list_versions_result(&echo("my-bucket"), &result);

            let mut order: Vec<(usize, &str)> = body
                .match_indices("<DeleteMarker>")
                .map(|(at, _)| (at, "marker"))
                .chain(
                    body.match_indices("<Version>")
                        .map(|(at, _)| (at, "version")),
                )
                .collect();
            order.sort();

            let kinds: Vec<&str> = order.into_iter().map(|(_, kind)| kind).collect();
            assert_eq!(kinds, vec!["marker", "version", "marker"], "{body}");
        }

        #[test]
        fn should_echo_both_markers_a_request_resumed_from() {
            let result = VersionListResult::default();
            let mut sent = echo("my-bucket");
            sent.key_marker = Some("notes.txt");
            sent.version_id_marker = Some("v1");

            let body = list_versions_result(&sent, &result);

            assert!(body.contains("<KeyMarker>notes.txt</KeyMarker>"), "{body}");
            assert!(body.contains("<VersionIdMarker>v1</VersionIdMarker>"));
        }

        /// A page that ends on a common prefix has no version to resume from, and S3
        /// reports `null` there. Both markers or neither: a client feeds both back, and
        /// some validate their own parameters before sending.
        #[test]
        fn should_report_a_null_version_marker_when_a_page_ends_on_a_prefix() {
            let result = VersionListResult {
                common_prefixes: vec!["photos/".into()],
                is_truncated: true,
                next_key_marker: Some("photos/".into()),
                next_version_id_marker: None,
                ..Default::default()
            };

            let body = list_versions_result(&echo("my-bucket"), &result);

            assert!(
                body.contains("<NextKeyMarker>photos/</NextKeyMarker>"),
                "{body}"
            );
            assert!(body.contains("<NextVersionIdMarker>null</NextVersionIdMarker>"));
        }

        #[test]
        fn should_omit_both_next_markers_when_the_page_is_the_last_one() {
            let result = VersionListResult {
                versions: vec![version("notes.txt", "v1", true)],
                ..Default::default()
            };

            let body = list_versions_result(&echo("my-bucket"), &result);

            assert!(!body.contains("<NextKeyMarker>"), "{body}");
            assert!(!body.contains("<NextVersionIdMarker>"));
        }

        #[test]
        fn should_url_encode_a_key_when_the_request_asked_for_it() {
            let result = VersionListResult {
                versions: vec![version("photos/a b.jpg", "v1", true)],
                ..Default::default()
            };
            let mut sent = echo("my-bucket");
            sent.encoding_type = Some("url");

            let body = list_versions_result(&sent, &result);

            assert!(body.contains("<EncodingType>url</EncodingType>"), "{body}");
            // S3 leaves the path separator alone even when it encodes everything else.
            assert!(body.contains("<Key>photos/a%20b.jpg</Key>"));
        }

        #[test]
        fn should_escape_a_key_containing_xml_syntax() {
            let result = VersionListResult {
                versions: vec![version("a&b.txt", "v1", true)],
                ..Default::default()
            };

            let body = list_versions_result(&echo("my-bucket"), &result);

            assert!(body.contains("<Key>a&amp;b.txt</Key>"), "{body}");
        }
    }

    mod delete_result {
        use super::*;

        fn deleted(key: &str) -> Deleted {
            Deleted {
                key: key.to_string(),
                version_id: None,
                delete_marker: false,
                delete_marker_version_id: None,
            }
        }

        /// A batch delete in an unversioned bucket reports the key alone. An SDK that
        /// found `DeleteMarker` there would report every delete as a marker.
        #[test]
        fn should_report_a_key_alone_when_nothing_was_versioned() {
            let body = delete_result(&[deleted("notes.txt")], &[], false);

            assert!(
                body.contains("<Deleted><Key>notes.txt</Key></Deleted>"),
                "{body}"
            );
        }

        #[test]
        fn should_report_the_marker_it_created_for_a_key_with_no_version_named() {
            let entry = Deleted {
                delete_marker: true,
                delete_marker_version_id: Some("v9".into()),
                ..deleted("notes.txt")
            };

            let body = delete_result(&[entry], &[], false);

            assert!(body.contains("<DeleteMarker>true</DeleteMarker>"), "{body}");
            assert!(body.contains("<DeleteMarkerVersionId>v9</DeleteMarkerVersionId>"));
            assert!(!body.contains("<VersionId>"));
        }

        #[test]
        fn should_echo_the_version_a_request_named() {
            let entry = Deleted {
                version_id: Some("v1".into()),
                ..deleted("notes.txt")
            };

            let body = delete_result(&[entry], &[], false);

            assert!(body.contains("<VersionId>v1</VersionId>"), "{body}");
            assert!(!body.contains("<DeleteMarker>"));
        }

        #[test]
        fn should_report_nothing_deleted_in_quiet_mode() {
            let body = delete_result(&[deleted("notes.txt")], &[], true);

            assert!(!body.contains("<Deleted>"), "{body}");
        }
    }
}
