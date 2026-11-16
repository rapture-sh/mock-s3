//! Bucket versioning through the real SDK.
//!
//! The engine's own rules are unit-tested in `mock-s3-core`. What is proved here is the
//! wire: that the SDK's own `version_id` plumbing reads the headers mock-s3 sends, that a
//! delete marker is reported the way the SDK expects, and that a versions listing parses.

use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{
    BucketVersioningStatus, Delete, ObjectIdentifier, VersioningConfiguration,
};

const BUCKET: &str = "my-bucket";

/// A harness whose bucket already has versioning enabled.
async fn versioned() -> conformance::Harness {
    let h = conformance::harness().await;
    h.s3.create_bucket().bucket(BUCKET).send().await.unwrap();
    enable(&h, BucketVersioningStatus::Enabled).await;
    h
}

async fn enable(h: &conformance::Harness, status: BucketVersioningStatus) {
    h.s3.put_bucket_versioning()
        .bucket(BUCKET)
        .versioning_configuration(VersioningConfiguration::builder().status(status).build())
        .send()
        .await
        .unwrap();
}

/// Store an object and return the version id the server assigned.
async fn put(h: &conformance::Harness, key: &str, body: &'static str) -> Option<String> {
    h.s3.put_object()
        .bucket(BUCKET)
        .key(key)
        .body(ByteStream::from_static(body.as_bytes()))
        .send()
        .await
        .unwrap()
        .version_id
}

async fn body_of(h: &conformance::Harness, key: &str, version_id: Option<&str>) -> String {
    let got =
        h.s3.get_object()
            .bucket(BUCKET)
            .key(key)
            .set_version_id(version_id.map(str::to_string))
            .send()
            .await
            .unwrap();

    let bytes = got.body.collect().await.unwrap().into_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

mod get_bucket_versioning {
    use super::*;

    /// A bucket that has never been configured reports no status at all, which is not
    /// the same as `Suspended`: an SDK reads the empty document as `None`.
    #[tokio::test]
    async fn should_report_no_status_before_versioning_is_configured() {
        let h = conformance::harness().await;
        h.s3.create_bucket().bucket(BUCKET).send().await.unwrap();

        let got =
            h.s3.get_bucket_versioning()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();

        assert_eq!(got.status(), None);
    }

    #[tokio::test]
    async fn should_report_each_status_it_was_set_to() {
        let h = versioned().await;

        let enabled =
            h.s3.get_bucket_versioning()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();
        assert_eq!(enabled.status(), Some(&BucketVersioningStatus::Enabled));

        enable(&h, BucketVersioningStatus::Suspended).await;
        let suspended =
            h.s3.get_bucket_versioning()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();
        assert_eq!(suspended.status(), Some(&BucketVersioningStatus::Suspended));
    }

    #[tokio::test]
    async fn should_report_no_such_bucket_for_a_bucket_that_is_not_there() {
        let h = conformance::harness().await;

        let error =
            h.s3.get_bucket_versioning()
                .bucket("absent-bucket")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("NoSuchBucket"));
    }
}

mod put_object {
    use super::*;

    /// The absence of the header is how a client tells an unversioned bucket from a
    /// versioned one, so it has to stay absent.
    #[tokio::test]
    async fn should_not_report_a_version_in_an_unversioned_bucket() {
        let h = conformance::harness().await;
        h.s3.create_bucket().bucket(BUCKET).send().await.unwrap();

        assert_eq!(put(&h, "notes.txt", "one").await, None);
    }

    #[tokio::test]
    async fn should_report_a_new_version_for_every_write() {
        let h = versioned().await;

        let first = put(&h, "notes.txt", "one").await.unwrap();
        let second = put(&h, "notes.txt", "two").await.unwrap();

        assert_ne!(first, second);
        assert_eq!(body_of(&h, "notes.txt", None).await, "two");
        assert_eq!(body_of(&h, "notes.txt", Some(&first)).await, "one");
    }

