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

    #[tokio::test]
    async fn should_report_bucket_already_owned_by_you_on_a_repeat_create() {
        let h = conformance::harness().await;
        h.s3.create_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();

        let error =
            h.s3.create_bucket()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("BucketAlreadyOwnedByYou"));
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
