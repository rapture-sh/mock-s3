//! The operations that already worked under curl, now locked in through the real SDK.

use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;

mod buckets {
    use super::*;

    #[tokio::test]
    async fn should_create_a_bucket_and_then_list_it() {
        let h = conformance::harness().await;

        h.s3.create_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();
        let listed = h.s3.list_buckets().send().await.unwrap();

        let names: Vec<&str> = listed
            .buckets()
            .iter()
            .filter_map(|bucket| bucket.name())
            .collect();
        assert_eq!(names, vec!["my-bucket"]);
    }

    /// Re-creating a bucket you already own succeeds in `us-east-1` and fails with
    /// `BucketAlreadyOwnedByYou` everywhere else. That is a documented quirk of the
    /// original region rather than an inconsistency, and `us-east-1` is the region
    /// mock-s3 reports, so setup code that creates a bucket unconditionally has to work.
    #[tokio::test]
    async fn should_succeed_on_a_repeat_create_in_us_east_1() {
        let h = conformance::harness().await;
        assert_eq!(h.server.region(), "us-east-1");

        h.s3.create_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();

        let again = h.s3.create_bucket().bucket("my-bucket").send().await;

        assert!(
            again.is_ok(),
            "us-east-1 answers a repeat create with 200: {again:?}"
        );
    }

    /// A subresource mock-s3 does not implement must be refused, not quietly treated as
    /// the operation the bare path would mean — a bucket listing, or worse.
    #[tokio::test]
    async fn should_refuse_a_bucket_subresource_it_does_not_implement() {
        let h = conformance::harness().await;
        h.s3.create_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();

        let status =
            h.s3.get_bucket_policy_status()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap_err()
                .into_service_error();
        assert_eq!(status.meta().code(), Some("NotImplemented"));

        let inventory =
            h.s3.list_bucket_inventory_configurations()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap_err()
                .into_service_error();
        assert_eq!(inventory.meta().code(), Some("NotImplemented"));

        // Still there: neither request was treated as an operation on the bucket itself.
        h.s3.head_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .expect("the bucket must survive a request for a subresource");
    }

    #[tokio::test]
    async fn should_reject_an_invalid_bucket_name() {
        let h = conformance::harness().await;

        let error =
            h.s3.create_bucket()
                .bucket("Not-Valid")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("InvalidBucketName"));
    }

    #[tokio::test]
    async fn should_refuse_to_delete_a_bucket_that_still_holds_objects() {
        let h = conformance::harness().await;
        h.s3.create_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();
        h.s3.put_object()
            .bucket("my-bucket")
            .key("a")
            .body(ByteStream::from_static(b"x"))
            .send()
            .await
            .unwrap();

        let error =
            h.s3.delete_bucket()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("BucketNotEmpty"));
    }

    #[tokio::test]
    async fn should_report_no_such_bucket_for_a_bucket_that_was_never_created() {
        let h = conformance::harness().await;

        let error =
            h.s3.list_objects_v2()
                .bucket("absent")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("NoSuchBucket"));
    }
}

mod objects {
    use super::*;

    async fn harness_with_bucket() -> conformance::Harness {
        let h = conformance::harness().await;
        h.s3.create_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();
        h
    }

    /// The worst of the fall-through cases: `PUT /{bucket}/{key}?retention` reaching
    /// `PutObject` stored the retention document *as the object*, answered `200`, and
    /// destroyed what was there. Silent data loss on a request that should be refused.
    #[tokio::test]
    async fn should_refuse_to_set_retention_rather_than_overwrite_the_object() {
        use aws_sdk_s3::types::{ObjectLockRetention, ObjectLockRetentionMode};

        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("notes.txt")
            .body(ByteStream::from_static(b"the real object"))
            .send()
            .await
            .unwrap();

        let error = h
            .s3
            .put_object_retention()
            .bucket("my-bucket")
            .key("notes.txt")
            .retention(
                ObjectLockRetention::builder()
                    .mode(ObjectLockRetentionMode::Governance)
                    .retain_until_date(aws_sdk_s3::primitives::DateTime::from_secs(4_000_000_000))
                    .build(),
            )
            .send()
            .await
            .unwrap_err()
            .into_service_error();
        assert_eq!(error.meta().code(), Some("NotImplemented"));

        let got =
            h.s3.get_object()
                .bucket("my-bucket")
                .key("notes.txt")
                .send()
                .await
                .unwrap();
        let body = got.body.collect().await.unwrap().into_bytes();
        assert_eq!(
            body.as_ref(),
            b"the real object",
            "the object must not have been replaced by the retention document"
        );
    }