    /// Enabling versioning on a bucket that already holds objects must not lose them.
    /// They become the `null` version, which is what real S3 reports too.
    #[tokio::test]
    async fn should_keep_an_object_written_before_versioning_was_enabled() {
        let h = conformance::harness().await;
        h.s3.create_bucket().bucket(BUCKET).send().await.unwrap();
        put(&h, "notes.txt", "before").await;

        enable(&h, BucketVersioningStatus::Enabled).await;

        assert_eq!(body_of(&h, "notes.txt", Some("null")).await, "before");
        assert_eq!(body_of(&h, "notes.txt", None).await, "before");
    }

    /// A suspended write stores the null version, so reporting `null` reads as the
    /// consistent choice, a suspended *delete* does report it. Real S3 does not, and
    /// ceph/s3-tests asserts it: "for versioning-suspended-bucket, no version-id should
    /// return". This test is here so the reasoning does not win a second time.
    #[tokio::test]
    async fn should_not_report_a_version_for_a_write_into_a_suspended_bucket() {
        let h = versioned().await;
        enable(&h, BucketVersioningStatus::Suspended).await;

        assert_eq!(put(&h, "notes.txt", "one").await, None);

        // The delete is the asymmetric half, and it is the half that reports `null`.
        let deleted =
            h.s3.delete_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap();
        assert_eq!(deleted.version_id(), Some("null"));
    }

    #[tokio::test]
    async fn should_stop_collecting_versions_once_suspended() {
        let h = versioned().await;
        let kept = put(&h, "notes.txt", "one").await.unwrap();

        enable(&h, BucketVersioningStatus::Suspended).await;
        put(&h, "notes.txt", "two").await;
        put(&h, "notes.txt", "three").await;

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();
        assert_eq!(listed.versions().len(), 2);
        assert_eq!(body_of(&h, "notes.txt", None).await, "three");
        assert_eq!(body_of(&h, "notes.txt", Some(&kept)).await, "one");
    }
}

mod get_object {
    use super::*;

    #[tokio::test]
    async fn should_report_the_version_it_served() {
        let h = versioned().await;
        let written = put(&h, "notes.txt", "one").await.unwrap();

        let got =
            h.s3.get_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap();

        assert_eq!(got.version_id(), Some(written.as_str()));
    }

    #[tokio::test]
    async fn should_report_no_such_version_for_an_id_that_names_nothing() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;

        let error =
            h.s3.get_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .version_id("deadbeef")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("NoSuchVersion"));
    }

    #[tokio::test]
    async fn should_serve_a_range_of_an_older_version() {
        let h = versioned().await;
        let first = put(&h, "notes.txt", "0123456789").await.unwrap();
        put(&h, "notes.txt", "overwritten").await;

        let got =
            h.s3.get_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .version_id(&first)
                .range("bytes=2-4")
                .send()
                .await
                .unwrap();

        let bytes = got.body.collect().await.unwrap().into_bytes();
        assert_eq!(bytes.as_ref(), b"234");
    }
}

mod head_object {
    use super::*;

    #[tokio::test]
    async fn should_report_the_version_of_the_metadata_it_returned() {
        let h = versioned().await;
        let first = put(&h, "notes.txt", "one").await.unwrap();
        let second = put(&h, "notes.txt", "two but longer").await.unwrap();

        let older =
            h.s3.head_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .version_id(&first)
                .send()
                .await
                .unwrap();
        let current =
            h.s3.head_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap();

        assert_eq!(older.version_id(), Some(first.as_str()));
        assert_eq!(older.content_length(), Some(3));
        assert_eq!(current.version_id(), Some(second.as_str()));
        assert_eq!(current.content_length(), Some(14));
    }
}

mod delete_object {
    use super::*;

    #[tokio::test]
    async fn should_leave_a_delete_marker_rather_than_remove_the_object() {
        let h = versioned().await;
        let written = put(&h, "notes.txt", "one").await.unwrap();

        let deleted =
            h.s3.delete_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap();

        assert_eq!(deleted.delete_marker(), Some(true));
        assert!(deleted.version_id().is_some_and(|id| id != written));
        assert_eq!(body_of(&h, "notes.txt", Some(&written)).await, "one");
    }

