//! Multipart uploads.
//!
//! SDKs switch to multipart above a size threshold with no change in caller code, so
//! this is not optional. It is built on top of the ordinary object primitives rather
//! than new `Store` methods: a part is an object under a reserved key prefix, and
//! completing an upload concatenates those objects into the real one. Both backends get
//! multipart for free that way, and the parts are visible in the data volume like
//! everything else.
//!
//! The reserved prefix is hidden from listings — see [`Engine::list_objects`] — so a
//! client never sees an in-flight upload as an object.
//!
//! One limitation worth stating: completing an upload assembles the object in memory.
//! That is fine for the file sizes a test suite uses and wrong for a 5 GB upload, which
//! is not what a mock is for.

use std::sync::atomic::{AtomicU64, Ordering};

use bytes::{BufMut, BytesMut};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{ObjectMeta, PutOptions, multipart_etag};
use crate::{Engine, ListRequest};

/// Where in-flight uploads live. Hidden from listings.
pub const UPLOAD_PREFIX: &str = ".mock-s3/uploads/";

/// S3 requires every part but the last to be at least 5 MiB.
pub const MIN_PART_SIZE: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PartInfo {
    pub number: u32,
    pub size: u64,
    pub etag: String,
    pub last_modified: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UploadInfo {
    pub upload_id: String,
    pub key: String,
    pub initiated: DateTime<Utc>,
}

/// What a `CreateMultipartUpload` recorded, so the finished object gets the content type
/// and metadata that were set when the upload began rather than when it ended.
#[derive(Serialize, Deserialize)]
struct Manifest {
    key: String,
    initiated: DateTime<Utc>,
    options: PutOptions,
}

fn new_upload_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or_default();
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("{nanos:016x}{sequence:08x}")
}

fn upload_root(upload_id: &str) -> String {
    format!("{UPLOAD_PREFIX}{upload_id}/")
}

fn manifest_key(upload_id: &str) -> String {
    format!("{}manifest.json", upload_root(upload_id))
}

/// What a finished upload produced.
///
/// Kept after the parts are gone so that completing the same upload twice returns the
/// same answer. `CompleteMultipartUpload` can time out on a client that then retries it,
/// and S3 answers the retry rather than reporting the upload missing — a mock that
/// 404s instead turns a successful upload into a failed one.
fn completed_key(upload_id: &str) -> String {
    format!("{}completed.json", upload_root(upload_id))
}

/// Parts are named so that a plain sorted listing is already in part order.
fn part_key(upload_id: &str, number: u32) -> String {
    format!("{}part-{number:05}", upload_root(upload_id))
}

fn part_number_from_key(key: &str) -> Option<u32> {
    key.rsplit_once("part-")
        .and_then(|(_, number)| number.parse().ok())
}

impl Engine {
    pub async fn create_upload(
        &self,
        bucket: &str,
        key: &str,
        options: PutOptions,
    ) -> Result<String> {
        crate::model::validate_key(key)?;
        self.head_bucket(bucket).await?;

        let upload_id = new_upload_id();
        let manifest = Manifest {
            key: key.to_string(),
            initiated: Utc::now(),
            options,
        };
        let encoded = serde_json::to_vec(&manifest)
            .map_err(|error| Error::InvalidArgument(error.to_string()))?;

        self.store
            .put_object(
                bucket,
                &manifest_key(&upload_id),
                encoded.into(),
                PutOptions::with_content_type("application/json"),
            )
            .await?;

        Ok(upload_id)
    }

    pub async fn upload_part(
        &self,
        bucket: &str,
        upload_id: &str,
        number: u32,
        body: bytes::Bytes,
    ) -> Result<PartInfo> {
        if !(1..=10_000).contains(&number) {
            return Err(Error::InvalidPart(format!(
                "part number {number} is outside the range 1 to 10000"
            )));
        }
        self.load_manifest(bucket, upload_id).await?;

        let meta = self
            .store
            .put_object(
                bucket,
                &part_key(upload_id, number),
                body,
                PutOptions::default(),
            )
            .await?;

        Ok(PartInfo {
            number,
            size: meta.size,
            etag: meta.etag,
            last_modified: meta.last_modified,
        })
    }

    pub async fn list_parts(&self, bucket: &str, upload_id: &str) -> Result<Vec<PartInfo>> {
        self.load_manifest(bucket, upload_id).await?;

        let request = ListRequest {
            prefix: upload_root(upload_id),
            ..Default::default()
        };
        let listing = crate::listing::list(self.store.list_objects(bucket).await?, &request);

        let mut parts: Vec<PartInfo> = listing
            .objects
            .into_iter()
            .filter_map(|meta| {
                let number = part_number_from_key(&meta.key)?;
                Some(PartInfo {
                    number,
                    size: meta.size,
                    etag: meta.etag,
                    last_modified: meta.last_modified,
                })
            })
            .collect();
        parts.sort_by_key(|part| part.number);
        Ok(parts)
    }

    pub async fn list_uploads(&self, bucket: &str) -> Result<Vec<UploadInfo>> {
        self.head_bucket(bucket).await?;

        let request = ListRequest {
            prefix: UPLOAD_PREFIX.to_string(),
            ..Default::default()
        };
        let listing = crate::listing::list(self.store.list_objects(bucket).await?, &request);

        let mut uploads = Vec::new();
        for meta in listing.objects {
            if !meta.key.ends_with("manifest.json") {
                continue;
            }
            let upload_id = meta
                .key
                .trim_start_matches(UPLOAD_PREFIX)
                .trim_end_matches("/manifest.json");

            if let Ok(manifest) = self.load_manifest(bucket, upload_id).await {
                uploads.push(UploadInfo {
                    upload_id: upload_id.to_string(),
                    key: manifest.key,
                    initiated: manifest.initiated,
                });
            }
        }
        Ok(uploads)
    }