    /// Copying a key onto itself changes nothing unless the metadata is being replaced,
    /// and S3 refuses it rather than rewriting the object. Allowing it hides a client
    /// bug — usually a sync that computed the wrong destination.
    #[tokio::test]
    async fn should_refuse_a_copy_of_an_object_onto_itself() {
        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("notes.txt")
            .body(ByteStream::from_static(b"one"))
            .send()
            .await
            .unwrap();

        let error =
            h.s3.copy_object()
                .bucket("my-bucket")
                .key("notes.txt")
                .copy_source("my-bucket/notes.txt")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.meta().code(), Some("InvalidRequest"));
    }

    /// The same copy is legal when it replaces the metadata, which is how a client
    /// changes an object's content type in place.
    #[tokio::test]
    async fn should_allow_a_copy_onto_itself_that_replaces_the_metadata() {
        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("notes.txt")
            .body(ByteStream::from_static(b"one"))
            .send()
            .await
            .unwrap();

        h.s3.copy_object()
            .bucket("my-bucket")
            .key("notes.txt")
            .copy_source("my-bucket/notes.txt")
            .metadata_directive(aws_sdk_s3::types::MetadataDirective::Replace)
            .content_type("text/plain")
            .send()
            .await
            .unwrap();

        let got =
            h.s3.get_object()
                .bucket("my-bucket")
                .key("notes.txt")
                .send()
                .await
                .unwrap();
        assert_eq!(got.content_type(), Some("text/plain"));
    }

    /// The `response-*` parameters override the stored headers for one response, which is
    /// how a presigned link serves an object under a different filename. Ignoring them
    /// answers with the stored values, which looks correct and defeats the link.
    #[tokio::test]
    async fn should_apply_the_response_header_overrides() {
        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("notes.txt")
            .content_type("text/plain")
            .body(ByteStream::from_static(b"one"))
            .send()
            .await
            .unwrap();

        let got =
            h.s3.get_object()
                .bucket("my-bucket")
                .key("notes.txt")
                .response_content_type("foo/bar")
                .response_content_disposition("attachment; filename=\"other.txt\"")
                .response_cache_control("no-cache")
                .response_content_language("esperanto")
                .send()
                .await
                .unwrap();

        assert_eq!(got.content_type(), Some("foo/bar"));
        assert_eq!(
            got.content_disposition(),
            Some("attachment; filename=\"other.txt\"")
        );
        assert_eq!(got.cache_control(), Some("no-cache"));
        assert_eq!(got.content_language(), Some("esperanto"));
    }

    /// `GetObjectAttributes` reaching `GetObject` answered with the object's bytes, which
    /// a client cannot parse and cannot distinguish from a server fault.
    #[tokio::test]
    async fn should_refuse_an_object_subresource_it_does_not_implement() {
        use aws_sdk_s3::types::ObjectAttributes;

        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("notes.txt")
            .body(ByteStream::from_static(b"hello"))
            .send()
            .await
            .unwrap();

        let attributes =
            h.s3.get_object_attributes()
                .bucket("my-bucket")
                .key("notes.txt")
                .object_attributes(ObjectAttributes::Etag)
                .send()
                .await
                .unwrap_err()
                .into_service_error();
        assert_eq!(attributes.meta().code(), Some("NotImplemented"));

        let legal_hold =
            h.s3.get_object_legal_hold()
                .bucket("my-bucket")
                .key("notes.txt")
                .send()
                .await
                .unwrap_err()
                .into_service_error();
        assert_eq!(legal_hold.meta().code(), Some("NotImplemented"));
    }

    #[tokio::test]
    async fn should_round_trip_an_object_byte_for_byte() {
        let h = harness_with_bucket().await;

        h.s3.put_object()
            .bucket("my-bucket")
            .key("my-file")
            .body(ByteStream::from_static(b"hello from mock-s3"))
            .content_type("text/plain")
            .send()
            .await
            .unwrap();

        let got =
            h.s3.get_object()
                .bucket("my-bucket")
                .key("my-file")
                .send()
                .await
                .unwrap();

        assert_eq!(got.content_type(), Some("text/plain"));
        let body = got.body.collect().await.unwrap().into_bytes();
        assert_eq!(&body[..], b"hello from mock-s3");
    }

    #[tokio::test]
    async fn should_round_trip_user_metadata() {
        let h = harness_with_bucket().await;

        h.s3.put_object()
            .bucket("my-bucket")
            .key("my-file")
            .body(ByteStream::from_static(b"x"))
            .metadata("owner", "abi")
            .send()
            .await
            .unwrap();

        let head =
            h.s3.head_object()
                .bucket("my-bucket")
                .key("my-file")
                .send()
                .await
                .unwrap();

        assert_eq!(
            head.metadata()
                .and_then(|meta| meta.get("owner"))
                .map(String::as_str),
            Some("abi")
        );
    }

    #[tokio::test]
    async fn should_report_the_object_size_on_head() {
        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("my-file")
            .body(ByteStream::from_static(b"0123456789"))
            .send()
            .await
            .unwrap();

        let head =
            h.s3.head_object()
                .bucket("my-bucket")
                .key("my-file")
                .send()
                .await
                .unwrap();

        assert_eq!(head.content_length(), Some(10));
    }

    #[tokio::test]
    async fn should_report_no_such_key_for_a_missing_object() {
        let h = harness_with_bucket().await;

        let error =
            h.s3.get_object()
                .bucket("my-bucket")
                .key("missing")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("NoSuchKey"));
    }

    #[tokio::test]
    async fn should_succeed_when_deleting_a_key_that_does_not_exist() {
        let h = harness_with_bucket().await;

        h.s3.delete_object()
            .bucket("my-bucket")
            .key("never-existed")
            .send()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_return_the_requested_byte_range() {
        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("my-file")
            .body(ByteStream::from_static(b"hello from mock-s3"))
            .send()
            .await
            .unwrap();

        let got =
            h.s3.get_object()
                .bucket("my-bucket")
                .key("my-file")
                .range("bytes=6-9")
                .send()
                .await
                .unwrap();

        let body = got.body.collect().await.unwrap().into_bytes();
        assert_eq!(&body[..], b"from");
    }

    #[tokio::test]
    async fn should_reject_a_key_that_climbs_out_of_the_data_directory() {
        let h = conformance::harness_on_disk().await;
        h.s3.create_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();

        let error =
            h.s3.put_object()
                .bucket("my-bucket")
                .key("../../../etc/passwd")
                .body(ByteStream::from_static(b"pwned"))
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("InvalidArgument"));
    }
}