    /// The 404 has to carry `x-amz-delete-marker`, because that header is the only thing
    /// separating "deleted" from "never existed".
    #[tokio::test]
    async fn should_answer_a_deleted_key_with_a_not_found_that_says_it_was_deleted() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        h.s3.delete_object()
            .bucket(BUCKET)
            .key("notes.txt")
            .send()
            .await
            .unwrap();

        let error =
            h.s3.get_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap_err();

        let response = error.raw_response().expect("the error carries a response");
        assert_eq!(response.status().as_u16(), 404);
        assert_eq!(
            response.headers().get("x-amz-delete-marker"),
            Some("true"),
            "without this a client cannot tell a deleted key from one never written"
        );
        assert_eq!(error.into_service_error().code(), Some("NoSuchKey"));
    }

    /// S3 answers a delete marker named by its own id with a 405, not a 404: the marker
    /// is there, it simply has no body. The only thing left to do to it is delete it.
    #[tokio::test]
    async fn should_refuse_to_read_a_delete_marker_named_by_its_id() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        let marker =
            h.s3.delete_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap()
                .version_id
                .unwrap();

        let error =
            h.s3.get_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .version_id(&marker)
                .send()
                .await
                .unwrap_err();

        let response = error.raw_response().expect("the error carries a response");
        assert_eq!(response.status().as_u16(), 405);
        assert_eq!(response.headers().get("x-amz-delete-marker"), Some("true"));
    }

    /// Removing the delete marker is how a client undoes a delete.
    #[tokio::test]
    async fn should_restore_the_object_when_its_delete_marker_is_removed() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        let marker =
            h.s3.delete_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap()
                .version_id
                .unwrap();

        let removed =
            h.s3.delete_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .version_id(&marker)
                .send()
                .await
                .unwrap();

        assert_eq!(removed.delete_marker(), Some(true));
        assert_eq!(body_of(&h, "notes.txt", None).await, "one");
    }

    #[tokio::test]
    async fn should_remove_one_version_outright_when_its_id_is_named() {
        let h = versioned().await;
        let first = put(&h, "notes.txt", "one").await.unwrap();
        put(&h, "notes.txt", "two").await;

        let removed =
            h.s3.delete_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .version_id(&first)
                .send()
                .await
                .unwrap();

        assert_eq!(removed.delete_marker(), None);
        assert_eq!(removed.version_id(), Some(first.as_str()));
        assert_eq!(body_of(&h, "notes.txt", None).await, "two");
    }
}

mod delete_objects {
    use super::*;

    #[tokio::test]
    async fn should_leave_a_delete_marker_for_a_key_with_no_version_named() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;

        let result =
            h.s3.delete_objects()
                .bucket(BUCKET)
                .delete(
                    Delete::builder()
                        .objects(
                            ObjectIdentifier::builder()
                                .key("notes.txt")
                                .build()
                                .unwrap(),
                        )
                        .build()
                        .unwrap(),
                )
                .send()
                .await
                .unwrap();

