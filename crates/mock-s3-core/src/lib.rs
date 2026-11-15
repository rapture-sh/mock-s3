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

pub use error::{Error, Result};
pub use listing::{ListRequest, ListResult};
pub use model::{BucketInfo, ObjectData, ObjectMeta, PutOptions};
pub use store::{DiskStore, MemoryStore, Store};

/// Keys under this prefix are mock-s3's own bookkeeping — in-flight multipart parts and
/// bucket configuration — and never appear in a listing.
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

    /// Delete a bucket, which S3 only allows once it is empty.
    ///
    /// "Empty" means empty of what a client can see, plus no upload still in flight —
    /// which is what S3 requires. mock-s3's own bookkeeping under the hidden prefix does
    /// not count and is swept away here: leaving it to block the delete would mean a
    /// client staring at an empty listing and an unexplainable `BucketNotEmpty`.
    pub async fn delete_bucket(&self, bucket: &str) -> Result<()> {
        if !self
            .list_objects(bucket, &ListRequest::default())
            .await?
            .objects
            .is_empty()
        {
            return Err(Error::BucketNotEmpty(bucket.to_string()));
        }
        if !self.list_uploads(bucket).await?.is_empty() {
            return Err(Error::BucketNotEmpty(bucket.to_string()));
        }

        for meta in self.store.list_objects(bucket).await? {
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

    pub async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        body: Bytes,
        options: PutOptions,
    ) -> Result<ObjectMeta> {
        model::validate_key(key)?;
        self.store.put_object(bucket, key, body, options).await
    }

    pub async fn get_object(&self, bucket: &str, key: &str) -> Result<ObjectData> {
        model::validate_key(key)?;
        self.store.get_object(bucket, key).await
    }

    pub async fn head_object(&self, bucket: &str, key: &str) -> Result<ObjectMeta> {
        model::validate_key(key)?;
        self.store.head_object(bucket, key).await
    }

    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        model::validate_key(key)?;
        self.store.delete_object(bucket, key).await
    }

    /// List one page of a bucket. The pagination itself is in [`listing::list`].
    pub async fn list_objects(&self, bucket: &str, request: &ListRequest) -> Result<ListResult> {
        let all = self.store.list_objects(bucket).await?;

        // Multipart parts and bucket configuration are stored as objects under a hidden
        // prefix. A client must never see one in a listing.
        let visible: Vec<ObjectMeta> = all
            .into_iter()
            .filter(|meta| !bucket_config::is_hidden(&meta.key))
            .collect();

        Ok(listing::list(visible, request))
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