mod listing {
    use super::*;

    async fn harness_with_keys(keys: &[&str]) -> conformance::Harness {
        let h = conformance::harness().await;
        h.s3.create_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();
        for key in keys {
            h.s3.put_object()
                .bucket("my-bucket")
                .key(*key)
                .body(ByteStream::from_static(b"x"))
                .send()
                .await
                .unwrap();
        }
        h
    }

    /// `max-keys=0` is a request for nothing. S3 answers it with an empty page that is
    /// not truncated, so a client has no token to follow and nothing to follow it to.
    #[tokio::test]
    async fn should_report_max_keys_of_zero_as_an_untruncated_empty_page() {
        let h = harness_with_keys(&["bar", "baz", "foo", "quxx"]).await;

        for truncated in [
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .max_keys(0)
                .send()
                .await
                .unwrap()
                .is_truncated(),
            h.s3.list_objects()
                .bucket("my-bucket")
                .max_keys(0)
                .send()
                .await
                .unwrap()
                .is_truncated(),
        ] {
            assert_eq!(truncated, Some(false));
        }
    }

    /// An empty continuation token is not an invalid one: it names no position, so the
    /// listing starts from the beginning — and S3 echoes the parameter it was given, so
    /// a client that sent one can read it back.
    #[tokio::test]
    async fn should_accept_and_echo_an_empty_continuation_token() {
        let h = harness_with_keys(&["bar", "baz", "foo", "quxx"]).await;

        let listed =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .continuation_token("")
                .send()
                .await
                .unwrap();

        assert_eq!(listed.continuation_token(), Some(""));
        assert_eq!(listed.is_truncated(), Some(false));
        assert_eq!(listed.contents().len(), 4);
    }

    #[tokio::test]
    async fn should_roll_keys_up_into_common_prefixes_when_a_delimiter_is_given() {
        let h = harness_with_keys(&["photos/2024/a.jpg", "photos/cover.png", "top"]).await;

        let listed =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .delimiter("/")
                .send()
                .await
                .unwrap();

        let prefixes: Vec<&str> = listed
            .common_prefixes()
            .iter()
            .filter_map(|prefix| prefix.prefix())
            .collect();
        let keys: Vec<&str> = listed.contents().iter().filter_map(|o| o.key()).collect();

        assert_eq!(prefixes, vec!["photos/"]);
        assert_eq!(keys, vec!["top"]);
    }

    #[tokio::test]
    async fn should_page_through_keys_with_a_continuation_token() {
        let h = harness_with_keys(&["a", "b", "c", "d"]).await;

        let first =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .max_keys(2)
                .send()
                .await
                .unwrap();
        assert_eq!(first.is_truncated(), Some(true));
        assert_eq!(first.key_count(), Some(2));

        let token = first
            .next_continuation_token()
            .expect("a truncated page carries a token");
        let second =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .max_keys(2)
                .continuation_token(token)
                .send()
                .await
                .unwrap();

        let keys: Vec<&str> = second.contents().iter().filter_map(|o| o.key()).collect();
        assert_eq!(keys, vec!["c", "d"]);
        assert_eq!(second.is_truncated(), Some(false));
    }