        let deleted = result.deleted();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].delete_marker(), Some(true));
        assert!(deleted[0].delete_marker_version_id().is_some());
    }

    /// Two clients emptying a bucket at once both ask for every version, so the second
    /// request to arrive deletes things that are already gone, and has to report
    /// success, because the outcome it asked for is exactly what happened. Repeating the
    /// same batch is the same situation without the threads.
    #[tokio::test]
    async fn should_report_success_when_the_same_batch_is_deleted_twice() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        put(&h, "notes.txt", "two").await;

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();
        let targets: Vec<ObjectIdentifier> = listed
            .versions()
            .iter()
            .map(|version| {
                ObjectIdentifier::builder()
                    .key(version.key().unwrap())
                    .set_version_id(version.version_id.clone())
                    .build()
                    .unwrap()
            })
            .collect();
        assert_eq!(targets.len(), 2);

        for attempt in 1..=2 {
            let result =
                h.s3.delete_objects()
                    .bucket(BUCKET)
                    .delete(
                        Delete::builder()
                            .set_objects(Some(targets.clone()))
                            .build()
                            .unwrap(),
                    )
                    .send()
                    .await
                    .unwrap();

            assert_eq!(
                result.errors().len(),
                0,
                "attempt {attempt} reported errors"
            );
            assert_eq!(result.deleted().len(), 2, "attempt {attempt}");
        }
    }

    /// Enumerating `?versions` and deleting each one is how a client empties a versioned
    /// bucket, so the two have to agree on what is there.
    #[tokio::test]
    async fn should_empty_a_versioned_bucket_when_every_version_is_named() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        put(&h, "notes.txt", "two").await;
        h.s3.delete_object()
            .bucket(BUCKET)
            .key("notes.txt")
            .send()
            .await
            .unwrap();

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();
        let mut targets: Vec<ObjectIdentifier> = Vec::new();
        for version in listed.versions() {
            targets.push(
                ObjectIdentifier::builder()
                    .key(version.key().unwrap())
                    .set_version_id(version.version_id.clone())
                    .build()
                    .unwrap(),
            );
        }
        for marker in listed.delete_markers() {
            targets.push(
                ObjectIdentifier::builder()
                    .key(marker.key().unwrap())
                    .set_version_id(marker.version_id.clone())
                    .build()
                    .unwrap(),
            );
        }
        assert_eq!(targets.len(), 3);

        h.s3.delete_objects()
            .bucket(BUCKET)
            .delete(
                Delete::builder()
                    .set_objects(Some(targets))
                    .build()
                    .unwrap(),
            )
            .send()
            .await
            .unwrap();

        h.s3.delete_bucket().bucket(BUCKET).send().await.unwrap();
    }
}

mod list_object_versions {
    use super::*;

    #[tokio::test]
    async fn should_report_every_version_newest_first() {
        let h = versioned().await;
        let first = put(&h, "notes.txt", "one").await.unwrap();
        let second = put(&h, "notes.txt", "two").await.unwrap();

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();

        let versions = listed.versions();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version_id(), Some(second.as_str()));
        assert_eq!(versions[0].is_latest(), Some(true));
        assert_eq!(versions[1].version_id(), Some(first.as_str()));
        assert_eq!(versions[1].is_latest(), Some(false));
        assert_eq!(versions[0].size(), Some(3));
    }

    #[tokio::test]
    async fn should_report_a_delete_marker_separately_from_a_version() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        h.s3.delete_object()
            .bucket(BUCKET)
            .key("notes.txt")
            .send()
            .await
            .unwrap();

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();

        assert_eq!(listed.versions().len(), 1);
        assert_eq!(listed.delete_markers().len(), 1);
        assert_eq!(listed.delete_markers()[0].is_latest(), Some(true));
        assert_eq!(listed.versions()[0].is_latest(), Some(false));
    }

    /// An unversioned bucket still answers here, with one `null` version per object.
    /// Clients enumerate this before deleting a bucket whatever its versioning is.
    #[tokio::test]
    async fn should_report_a_null_version_for_each_object_in_an_unversioned_bucket() {
        let h = conformance::harness().await;
        h.s3.create_bucket().bucket(BUCKET).send().await.unwrap();
        put(&h, "notes.txt", "one").await;

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();

        let versions = listed.versions();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version_id(), Some("null"));
        assert_eq!(versions[0].is_latest(), Some(true));
    }

    #[tokio::test]
    async fn should_roll_up_versions_under_a_delimiter() {
        let h = versioned().await;
        put(&h, "photos/one.jpg", "one").await;
        put(&h, "photos/one.jpg", "again").await;
        put(&h, "notes.txt", "one").await;

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .delimiter("/")
                .send()
                .await
                .unwrap();

        assert_eq!(listed.common_prefixes().len(), 1);
        assert_eq!(listed.common_prefixes()[0].prefix(), Some("photos/"));
        assert_eq!(listed.versions().len(), 1);
        assert_eq!(listed.versions()[0].key(), Some("notes.txt"));
    }

    /// A key can hold more versions than a page can carry, so a client has to be able to
    /// resume inside one. Both markers come back, and feeding them in continues.
    #[tokio::test]
    async fn should_paginate_within_a_single_key() {
        let h = versioned().await;
        let mut written = Vec::new();
        for body in ["one", "two", "three"] {
            written.push(put(&h, "notes.txt", body).await.unwrap());
        }
        written.reverse();

        let first =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .max_keys(2)
                .send()
                .await
                .unwrap();

        assert_eq!(first.is_truncated(), Some(true));
        assert_eq!(first.next_key_marker(), Some("notes.txt"));
        assert_eq!(first.next_version_id_marker(), Some(written[1].as_str()));

        let second =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .max_keys(2)
                .set_key_marker(first.next_key_marker.clone())
                .set_version_id_marker(first.next_version_id_marker.clone())
                .send()
                .await
                .unwrap();

        assert_eq!(second.is_truncated(), Some(false));
        let seen: Vec<&str> = first
            .versions()
            .iter()
            .chain(second.versions())
            .map(|version| version.version_id().unwrap())
            .collect();
        assert_eq!(seen, written);
    }

    #[tokio::test]
    async fn should_keep_only_the_versions_under_a_prefix() {
        let h = versioned().await;
        put(&h, "photos/one.jpg", "one").await;
        put(&h, "notes.txt", "one").await;

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .prefix("photos/")
                .send()
                .await
                .unwrap();

        assert_eq!(listed.versions().len(), 1);
        assert_eq!(listed.versions()[0].key(), Some("photos/one.jpg"));
    }
}

