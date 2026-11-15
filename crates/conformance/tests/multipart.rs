//! Multipart uploads, copy and batch delete, through the real SDK.
//!
//! Multipart matters more than its share of the API suggests: SDKs switch to it above a
//! size threshold on their own, so a caller who never mentions multipart still depends
//! on it working.

use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier};

/// S3 requires every part but the last to be at least this large.
const MIN_PART: usize = 5 * 1024 * 1024;

async fn harness_with_bucket() -> conformance::Harness {
    let h = conformance::harness().await;
    h.s3.create_bucket()
        .bucket("my-bucket")
        .send()
        .await
        .unwrap();
    h
}

fn filler(size: usize, byte: u8) -> Vec<u8> {
    vec![byte; size]
}

mod multipart {
    use super::*;

    #[tokio::test]
    async fn should_round_trip_an_upload_assembled_from_several_parts() {
        let h = harness_with_bucket().await;
        let first = filler(MIN_PART, b'a');
        let second = filler(1024, b'b');

        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("big")
                .content_type("application/octet-stream")
                .send()
                .await
                .unwrap();
        let upload_id = created.upload_id().unwrap().to_string();

        let mut completed = Vec::new();
        for (number, payload) in [(1, &first), (2, &second)] {
            let part =
                h.s3.upload_part()
                    .bucket("my-bucket")
                    .key("big")
                    .upload_id(&upload_id)
                    .part_number(number)
                    .body(ByteStream::from(payload.clone()))
                    .send()
                    .await
                    .unwrap();
            completed.push(
                CompletedPart::builder()
                    .part_number(number)
                    .e_tag(part.e_tag().unwrap())
                    .build(),
            );
        }

        h.s3.complete_multipart_upload()
            .bucket("my-bucket")
            .key("big")
            .upload_id(&upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(completed))
                    .build(),
            )
            .send()
            .await
            .unwrap();

        let got =
            h.s3.get_object()
                .bucket("my-bucket")
                .key("big")
                .send()
                .await
                .unwrap();

