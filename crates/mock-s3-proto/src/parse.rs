//! Reading the XML bodies S3 accepts.
//!
//! Only two operations take one: `CompleteMultipartUpload` and `DeleteObjects`. Both are
//! shallow lists, so this is a small state machine over element events rather than a
//! deserialisation layer. Element names are matched on their local part, so a body that
//! declares the S3 namespace with a prefix parses the same as one that does not.
//!
//! Responses are still written by hand in [`crate::xml`], this module only reads.

use http::StatusCode;
use mock_s3_core::VersioningStatus;
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::S3Error;

/// One `<Part>` from a `CompleteMultipartUpload` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedPart {
    pub number: u32,
    /// Unquoted: clients send it with the quotes the UploadPart response carried.
    pub etag: String,
}

/// One `<Object>` from a `DeleteObjects` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteTarget {
    pub key: String,
    /// Naming a version deletes that version outright. Without one, a versioned bucket
    /// gets a delete marker instead, so the difference matters.
    pub version_id: Option<String>,
}

/// A parsed `DeleteObjects` body.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeleteRequest {
    /// Quiet mode reports only failures, which is what `aws s3 rm --recursive` asks for.
    pub quiet: bool,
    pub objects: Vec<DeleteTarget>,
}

fn malformed(detail: impl std::fmt::Display) -> S3Error {
    S3Error::new(
        StatusCode::BAD_REQUEST,
        "MalformedXML",
        format!("The XML you provided was not well-formed or did not validate: {detail}"),
    )
}

/// Walk an XML body, handing each element's name and text to `visit`.
///
/// `visit` is called with `(name, None)` when an element opens, `(name, Some(text))`
/// when it closes, and `("/name", None)` after that, enough structure for the flat
/// bodies S3 accepts.
///
/// **Text is never trimmed.** An object key may legitimately begin or end with a space:
/// S3 accepts `" "` and `"_ "` as keys, and trimming them turns a delete of one object
/// into a delete of a different one. Callers that want a number or a flag trim their own
/// value; callers handling keys must not.
///
/// Text is accumulated rather than reported as it arrives, because a value split across
/// events, by an entity reference or a CDATA section, would otherwise be delivered in
/// pieces and the last piece would win.
pub(crate) fn walk(body: &[u8], mut visit: impl FnMut(&str, Option<&str>)) -> Result<(), S3Error> {
    let text = std::str::from_utf8(body).map_err(|_| malformed("the body was not UTF-8"))?;

    let mut reader = Reader::from_str(text);

    let mut open: Vec<String> = Vec::new();
    let mut collected = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                visit(&name, None);
                open.push(name);
                collected.clear();
            }
            Ok(Event::Text(contents)) => {
                collected.push_str(&contents.unescape().map_err(malformed)?);
            }
            Ok(Event::CData(contents)) => {
                collected.push_str(&String::from_utf8_lossy(&contents));
            }
            Ok(Event::End(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                if open.last().map(String::as_str) == Some(name.as_str()) {
                    open.pop();
                    visit(&name, Some(&collected));
                }
                visit(&format!("/{name}"), None);
                collected.clear();
            }
            Ok(Event::Eof) => return Ok(()),
            Err(error) => return Err(malformed(error)),
            _ => {}
        }
    }
}

pub fn complete_multipart_upload(body: &[u8]) -> Result<Vec<CompletedPart>, S3Error> {
    let mut parts = Vec::new();
    let mut number: Option<u32> = None;
    let mut etag: Option<String> = None;

    walk(body, |element, text| match (element, text) {
        ("Part", None) => {
            number = None;
            etag = None;
        }
        ("PartNumber", Some(value)) => number = value.trim().parse().ok(),
        ("ETag", Some(value)) => etag = Some(value.trim().trim_matches('"').to_string()),
        ("/Part", None) => {
            if let Some(found) = number.take() {
                parts.push(CompletedPart {
                    number: found,
                    etag: etag.take().unwrap_or_default(),
                });
            }
        }
        _ => {}
    })?;

    if parts.is_empty() {
        return Err(malformed("no parts were listed"));
    }
    Ok(parts)
}