mod list_objects_v2 {
    use super::*;

    /// A plain listing shows one entry per key, and a key whose current version is a
    /// delete marker is not there at all.
    #[tokio::test]
    async fn should_show_only_the_current_version_of_each_key() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        put(&h, "notes.txt", "two").await;
        put(&h, "other.txt", "one").await;
        h.s3.delete_object()
            .bucket(BUCKET)
            .key("other.txt")
            .send()
            .await
            .unwrap();

        let listed = h.s3.list_objects_v2().bucket(BUCKET).send().await.unwrap();

        let keys: Vec<&str> = listed
            .contents()
            .iter()
            .map(|object| object.key().unwrap())
            .collect();
        assert_eq!(keys, vec!["notes.txt"]);
        assert_eq!(listed.contents()[0].size(), Some(3));
    }
}

mod copy_object {
    use super::*;

    #[tokio::test]
    async fn should_copy_the_version_the_source_names() {
        let h = versioned().await;
        let first = put(&h, "notes.txt", "one").await.unwrap();
        put(&h, "notes.txt", "two").await;

        let copied =
            h.s3.copy_object()
                .bucket(BUCKET)
                .key("copy.txt")
                .copy_source(format!("{BUCKET}/notes.txt?versionId={first}"))
                .send()
                .await
                .unwrap();

        assert_eq!(copied.copy_source_version_id(), Some(first.as_str()));
        assert!(copied.version_id().is_some());
        assert_eq!(body_of(&h, "copy.txt", None).await, "one");
    }

    #[tokio::test]
    async fn should_copy_the_current_version_when_the_source_names_none() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        put(&h, "notes.txt", "two").await;

        h.s3.copy_object()
            .bucket(BUCKET)
            .key("copy.txt")
            .copy_source(format!("{BUCKET}/notes.txt"))
            .send()
            .await
            .unwrap();

        assert_eq!(body_of(&h, "copy.txt", None).await, "two");
    }
}

mod delete_bucket {
    use super::*;

    /// The listing is empty once the current version is a delete marker, but the bucket
    /// is not: a version a client can still read is a version that still has to be
    /// deleted before the bucket can go.
    #[tokio::test]
    async fn should_refuse_while_a_version_is_still_there() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        h.s3.delete_object()
            .bucket(BUCKET)
            .key("notes.txt")
            .send()
            .await
            .unwrap();