        let body = got.body.collect().await.unwrap().into_bytes();
        let mut expected = first.clone();
        expected.extend_from_slice(&second);
        assert_eq!(body.len(), expected.len());
        assert_eq!(&body[..], &expected[..]);
    }

    #[tokio::test]
    async fn should_report_an_etag_with_a_part_count_suffix() {
        let h = harness_with_bucket().await;

        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("suffixed")
                .send()
                .await
                .unwrap();
        let upload_id = created.upload_id().unwrap().to_string();

        let part =
            h.s3.upload_part()
                .bucket("my-bucket")
                .key("suffixed")
                .upload_id(&upload_id)
                .part_number(1)
                .body(ByteStream::from(filler(1024, b'x')))
                .send()
                .await
                .unwrap();

        let done =
            h.s3.complete_multipart_upload()
                .bucket("my-bucket")
                .key("suffixed")
                .upload_id(&upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .parts(
                            CompletedPart::builder()
                                .part_number(1)
                                .e_tag(part.e_tag().unwrap())
                                .build(),
                        )
                        .build(),
                )
                .send()
                .await
                .unwrap();

        // A multipart etag is a digest over the parts' digests, then `-<count>`. It is
        // not the MD5 of the assembled object, and clients check the shape.
        let etag = done.e_tag().unwrap();
        assert!(etag.ends_with("-1\""), "etag was {etag}");
    }

    #[tokio::test]
    async fn should_reject_a_non_final_part_below_the_minimum_size() {
        let h = harness_with_bucket().await;

        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("too-small")
                .send()
                .await
                .unwrap();
        let upload_id = created.upload_id().unwrap().to_string();

        let mut completed = Vec::new();
        for number in 1..=2 {
            let part =
                h.s3.upload_part()
                    .bucket("my-bucket")
                    .key("too-small")
                    .upload_id(&upload_id)
                    .part_number(number)
                    .body(ByteStream::from(filler(1024, b'x')))
                    .send()
                    .await
                    .unwrap();
            completed.push(
                CompletedPart::builder()
                    .part_number(number)
                    .e_tag(part.e_tag().unwrap())
                    .build(),
            );
        }

        let error =
            h.s3.complete_multipart_upload()
                .bucket("my-bucket")
                .key("too-small")
                .upload_id(&upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(completed))
                        .build(),
                )
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("EntityTooSmall"));
    }

    /// Clients retry this call when it times out, and S3 answers the retry rather than
    /// reporting the upload gone. A 404 here turns a successful upload into a failure.
    #[tokio::test]
    async fn should_answer_a_repeated_complete_with_the_same_result() {
        let h = harness_with_bucket().await;
        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("retried")
                .send()
                .await
                .unwrap();
        let upload_id = created.upload_id().unwrap().to_string();

        let part =
            h.s3.upload_part()
                .bucket("my-bucket")
                .key("retried")
                .upload_id(&upload_id)
                .part_number(1)
                .body(ByteStream::from(filler(256, b'x')))
                .send()
                .await
                .unwrap();
        let completed = CompletedMultipartUpload::builder()
            .parts(
                CompletedPart::builder()
                    .part_number(1)
                    .e_tag(part.e_tag().unwrap())
                    .build(),
            )
            .build();

        let complete = || {
            h.s3.complete_multipart_upload()
                .bucket("my-bucket")
                .key("retried")
                .upload_id(&upload_id)
                .multipart_upload(completed.clone())
                .send()
        };

        let first = complete().await.unwrap();
        let second = complete().await.unwrap();

        assert_eq!(first.e_tag(), second.e_tag());
        assert!(first.e_tag().unwrap().ends_with("-1\""));
    }

    /// Bookkeeping left over from a finished upload is invisible to a client, so it must
    /// not be what stops them deleting the bucket.
    #[tokio::test]
    async fn should_allow_deleting_a_bucket_after_an_upload_finished() {
        let h = harness_with_bucket().await;
        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("done")
                .send()
                .await
                .unwrap();
        let upload_id = created.upload_id().unwrap().to_string();
        let part =
            h.s3.upload_part()
                .bucket("my-bucket")
                .key("done")
                .upload_id(&upload_id)
                .part_number(1)
                .body(ByteStream::from(filler(128, b'x')))
                .send()
                .await
                .unwrap();
        h.s3.complete_multipart_upload()
            .bucket("my-bucket")
            .key("done")
            .upload_id(&upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .parts(
                        CompletedPart::builder()
                            .part_number(1)
                            .e_tag(part.e_tag().unwrap())
                            .build(),
                    )
                    .build(),
            )
            .send()
            .await
            .unwrap();

        h.s3.delete_object()
            .bucket("my-bucket")
            .key("done")
            .send()
            .await
            .unwrap();

        h.s3.delete_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();
    }

    /// An upload still in flight does not block the delete.
    ///
    /// S3 conditions `DeleteBucket` on objects only, and clients delete a bucket's
    /// objects and then the bucket without aborting uploads first — the ceph/s3-tests
    /// cleanup among them. Refusing here strands the bucket for good.
    #[tokio::test]
    async fn should_delete_a_bucket_with_an_upload_still_in_flight() {
        let h = harness_with_bucket().await;
        h.s3.create_multipart_upload()
            .bucket("my-bucket")
            .key("pending")
            .send()
            .await
            .unwrap();

        h.s3.delete_bucket()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();

        // Gone, along with the upload it was holding.
        let error =
            h.s3.list_multipart_uploads()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap_err()
                .into_service_error();
        assert_eq!(error.code(), Some("NoSuchBucket"));
    }

    #[tokio::test]
    async fn should_report_no_such_upload_for_an_unknown_id() {
        let h = harness_with_bucket().await;

        let error =
            h.s3.list_parts()
                .bucket("my-bucket")
                .key("whatever")
                .upload_id("not-a-real-upload")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("NoSuchUpload"));
    }

    #[tokio::test]
    async fn should_list_the_parts_uploaded_so_far() {
        let h = harness_with_bucket().await;

        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("listed")
                .send()
                .await
                .unwrap();
        let upload_id = created.upload_id().unwrap().to_string();

        for number in 1..=3 {
            h.s3.upload_part()
                .bucket("my-bucket")
                .key("listed")
                .upload_id(&upload_id)
                .part_number(number)
                .body(ByteStream::from(filler(512, b'x')))
                .send()
                .await
                .unwrap();
        }

        let listed =
            h.s3.list_parts()
                .bucket("my-bucket")
                .key("listed")
                .upload_id(&upload_id)
                .send()
                .await
                .unwrap();

        let numbers: Vec<i32> = listed
            .parts()
            .iter()
            .filter_map(|part| part.part_number())
            .collect();
        assert_eq!(numbers, vec![1, 2, 3]);
        assert_eq!(listed.parts()[0].size(), Some(512));
    }

    #[tokio::test]
    async fn should_list_an_upload_that_is_still_in_flight() {
        let h = harness_with_bucket().await;
        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("pending")
                .send()
                .await
                .unwrap();

        let listed =
            h.s3.list_multipart_uploads()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap();

        let keys: Vec<&str> = listed
            .uploads()
            .iter()
            .filter_map(|upload| upload.key())
            .collect();
        assert_eq!(keys, vec!["pending"]);
        assert_eq!(
            listed.uploads()[0].upload_id(),
            Some(created.upload_id().unwrap())
        );
    }

    #[tokio::test]
    async fn should_abort_an_upload_and_forget_it() {
        let h = harness_with_bucket().await;
        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("doomed")
                .send()
                .await
                .unwrap();
        let upload_id = created.upload_id().unwrap().to_string();

        h.s3.abort_multipart_upload()
            .bucket("my-bucket")
            .key("doomed")
            .upload_id(&upload_id)
            .send()
            .await
            .unwrap();

        let error =
            h.s3.list_parts()
                .bucket("my-bucket")
                .key("doomed")
                .upload_id(&upload_id)
                .send()
                .await
                .unwrap_err()
                .into_service_error();
        assert_eq!(error.code(), Some("NoSuchUpload"));
    }

    /// Parts are stored as objects under a reserved prefix, so a listing has to hide
    /// them — otherwise a client sees an in-flight upload as object data.
    #[tokio::test]
    async fn should_hide_the_parts_of_an_in_flight_upload_from_a_listing() {
        let h = harness_with_bucket().await;
        let created =
            h.s3.create_multipart_upload()
                .bucket("my-bucket")
                .key("hidden")
                .send()
                .await
                .unwrap();
        h.s3.upload_part()
            .bucket("my-bucket")
            .key("hidden")
            .upload_id(created.upload_id().unwrap())
            .part_number(1)
            .body(ByteStream::from(filler(32, b'x')))
            .send()
            .await
            .unwrap();

        let listed =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap();

        assert_eq!(listed.key_count(), Some(0));
        assert!(listed.contents().is_empty());
    }
}

