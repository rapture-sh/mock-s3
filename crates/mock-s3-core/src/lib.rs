//! The mock-s3 storage engine.
//!
//! This crate owns buckets, objects, listings and the storage backends. It knows nothing
//! about HTTP or XML: those live in `mock-s3-proto`, so the logic that is actually
//! fiddly — listing pagination, key-to-path resolution — can be tested without binding
//! a port.

use std::sync::Arc;

use bytes::Bytes;

pub mod bucket_config;
pub mod error;
pub mod listing;
pub mod model;
pub mod multipart;
pub mod store;
pub mod versioning;

pub use error::{Error, Result};
pub use listing::{ListRequest, ListResult, VersionListRequest, VersionListResult};
pub use model::{BucketInfo, ObjectData, ObjectMeta, PutOptions};
pub use store::{DiskStore, MemoryStore, Store};
pub use versioning::{DeleteOutcome, ObjectVersion, VersioningStatus};

/// Keys under this prefix are mock-s3's own bookkeeping — in-flight multipart parts,
/// object versions and bucket configuration — and never appear in a listing.
pub const HIDDEN_PREFIX: &str = ".mock-s3/";

/// The engine every request goes through, whichever listener it arrived on.
pub struct Engine {
    /// Visible to the crate so the multipart module can build on the same primitives
    /// rather than growing the `Store` trait.
    pub(crate) store: Arc<dyn Store>,
    region: String,
}

impl Engine {
    pub fn new(store: Arc<dyn Store>, region: impl Into<String>) -> Self {
        Self {
            store,
            region: region.into(),
        }
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn create_bucket(&self, bucket: &str) -> Result<()> {
        model::validate_bucket_name(bucket)?;
        self.store.create_bucket(bucket).await
    }

    /// Delete a bucket, which S3 only allows once it holds no objects.
    ///
    /// "No objects" means none a client can reach. A version or a delete marker counts,
    /// because both are still there to be listed and read — emptying a versioned bucket
    /// means deleting every version, which is why a client enumerates `?versions` before
    /// it tries this. Two things deliberately do not count:
    ///
    /// - mock-s3's own bookkeeping under the hidden prefix, because a client staring at
    ///   an empty listing and an unexplainable `BucketNotEmpty` has no way forward.
    /// - an upload still in flight. S3's own documentation lists only objects, and the
    ///   compatibility suite's cleanup deletes objects and then the bucket with no abort
    ///   step — so blocking here would break every client that follows the same order.
    ///   The upload is discarded with the bucket.
    pub async fn delete_bucket(&self, bucket: &str) -> Result<()> {
        let stored = self.store.list_objects(bucket).await?;

        if stored
            .iter()
            .any(|meta| versioning::is_client_visible(&meta.key))
        {
            return Err(Error::BucketNotEmpty(bucket.to_string()));
        }

        for meta in stored {
            self.store.delete_object(bucket, &meta.key).await?;
        }

        self.store.delete_bucket(bucket).await
    }

    pub async fn head_bucket(&self, bucket: &str) -> Result<()> {
        if self.store.bucket_exists(bucket).await? {
            Ok(())
        } else {
            Err(Error::NoSuchBucket(bucket.to_string()))
        }
    }

    pub async fn list_buckets(&self) -> Result<Vec<BucketInfo>> {
        self.store.list_buckets().await
    }

    /// Store an object, which in a versioned bucket means storing a new version of it.
    pub async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        body: Bytes,
        options: PutOptions,
    ) -> Result<ObjectMeta> {
        model::validate_key(key)?;
        self.put_object_version(bucket, key, body, options).await
    }

    /// Read the current version of a key. [`Engine::get_object_version`] reads an older
    /// one.
    pub async fn get_object(&self, bucket: &str, key: &str) -> Result<ObjectData> {
        self.get_object_version(bucket, key, None).await
    }

    pub async fn head_object(&self, bucket: &str, key: &str) -> Result<ObjectMeta> {
        self.head_object_version(bucket, key, None).await
    }

    /// Delete the current version of a key, which in a bucket with versioning enabled
    /// leaves a delete marker rather than removing anything.
    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<DeleteOutcome> {
        self.delete_object_version(bucket, key, None).await
    }

    /// List one page of a bucket. The pagination itself is in [`listing::list`].
    pub async fn list_objects(&self, bucket: &str, request: &ListRequest) -> Result<ListResult> {
        let all = self.store.list_objects(bucket).await?;

        // A listing shows one entry per key: in a versioned bucket that is the current
        // version, and a key whose current version is a delete marker does not appear at
        // all. Multipart parts and bucket configuration are stored as objects under a
        // hidden prefix, and a client must never see one either.
        let visible: Vec<ObjectMeta> = if self.versioning_status(bucket).await?.is_configured() {
            versioning::collect_versions(all)
                .into_iter()
                .filter(|version| version.is_latest)
                .filter_map(|version| version.meta)
                .collect()
        } else {
            all.into_iter()
                .filter(|meta| !bucket_config::is_hidden(&meta.key))
                .collect()
        };

        Ok(listing::list(visible, request))
    }

    /// List one page of every version in a bucket, which is `ListObjectVersions`.
    pub async fn list_object_versions(
        &self,
        bucket: &str,
        request: &VersionListRequest,
    ) -> Result<VersionListResult> {
        Ok(listing::list_versions(
            self.all_versions(bucket).await?,
            request,
        ))
    }

    /// Drop every bucket. The admin API exposes this for use between test runs.
    pub async fn reset(&self) -> Result<()> {
        self.store.reset().await
    }

    /// Empty one bucket, leaving the bucket itself in place. Cheaper for a test suite
    /// than recreating the bucket, and it keeps other tests' buckets untouched.
    pub async fn reset_bucket(&self, bucket: &str) -> Result<usize> {
        let all = self.store.list_objects(bucket).await?;
        let count = all.len();
        for meta in all {
            self.store.delete_object(bucket, &meta.key).await?;
        }
        Ok(count)
    }
}