    #[tokio::test]
    async fn should_list_a_key_containing_xml_syntax() {
        let h = harness_with_keys(&["notes&misc.txt"]).await;

        let listed =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap();

        let keys: Vec<&str> = listed.contents().iter().filter_map(|o| o.key()).collect();
        assert_eq!(keys, vec!["notes&misc.txt"]);
    }

    #[tokio::test]
    async fn should_return_an_empty_listing_for_an_empty_bucket() {
        let h = harness_with_keys(&[]).await;

        let listed =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap();

        assert_eq!(listed.key_count(), Some(0));
        assert!(listed.contents().is_empty());
    }

    /// The continuation token travels back as a query parameter, where `+` means space.
    /// A token that is just the last key would resume from the wrong place for these.
    #[tokio::test]
    async fn should_paginate_through_keys_containing_a_plus() {
        let h = harness_with_keys(&["a+1", "a+2", "a+3"]).await;

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page =
                h.s3.list_objects_v2()
                    .bucket("my-bucket")
                    .max_keys(1)
                    .set_continuation_token(token.clone())
                    .send()
                    .await
                    .unwrap();

            seen.extend(
                page.contents()
                    .iter()
                    .filter_map(|o| o.key())
                    .map(String::from),
            );

            if page.is_truncated() == Some(true) {
                token = page.next_continuation_token().map(String::from);
            } else {
                break;
            }
        }

        assert_eq!(seen, vec!["a+1", "a+2", "a+3"]);
    }

    /// Paginating a versions listing needs both markers back. A client that validates
    /// its own parameters cannot even send the second request without the version
    /// marker, so a truncated page that omits it is a dead end.
    #[tokio::test]
    async fn should_paginate_a_versions_listing() {
        let h = harness_with_keys(&["a", "b", "c"]).await;

        let first =
            h.s3.list_object_versions()
                .bucket("my-bucket")
                .max_keys(2)
                .send()
                .await
                .unwrap();

        assert_eq!(first.is_truncated(), Some(true));
        assert!(first.next_key_marker().is_some());
        assert!(
            first.next_version_id_marker().is_some(),
            "a truncated versions page must carry NextVersionIdMarker"
        );

        let second =
            h.s3.list_object_versions()
                .bucket("my-bucket")
                .max_keys(2)
                .key_marker(first.next_key_marker().unwrap())
                .version_id_marker(first.next_version_id_marker().unwrap())
                .send()
                .await
                .unwrap();

        let keys: Vec<&str> = second.versions().iter().filter_map(|v| v.key()).collect();
        assert_eq!(keys, vec!["c"]);
        assert_eq!(second.is_truncated(), Some(false));
    }

    #[tokio::test]
    async fn should_report_every_object_as_its_own_latest_version() {
        let h = harness_with_keys(&["a", "b"]).await;

        let listed =
            h.s3.list_object_versions()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap();

        assert_eq!(listed.versions().len(), 2);
        for version in listed.versions() {
            assert_eq!(version.version_id(), Some("null"));
            assert_eq!(version.is_latest(), Some(true));
        }
    }

    /// A nested key keeps its separators under `encoding-type=url`, and so does the
    /// echoed prefix. S3 does not encode `/`, and a client that lists with a prefix sees
    /// that element without decoding it.
    #[tokio::test]
    async fn should_keep_path_separators_when_url_encoding_is_requested() {
        let h = harness_with_keys(&["photos/2024/a b.jpg"]).await;

        let listed =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .prefix("photos/")
                .encoding_type(aws_sdk_s3::types::EncodingType::Url)
                .send()
                .await
                .unwrap();

        assert_eq!(listed.prefix(), Some("photos/"));
        let keys: Vec<&str> = listed.contents().iter().filter_map(|o| o.key()).collect();
        assert_eq!(keys, vec!["photos/2024/a%20b.jpg"]);
    }

    #[tokio::test]
    async fn should_round_trip_keys_when_url_encoding_is_requested() {
        let h = harness_with_keys(&["a b+c.txt", "100%.txt"]).await;

        let listed =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .encoding_type(aws_sdk_s3::types::EncodingType::Url)
                .send()
                .await
                .unwrap();

        // The Rust SDK does not decode for you — it hands back exactly what the wire
        // carried, same as against real S3 — so what matters is that the request was
        // honoured and the encoding is right. `+` must come back as %2B, which is the
        // whole point: a client cannot otherwise tell it from a space.
        let keys: Vec<&str> = listed.contents().iter().filter_map(|o| o.key()).collect();
        assert_eq!(keys, vec!["100%25.txt", "a%20b%2Bc.txt"]);
        assert_eq!(
            listed.encoding_type(),
            Some(&aws_sdk_s3::types::EncodingType::Url)
        );
    }
}
