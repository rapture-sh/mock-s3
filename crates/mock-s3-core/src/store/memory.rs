use std::collections::BTreeMap;

use bytes::Bytes;
use chrono::Utc;
use dashmap::DashMap;

use crate::error::{Error, Result};
use crate::model::{BucketInfo, ObjectData, ObjectMeta, PutOptions, compute_etag};
use crate::store::Store;

/// The `--in-memory` backend. Nothing survives the process.
#[derive(Default)]
pub struct MemoryStore {
    buckets: DashMap<String, Bucket>,
}

struct Bucket {
    created_at: chrono::DateTime<Utc>,
    objects: BTreeMap<String, (ObjectMeta, Bytes)>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl Store for MemoryStore {
    async fn create_bucket(&self, bucket: &str) -> Result<()> {
        if self.buckets.contains_key(bucket) {
            return Err(Error::BucketAlreadyOwnedByYou(bucket.to_string()));
        }
        self.buckets.insert(
            bucket.to_string(),
            Bucket {
                created_at: Utc::now(),
                objects: BTreeMap::new(),
            },
        );
        Ok(())
    }

    async fn delete_bucket(&self, bucket: &str) -> Result<()> {
        let entry = self
            .buckets
            .get(bucket)
            .ok_or_else(|| Error::NoSuchBucket(bucket.to_string()))?;
        if !entry.objects.is_empty() {
            return Err(Error::BucketNotEmpty(bucket.to_string()));
        }
        drop(entry);
        self.buckets.remove(bucket);
        Ok(())
    }

    async fn bucket_exists(&self, bucket: &str) -> Result<bool> {
        Ok(self.buckets.contains_key(bucket))
    }

    async fn list_buckets(&self) -> Result<Vec<BucketInfo>> {
        // Collected into a map so the result is sorted by name, as S3 returns it.
        let mut sorted: BTreeMap<String, chrono::DateTime<Utc>> = BTreeMap::new();
        for entry in self.buckets.iter() {
            sorted.insert(entry.key().clone(), entry.value().created_at);
        }
        Ok(sorted
            .into_iter()
            .map(|(name, created_at)| BucketInfo { name, created_at })
            .collect())
    }

    async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        body: Bytes,
        options: PutOptions,
    ) -> Result<ObjectMeta> {
        let mut entry = self
            .buckets
            .get_mut(bucket)
            .ok_or_else(|| Error::NoSuchBucket(bucket.to_string()))?;

        let meta = ObjectMeta {
            key: key.to_string(),
            size: body.len() as u64,
            // A multipart object's etag is a digest of its parts, not of the
            // assembled bytes, so the caller can supply it.
            etag: options.etag.unwrap_or_else(|| compute_etag(&body)),
            last_modified: Utc::now(),
            content_type: options.content_type,
            user_metadata: options.user_metadata,
            version_id: None,
            system_headers: options.system_headers,
        };
        entry.objects.insert(key.to_string(), (meta.clone(), body));
        Ok(meta)
    }

    async fn get_object(&self, bucket: &str, key: &str) -> Result<ObjectData> {
        let entry = self
            .buckets
            .get(bucket)
            .ok_or_else(|| Error::NoSuchBucket(bucket.to_string()))?;
        let (meta, body) = entry
            .objects
            .get(key)
            .ok_or_else(|| Error::NoSuchKey(key.to_string()))?;
        Ok(ObjectData {
            meta: meta.clone(),
            body: body.clone(),
        })
    }

    async fn head_object(&self, bucket: &str, key: &str) -> Result<ObjectMeta> {
        let entry = self
            .buckets
            .get(bucket)
            .ok_or_else(|| Error::NoSuchBucket(bucket.to_string()))?;
        entry
            .objects
            .get(key)
            .map(|(meta, _)| meta.clone())
            .ok_or_else(|| Error::NoSuchKey(key.to_string()))
    }

    async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        let mut entry = self
            .buckets
            .get_mut(bucket)
            .ok_or_else(|| Error::NoSuchBucket(bucket.to_string()))?;
        // S3 does not fail a delete of a key that is not there.
        entry.objects.remove(key);
        Ok(())
    }

    async fn list_objects(&self, bucket: &str) -> Result<Vec<ObjectMeta>> {
        let entry = self
            .buckets
            .get(bucket)
            .ok_or_else(|| Error::NoSuchBucket(bucket.to_string()))?;
        Ok(entry
            .objects
            .values()
            .map(|(meta, _)| meta.clone())
            .collect())
    }

    async fn reset(&self) -> Result<()> {
        self.buckets.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store_with_bucket() -> MemoryStore {
        let store = MemoryStore::new();
        store.create_bucket("test").await.unwrap();
        store
    }

    #[tokio::test]
    async fn should_round_trip_an_object() {
        let store = store_with_bucket().await;

        store
            .put_object(
                "test",
                "a.txt",
                Bytes::from_static(b"hello"),
                PutOptions::with_content_type("text/plain"),
            )
            .await
            .unwrap();
        let got = store.get_object("test", "a.txt").await.unwrap();

        assert_eq!(&got.body[..], b"hello");
        assert_eq!(got.meta.content_type, "text/plain");
        assert_eq!(got.meta.etag, compute_etag(b"hello"));
    }

    #[tokio::test]
    async fn should_report_no_such_key_for_a_missing_object() {
        let store = store_with_bucket().await;

        let error = store.get_object("test", "missing").await.unwrap_err();

        assert!(matches!(error, Error::NoSuchKey(_)));
    }

    #[tokio::test]
    async fn should_report_no_such_bucket_when_putting_into_an_unknown_bucket() {
        let store = MemoryStore::new();

        let error = store
            .put_object("nope", "a", Bytes::new(), PutOptions::default())
            .await
            .unwrap_err();

        assert!(matches!(error, Error::NoSuchBucket(_)));
    }

    #[tokio::test]
    async fn should_refuse_to_delete_a_bucket_that_still_holds_objects() {
        let store = store_with_bucket().await;
        store
            .put_object("test", "a", Bytes::new(), PutOptions::default())
            .await
            .unwrap();

        let error = store.delete_bucket("test").await.unwrap_err();

        assert!(matches!(error, Error::BucketNotEmpty(_)));
    }

    #[tokio::test]
    async fn should_return_keys_in_sorted_order() {
        let store = store_with_bucket().await;
        for key in ["c", "a", "b"] {
            store
                .put_object("test", key, Bytes::new(), PutOptions::default())
                .await
                .unwrap();
        }

        let keys: Vec<String> = store
            .list_objects("test")
            .await
            .unwrap()
            .into_iter()
            .map(|meta| meta.key)
            .collect();

        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn should_succeed_when_deleting_a_key_that_does_not_exist() {
        let store = store_with_bucket().await;

        assert!(store.delete_object("test", "missing").await.is_ok());
    }
}