    pub async fn abort_upload(&self, bucket: &str, upload_id: &str) -> Result<()> {
        self.load_manifest(bucket, upload_id).await?;
        self.discard_upload(bucket, upload_id).await
    }

    /// Assemble the parts the client listed into the real object.
    ///
    /// `parts` is `(part number, etag)` in the order the client gave them, which S3
    /// requires to be ascending.
    pub async fn complete_upload(
        &self,
        bucket: &str,
        upload_id: &str,
        parts: &[(u32, String)],
    ) -> Result<ObjectMeta> {
        // A retry of a completed upload gets the original answer.
        if let Some(already) = self.load_completed(bucket, upload_id).await? {
            return Ok(already);
        }

        let manifest = self.load_manifest(bucket, upload_id).await?;

        if parts.is_empty() {
            return Err(Error::InvalidPart(
                "a completed upload must list at least one part".to_string(),
            ));
        }
        if parts.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(Error::InvalidPartOrder);
        }

        let available = self.list_parts(bucket, upload_id).await?;
        let mut assembled = BytesMut::new();
        let mut etags: Vec<String> = Vec::with_capacity(parts.len());

        for (index, (number, requested_etag)) in parts.iter().enumerate() {
            let part = available
                .iter()
                .find(|candidate| candidate.number == *number)
                .ok_or_else(|| Error::InvalidPart(format!("part {number} was never uploaded")))?;

            // Clients send the etag quoted, straight out of the UploadPart response.
            let requested = requested_etag.trim().trim_matches('"');
            if !requested.is_empty() && requested != part.etag {
                return Err(Error::InvalidPart(format!(
                    "part {number} has etag {} but {requested} was listed",
                    part.etag
                )));
            }

            let is_last = index + 1 == parts.len();
            if !is_last && part.size < MIN_PART_SIZE {
                return Err(Error::EntityTooSmall {
                    number: *number,
                    size: part.size,
                });
            }

            let data = self
                .store
                .get_object(bucket, &part_key(upload_id, *number))
                .await?;
            assembled.put(data.body);
            etags.push(part.etag.clone());
        }

        let options = PutOptions {
            etag: Some(multipart_etag(&etags)),
            ..manifest.options
        };
        let meta = self
            .store
            .put_object(bucket, &manifest.key, assembled.freeze(), options)
            .await?;

        // Parts and the manifest go first; the record of what was produced is written
        // afterwards, so the cleanup does not sweep it away.
        self.discard_upload(bucket, upload_id).await?;
        self.record_completed(bucket, upload_id, &meta).await?;

        Ok(meta)
    }

    async fn load_completed(&self, bucket: &str, upload_id: &str) -> Result<Option<ObjectMeta>> {
        match self
            .store
            .get_object(bucket, &completed_key(upload_id))
            .await
        {
            Ok(data) => Ok(serde_json::from_slice(&data.body).ok()),
            Err(Error::NoSuchKey(_)) => Ok(None),
            Err(Error::NoSuchBucket(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    async fn record_completed(
        &self,
        bucket: &str,
        upload_id: &str,
        meta: &ObjectMeta,
    ) -> Result<()> {
        let encoded =
            serde_json::to_vec(meta).map_err(|error| Error::InvalidArgument(error.to_string()))?;

        self.store
            .put_object(
                bucket,
                &completed_key(upload_id),
                encoded.into(),
                PutOptions::with_content_type("application/json"),
            )
            .await?;
        Ok(())
    }

    /// The key an upload will land on, which `CompleteMultipartUpload` has to report.
    pub async fn upload_key(&self, bucket: &str, upload_id: &str) -> Result<String> {
        Ok(self.load_manifest(bucket, upload_id).await?.key)
    }

    async fn load_manifest(&self, bucket: &str, upload_id: &str) -> Result<Manifest> {
        let data = self
            .store
            .get_object(bucket, &manifest_key(upload_id))
            .await
            .map_err(|error| match error {
                Error::NoSuchKey(_) => Error::NoSuchUpload(upload_id.to_string()),
                other => other,
            })?;

        serde_json::from_slice(&data.body).map_err(|_| Error::NoSuchUpload(upload_id.to_string()))
    }

    /// Remove every object belonging to an upload.
    async fn discard_upload(&self, bucket: &str, upload_id: &str) -> Result<()> {
        let request = ListRequest {
            prefix: upload_root(upload_id),
            ..Default::default()
        };
        let listing = crate::listing::list(self.store.list_objects(bucket).await?, &request);

        for meta in listing.objects {
            self.store.delete_object(bucket, &meta.key).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod part_number_from_key {
        use super::*;

        #[test]
        fn should_read_the_number_from_a_part_key() {
            let key = part_key("abc", 7);

            assert_eq!(part_number_from_key(&key), Some(7));
        }

        #[test]
        fn should_return_nothing_for_a_manifest_key() {
            assert!(part_number_from_key(&manifest_key("abc")).is_none());
        }
    }

    mod new_upload_id {
        use super::*;

        #[test]
        fn should_be_unique_and_url_safe() {
            let first = new_upload_id();
            let second = new_upload_id();

            assert_ne!(first, second);
            assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    mod part_key {
        use super::*;

        #[test]
        fn should_sort_in_part_order_as_plain_text() {
            let mut keys = vec![part_key("a", 10), part_key("a", 2), part_key("a", 1)];
            keys.sort();

            assert_eq!(
                keys,
                vec![part_key("a", 1), part_key("a", 2), part_key("a", 10)]
            );
        }
    }
}