        let listed = h.s3.list_objects_v2().bucket(BUCKET).send().await.unwrap();
        assert_eq!(listed.contents().len(), 0);

        let error =
            h.s3.delete_bucket()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap_err()
                .into_service_error();
        assert_eq!(error.code(), Some("BucketNotEmpty"));
    }
}

mod head_object_on_a_deleted_key {
    use super::*;

    /// HEAD is the one verb with no error body, so the header is all a client has to
    /// tell a deleted key from one that was never written.
    #[tokio::test]
    async fn should_report_the_delete_marker_in_the_headers() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        let marker =
            h.s3.delete_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap()
                .version_id
                .unwrap();

        let error =
            h.s3.head_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap_err();

        let response = error.raw_response().expect("the error carries a response");
        assert_eq!(response.status().as_u16(), 404);
        assert_eq!(response.headers().get("x-amz-delete-marker"), Some("true"));
        assert_eq!(
            response.headers().get("x-amz-version-id"),
            Some(marker.as_str())
        );
    }

    /// The header has to be on *both* answers to be worth anything: a client comparing
    /// `true` against a missing header cannot tell "deleted" from "never existed", so S3
    /// sends `false` for a key that is simply not there.
    #[tokio::test]
    async fn should_report_no_delete_marker_for_a_key_that_was_never_versioned() {
        let h = conformance::harness().await;
        h.s3.create_bucket().bucket(BUCKET).send().await.unwrap();
        put(&h, "notes.txt", "one").await;
        h.s3.delete_object()
            .bucket(BUCKET)
            .key("notes.txt")
            .send()
            .await
            .unwrap();

        let error =
            h.s3.head_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap_err();

        let response = error.raw_response().expect("the error carries a response");
        assert_eq!(response.status().as_u16(), 404);
        assert_eq!(response.headers().get("x-amz-delete-marker"), Some("false"));
    }

    #[tokio::test]
    async fn should_refuse_a_head_of_a_delete_marker_named_by_its_id() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;
        let marker =
            h.s3.delete_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap()
                .version_id
                .unwrap();

        let error =
            h.s3.head_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .version_id(&marker)
                .send()
                .await
                .unwrap_err();

        let response = error.raw_response().expect("the error carries a response");
        assert_eq!(response.status().as_u16(), 405);
        assert_eq!(response.headers().get("x-amz-delete-marker"), Some("true"));
    }
}

mod version_markers {
    use super::*;

    /// A version id is only meaningful against a key. S3 rejects one sent alone, so a
    /// client with that bug has to fail here too rather than pass and fail on AWS.
    #[tokio::test]
    async fn should_reject_a_version_id_marker_sent_without_a_key_marker() {
        let h = versioned().await;
        put(&h, "notes.txt", "one").await;

        let error =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .version_id_marker("v1")
                .send()
                .await
                .unwrap_err()
                .into_service_error();

        assert_eq!(error.code(), Some("InvalidArgument"));
    }

    /// A page can end on a common prefix, which has no version to resume from. Feeding
    /// the markers back has to continue the listing rather than restart or stall it.
    #[tokio::test]
    async fn should_resume_a_page_that_ended_on_a_common_prefix() {
        let h = versioned().await;
        put(&h, "photos/one.jpg", "one").await;
        put(&h, "photos/two.jpg", "two").await;
        put(&h, "zebra.txt", "three").await;

        let first =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .delimiter("/")
                .max_keys(1)
                .send()
                .await
                .unwrap();

        assert_eq!(first.is_truncated(), Some(true));
        assert_eq!(first.common_prefixes()[0].prefix(), Some("photos/"));
        assert_eq!(first.next_key_marker(), Some("photos/"));

        let second =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .delimiter("/")
                .max_keys(1)
                .set_key_marker(first.next_key_marker.clone())
                .set_version_id_marker(first.next_version_id_marker.clone())
                .send()
                .await
                .unwrap();

        assert_eq!(second.is_truncated(), Some(false));
        assert_eq!(second.versions().len(), 1);
        assert_eq!(second.versions()[0].key(), Some("zebra.txt"));
    }
}

