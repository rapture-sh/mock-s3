//! Reading the XML bodies S3 accepts.
//!
//! Only two operations take one: `CompleteMultipartUpload` and `DeleteObjects`. Both are
//! shallow lists, so this is a small state machine over element events rather than a
//! deserialisation layer. Element names are matched on their local part, so a body that
//! declares the S3 namespace with a prefix parses the same as one that does not.
//!
//! Responses are still written by hand in [`crate::xml`] — this module only reads.

use http::StatusCode;
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

/// A parsed `DeleteObjects` body.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeleteRequest {
    /// Quiet mode reports only failures, which is what `aws s3 rm --recursive` asks for.
    pub quiet: bool,
    pub keys: Vec<String>,
}

fn malformed(detail: impl std::fmt::Display) -> S3Error {
    S3Error::new(
        StatusCode::BAD_REQUEST,
        "MalformedXML",
        format!("The XML you provided was not well-formed or did not validate: {detail}"),
    )
}

/// Walk an XML body, handing each (element, text) pair to `visit`.
///
/// Both bodies here are flat enough that knowing the innermost open element and when it
/// closes is all the structure needed.
pub(crate) fn walk(body: &[u8], mut visit: impl FnMut(&str, Option<&str>)) -> Result<(), S3Error> {
    let text = std::str::from_utf8(body).map_err(|_| malformed("the body was not UTF-8"))?;

    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut open = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                open = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                visit(&open, None);
            }
            Ok(Event::Text(contents)) => {
                let value = contents.unescape().map_err(malformed)?.into_owned();
                if !open.is_empty() {
                    visit(&open, Some(&value));
                }
            }
            Ok(Event::End(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                visit(&format!("/{name}"), None);
                open.clear();
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

    walk(body, |element, text| match (element, text) {
        ("Quiet", Some(value)) => request.quiet = value.trim().eq_ignore_ascii_case("true"),
        ("Key", Some(value)) => request.keys.push(value.to_string()),
        _ => {}
    })?;

    if request.keys.is_empty() {
        return Err(malformed("no objects were listed"));
    }
    Ok(request)
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

        #[test]
        fn should_read_every_key() {
            let body = br#"<Delete>
                <Object><Key>a.txt</Key></Object>
                <Object><Key>photos/b.jpg</Key></Object>
            </Delete>"#;

            let request = delete_objects(body).unwrap();

            assert_eq!(request.keys, vec!["a.txt", "photos/b.jpg"]);
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

            assert_eq!(delete_objects(body).unwrap().keys, vec!["notes&misc.txt"]);
        }

        #[test]
        fn should_reject_a_body_with_no_objects() {
            assert_eq!(
                delete_objects(b"<Delete></Delete>").unwrap_err().code,
                "MalformedXML"
            );
        }
    }
}
