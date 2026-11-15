//! The mock-s3 storage engine.
//!
//! This crate owns buckets, objects, listings and the storage backends. It knows nothing
//! about HTTP or XML: those live in `mock-s3-proto`, so the logic that is actually
//! fiddly — listing pagination, key-to-path resolution — can be tested without binding
//! a port.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;

pub mod error;
pub mod listing;
pub mod model;
pub mod store;

pub use error::{Error, Result};
pub use listing::{ListRequest, ListResult};
pub use model::{BucketInfo, ObjectData, ObjectMeta};
pub use store::{DiskStore, MemoryStore, Store};

/// The engine every request goes through, whichever listener it arrived on.
pub struct Engine {
    store: Arc<dyn Store>,
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

    pub async fn delete_bucket(&self, bucket: &str) -> Result<()> {
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
        content_type: &str,
        user_metadata: BTreeMap<String, String>,
    ) -> Result<ObjectMeta> {
        model::validate_key(key)?;
        self.store
            .put_object(bucket, key, body, content_type, user_metadata)
            .await
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
        Ok(listing::list(all, request))
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