mod copy_source_parsing {
    use super::*;

    /// `?` is a legal character in an object key and the SDK sends it percent-encoded.
    /// Splitting the version off after decoding would read this key as `weird` at
    /// version `x` and copy the wrong thing, or nothing.
    #[tokio::test]
    async fn should_not_mistake_a_question_mark_in_a_key_for_a_version() {
        let h = versioned().await;
        put(&h, "weird?versionId=x", "the real object").await;

        h.s3.copy_object()
            .bucket(BUCKET)
            .key("copy.txt")
            .copy_source(format!("{BUCKET}/weird%3FversionId%3Dx"))
            .send()
            .await
            .unwrap();

        assert_eq!(body_of(&h, "copy.txt", None).await, "the real object");
    }
}

mod on_disk {
    use super::*;

    /// Disk is the default backend, so every version has to resolve to a real path.
    /// Memory would never notice a key that cannot be mirrored onto the filesystem.
    #[tokio::test]
    async fn should_keep_versions_on_the_disk_backend() {
        let h = conformance::harness_on_disk().await;
        h.s3.create_bucket().bucket(BUCKET).send().await.unwrap();
        enable(&h, BucketVersioningStatus::Enabled).await;

        // A nested key, and one ending in `/`: the two shapes that turn into something
        // other than a plain file on disk.
        for key in ["photos/2024/cat.jpg", "folder/"] {
            let first = put(&h, key, "one").await.unwrap();
            let second = put(&h, key, "two").await.unwrap();

            assert_eq!(body_of(&h, key, Some(&first)).await, "one");
            assert_eq!(body_of(&h, key, Some(&second)).await, "two");
            assert_eq!(body_of(&h, key, None).await, "two");
        }

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();
        assert_eq!(listed.versions().len(), 4);
    }

    /// A delete marker is stored as a zero-byte object, which is the shape most likely
    /// to be mishandled by a backend that mirrors keys onto files.
    #[tokio::test]
    async fn should_hide_a_deleted_key_on_the_disk_backend() {
        let h = conformance::harness_on_disk().await;
        h.s3.create_bucket().bucket(BUCKET).send().await.unwrap();
        enable(&h, BucketVersioningStatus::Enabled).await;
        let written = put(&h, "notes.txt", "one").await.unwrap();

        h.s3.delete_object()
            .bucket(BUCKET)
            .key("notes.txt")
            .send()
            .await
            .unwrap();

        assert_eq!(
            h.s3.get_object()
                .bucket(BUCKET)
                .key("notes.txt")
                .send()
                .await
                .unwrap_err()
                .into_service_error()
                .code(),
            Some("NoSuchKey")
        );
        assert_eq!(body_of(&h, "notes.txt", Some(&written)).await, "one");
    }
}

mod multipart {
    use super::*;

    #[tokio::test]
    async fn should_assign_a_version_to_a_completed_upload() {
        use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};

        let h = versioned().await;
        let created =
            h.s3.create_multipart_upload()
                .bucket(BUCKET)
                .key("big.bin")
                .send()
                .await
                .unwrap();
        let upload_id = created.upload_id().unwrap();

        let part =
            h.s3.upload_part()
                .bucket(BUCKET)
                .key("big.bin")
                .upload_id(upload_id)
                .part_number(1)
                .body(ByteStream::from_static(b"a single part is enough"))
                .send()
                .await
                .unwrap();

        let completed =
            h.s3.complete_multipart_upload()
                .bucket(BUCKET)
                .key("big.bin")
                .upload_id(upload_id)
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

        assert!(completed.version_id().is_some());

        let listed =
            h.s3.list_object_versions()
                .bucket(BUCKET)
                .send()
                .await
                .unwrap();
        assert_eq!(listed.versions().len(), 1);
        assert_eq!(listed.versions()[0].version_id(), completed.version_id());
    }
}