mod copy_object {
    use super::*;

    #[tokio::test]
    async fn should_copy_an_object_with_its_metadata() {
        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("source")
            .body(ByteStream::from_static(b"payload"))
            .content_type("text/plain")
            .metadata("owner", "abi")
            .send()
            .await
            .unwrap();

        h.s3.copy_object()
            .bucket("my-bucket")
            .key("destination")
            .copy_source("my-bucket/source")
            .send()
            .await
            .unwrap();

        let got =
            h.s3.get_object()
                .bucket("my-bucket")
                .key("destination")
                .send()
                .await
                .unwrap();

        assert_eq!(got.content_type(), Some("text/plain"));
        assert_eq!(
            got.metadata()
                .and_then(|meta| meta.get("owner"))
                .map(String::as_str),
            Some("abi")
        );
        let body = got.body.collect().await.unwrap().into_bytes();
        assert_eq!(&body[..], b"payload");
    }

    #[tokio::test]
    async fn should_replace_metadata_when_the_directive_says_to() {
        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("source")
            .body(ByteStream::from_static(b"payload"))
            .content_type("text/plain")
            .metadata("owner", "abi")
            .send()
            .await
            .unwrap();

        h.s3.copy_object()
            .bucket("my-bucket")
            .key("destination")
            .copy_source("my-bucket/source")
            .metadata_directive(aws_sdk_s3::types::MetadataDirective::Replace)
            .content_type("application/json")
            .metadata("owner", "someone-else")
            .send()
            .await
            .unwrap();

        let got =
            h.s3.head_object()
                .bucket("my-bucket")
                .key("destination")
                .send()
                .await
                .unwrap();

        assert_eq!(got.content_type(), Some("application/json"));
        assert_eq!(
            got.metadata()
                .and_then(|meta| meta.get("owner"))
                .map(String::as_str),
            Some("someone-else")
        );
    }