pub fn delete_objects(body: &[u8]) -> Result<DeleteRequest, S3Error> {
    let mut request = DeleteRequest::default();
    let mut key: Option<String> = None;
    let mut version_id: Option<String> = None;

    walk(body, |element, text| match (element, text) {
        ("Quiet", Some(value)) => request.quiet = value.trim().eq_ignore_ascii_case("true"),
        ("Object", None) => {
            key = None;
            version_id = None;
        }
        ("Key", Some(value)) => key = Some(value.to_string()),
        ("VersionId", Some(value)) => {
            let trimmed = value.trim();
            version_id = (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        ("/Object", None) => {
            if let Some(found) = key.take() {
                request.objects.push(DeleteTarget {
                    key: found,
                    version_id: version_id.take(),
                });
            }
        }
        _ => {}
    })?;

    if request.objects.is_empty() {
        return Err(malformed("no objects were listed"));
    }
    Ok(request)
}

/// A `PutBucketVersioning` body.
///
/// `MfaDelete` is parsed over and ignored: mock-s3 has no authentication, so there is no
/// MFA to require. Real S3 rejects a document with no `Status` rather than defaulting it,
/// and so does this, silently picking one would hide a client bug.
pub fn versioning_configuration(body: &[u8]) -> Result<VersioningStatus, S3Error> {
    let mut status: Option<VersioningStatus> = None;

    walk(body, |element, text| {
        if let ("Status", Some(value)) = (element, text) {
            status = Some(VersioningStatus::parse(value));
        }
    })?;

    match status {
        Some(found) if found.is_configured() => Ok(found),
        _ => Err(S3Error::new(
            StatusCode::BAD_REQUEST,
            "IllegalVersioningConfigurationException",
            "The versioning configuration specified in the request is invalid: Status must \
             be Enabled or Suspended.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod complete_multipart_upload {
        use super::*;

        #[test]
        fn should_read_parts_in_the_order_given() {
            let body = br#"<CompleteMultipartUpload>
                <Part><PartNumber>1</PartNumber><ETag>"abc"</ETag></Part>
                <Part><PartNumber>2</PartNumber><ETag>"def"</ETag></Part>
            </CompleteMultipartUpload>"#;

            let parts = complete_multipart_upload(body).unwrap();

            assert_eq!(
                parts,
                vec![
                    CompletedPart {
                        number: 1,
                        etag: "abc".into()
                    },
                    CompletedPart {
                        number: 2,
                        etag: "def".into()
                    },
                ]
            );
        }

        #[test]
        fn should_strip_the_quotes_clients_send_around_an_etag() {
            let body = br#"<CompleteMultipartUpload><Part>
                <PartNumber>1</PartNumber><ETag>&quot;abc&quot;</ETag>
            </Part></CompleteMultipartUpload>"#;

            let parts = complete_multipart_upload(body).unwrap();

            assert_eq!(parts[0].etag, "abc");
        }

        #[test]
        fn should_parse_a_body_that_declares_the_s3_namespace() {
            let body =
                br#"<CompleteMultipartUpload xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
                <Part><PartNumber>1</PartNumber><ETag>"abc"</ETag></Part>
            </CompleteMultipartUpload>"#;

            assert_eq!(complete_multipart_upload(body).unwrap().len(), 1);
        }

        #[test]
        fn should_tolerate_the_elements_arriving_in_either_order() {
            let body = br#"<CompleteMultipartUpload><Part>
                <ETag>"abc"</ETag><PartNumber>3</PartNumber>
            </Part></CompleteMultipartUpload>"#;

            let parts = complete_multipart_upload(body).unwrap();

            assert_eq!(parts[0].number, 3);
            assert_eq!(parts[0].etag, "abc");
        }

        #[test]
        fn should_reject_a_body_with_no_parts() {
            let body = b"<CompleteMultipartUpload></CompleteMultipartUpload>";

            assert_eq!(
                complete_multipart_upload(body).unwrap_err().code,
                "MalformedXML"
            );
        }

        #[test]
        fn should_reject_a_body_that_is_not_xml() {
            assert_eq!(
                complete_multipart_upload(b"<<<not xml").unwrap_err().code,
                "MalformedXML"
            );
        }
    }

    mod delete_objects {
        use super::*;

        fn keys(request: &DeleteRequest) -> Vec<&str> {
            request
                .objects
                .iter()
                .map(|target| target.key.as_str())
                .collect()
        }

        #[test]
        fn should_read_every_key() {
            let body = br#"<Delete>
                <Object><Key>a.txt</Key></Object>
                <Object><Key>photos/b.jpg</Key></Object>
            </Delete>"#;

            let request = delete_objects(body).unwrap();

            assert_eq!(keys(&request), vec!["a.txt", "photos/b.jpg"]);
            assert!(!request.quiet);
        }

        #[test]
        fn should_read_quiet_mode() {
            let body = br#"<Delete><Quiet>true</Quiet>
                <Object><Key>a.txt</Key></Object></Delete>"#;

            assert!(delete_objects(body).unwrap().quiet);
        }

        #[test]
        fn should_unescape_a_key_containing_xml_syntax() {
            let body = br#"<Delete><Object><Key>notes&amp;misc.txt</Key></Object></Delete>"#;

            assert_eq!(keys(&delete_objects(body).unwrap()), vec!["notes&misc.txt"]);
        }

        #[test]
        fn should_reject_a_body_with_no_objects() {
            assert_eq!(
                delete_objects(b"<Delete></Delete>").unwrap_err().code,
                "MalformedXML"
            );
        }

        /// S3 accepts a key that is nothing but a space, and one that ends in a space.
        /// Trimming either turns a delete into a delete of something else.
        #[test]
        fn should_preserve_whitespace_in_a_key() {
            let body = b"<Delete><Object><Key> </Key></Object>\
                         <Object><Key>_ </Key></Object>\
                         <Object><Key> leading</Key></Object></Delete>";

            let request = delete_objects(body).unwrap();

            assert_eq!(keys(&request), vec![" ", "_ ", " leading"]);
        }

        #[test]
        fn should_ignore_the_whitespace_between_elements() {
            let body = br#"<Delete>
                <Object>
                    <Key>a.txt</Key>
                </Object>
            </Delete>"#;

            assert_eq!(keys(&delete_objects(body).unwrap()), vec!["a.txt"]);
        }

        #[test]
        fn should_read_a_key_split_across_an_entity_reference() {
            let body = b"<Delete><Object><Key>a&amp;b&amp;c</Key></Object></Delete>";

            assert_eq!(keys(&delete_objects(body).unwrap()), vec!["a&b&c"]);
        }

        /// A version id turns a batch delete of that entry from "leave a delete marker"
        /// into "remove this version", so it has to survive the parse.
        #[test]
        fn should_read_the_version_id_of_an_object_that_names_one() {
            let body = br#"<Delete>
                <Object><Key>a.txt</Key><VersionId>abc123</VersionId></Object>
                <Object><Key>b.txt</Key></Object>
            </Delete>"#;

            let request = delete_objects(body).unwrap();

            assert_eq!(request.objects[0].version_id.as_deref(), Some("abc123"));
            assert_eq!(request.objects[1].version_id, None);
        }

        /// One entry's version id must not carry over to the next.
        #[test]
        fn should_not_let_a_version_id_leak_into_the_following_object() {
            let body = br#"<Delete>
                <Object><Key>a.txt</Key><VersionId>abc123</VersionId></Object>
                <Object><Key>b.txt</Key></Object>
                <Object><Key>c.txt</Key><VersionId>def456</VersionId></Object>
            </Delete>"#;

            let request = delete_objects(body).unwrap();
            let found: Vec<Option<&str>> = request
                .objects
                .iter()
                .map(|target| target.version_id.as_deref())
                .collect();

            assert_eq!(found, vec![Some("abc123"), None, Some("def456")]);
        }

        #[test]
        fn should_treat_an_empty_version_id_as_none() {
            let body =
                br#"<Delete><Object><Key>a.txt</Key><VersionId></VersionId></Object></Delete>"#;

            assert_eq!(delete_objects(body).unwrap().objects[0].version_id, None);
        }
    }

    mod versioning_configuration {
        use super::*;

        #[test]
        fn should_read_each_status_a_client_can_set() {
            for (text, expected) in [
                ("Enabled", VersioningStatus::Enabled),
                ("Suspended", VersioningStatus::Suspended),
            ] {
                let body = format!(
                    "<VersioningConfiguration><Status>{text}</Status></VersioningConfiguration>"
                );

                assert_eq!(versioning_configuration(body.as_bytes()).unwrap(), expected);
            }
        }

        #[test]
        fn should_parse_over_mfa_delete() {
            let body = br#"<VersioningConfiguration>
                <MfaDelete>Disabled</MfaDelete><Status>Enabled</Status>
            </VersioningConfiguration>"#;

            assert_eq!(
                versioning_configuration(body).unwrap(),
                VersioningStatus::Enabled
            );
        }

        /// Defaulting a missing status would turn a client bug into silent data loss on
        /// the next overwrite, so S3 rejects it and so does this.
        #[test]
        fn should_reject_a_document_with_no_status() {
            let error =
                versioning_configuration(b"<VersioningConfiguration></VersioningConfiguration>")
                    .unwrap_err();

            assert_eq!(error.code, "IllegalVersioningConfigurationException");
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
        }

        #[test]
        fn should_reject_a_status_that_is_not_one_s3_defines() {
            assert_eq!(
                versioning_configuration(
                    b"<VersioningConfiguration><Status>On</Status></VersioningConfiguration>"
                )
                .unwrap_err()
                .code,
                "IllegalVersioningConfigurationException"
            );
        }
    }
}