    #[tokio::test]
    async fn should_report_no_such_key_for_a_missing_source() {
        let h = harness_with_bucket().await;

        let error =
            h.s3.copy_object()
                .bucket("my-bucket")
                .key("destination")
                .copy_source("my-bucket/absent")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("NoSuchKey"));
    }
}

mod delete_objects {
    use super::*;

    #[tokio::test]
    async fn should_delete_every_listed_key_in_one_call() {
        let h = harness_with_bucket().await;
        for key in ["a", "b", "c"] {
            h.s3.put_object()
                .bucket("my-bucket")
                .key(key)
                .body(ByteStream::from_static(b"x"))
                .send()
                .await
                .unwrap();
        }

        let report =
            h.s3.delete_objects()
                .bucket("my-bucket")
                .delete(
                    Delete::builder()
                        .objects(ObjectIdentifier::builder().key("a").build().unwrap())
                        .objects(ObjectIdentifier::builder().key("b").build().unwrap())
                        .build()
                        .unwrap(),
                )
                .send()
                .await
                .unwrap();

        assert_eq!(report.deleted().len(), 2);
        assert!(report.errors().is_empty());

        let listed =
            h.s3.list_objects_v2()
                .bucket("my-bucket")
                .send()
                .await
                .unwrap();
        let keys: Vec<&str> = listed.contents().iter().filter_map(|o| o.key()).collect();
        assert_eq!(keys, vec!["c"]);
    }

    #[tokio::test]
    async fn should_report_nothing_deleted_in_quiet_mode() {
        let h = harness_with_bucket().await;
        h.s3.put_object()
            .bucket("my-bucket")
            .key("a")
            .body(ByteStream::from_static(b"x"))
            .send()
            .await
            .unwrap();

        let report =
            h.s3.delete_objects()
                .bucket("my-bucket")
                .delete(
                    Delete::builder()
                        .objects(ObjectIdentifier::builder().key("a").build().unwrap())
                        .quiet(true)
                        .build()
                        .unwrap(),
                )
                .send()
                .await
                .unwrap();

        assert!(report.deleted().is_empty());
        assert!(report.errors().is_empty());
    }

    #[tokio::test]
    async fn should_succeed_for_a_key_that_was_never_there() {
        let h = harness_with_bucket().await;

        let report =
            h.s3.delete_objects()
                .bucket("my-bucket")
                .delete(
                    Delete::builder()
                        .objects(ObjectIdentifier::builder().key("absent").build().unwrap())
                        .build()
                        .unwrap(),
                )
                .send()
                .await
                .unwrap();

        assert_eq!(report.deleted().len(), 1);
        assert!(report.errors().is_empty());
    }
}
