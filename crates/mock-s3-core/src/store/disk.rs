use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{BucketInfo, ObjectData, ObjectMeta, PutOptions, compute_etag};
use crate::store::{Store, guess_content_type, key_to_relative_path, relative_path_to_key};

/// The default backend, writing into the mounted data volume.
///
/// The layout mirrors the key space so the volume can be browsed and seeded by hand:
///
/// ```text
/// <root>/buckets/<bucket>/bucket.json
/// <root>/buckets/<bucket>/objects/<key>        the payload, at the key's own path
/// <root>/buckets/<bucket>/meta/<key>.json      content type, user metadata, etag
/// ```
///
/// A payload dropped into `objects/` without a sidecar is still served: the metadata is
/// synthesised from the file itself, which is what makes seeding a volume work.
pub struct DiskStore {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct BucketRecord {
    name: String,
    created_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct MetaRecord {
    content_type: String,
    etag: String,
    size: u64,
    last_modified: DateTime<Utc>,
    #[serde(default)]
    user_metadata: BTreeMap<String, String>,
    #[serde(default)]
    system_headers: BTreeMap<String, String>,
}

impl DiskStore {
    /// Create the data directory if it is not there yet.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        tokio::fs::create_dir_all(root.join("buckets")).await?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn buckets_dir(&self) -> PathBuf {
        self.root.join("buckets")
    }

    fn bucket_dir(&self, bucket: &str) -> PathBuf {
        self.buckets_dir().join(bucket)
    }

    fn objects_dir(&self, bucket: &str) -> PathBuf {
        self.bucket_dir(bucket).join("objects")
    }

    fn meta_dir(&self, bucket: &str) -> PathBuf {
        self.bucket_dir(bucket).join("meta")
    }

    fn object_path(&self, bucket: &str, key: &str) -> Result<PathBuf> {
        Ok(self.objects_dir(bucket).join(key_to_relative_path(key)?))
    }

    fn meta_path(&self, bucket: &str, key: &str) -> Result<PathBuf> {
        let relative = key_to_relative_path(key)?;
        let mut path = self.meta_dir(bucket).join(&relative);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        path.set_file_name(format!("{name}.json"));
        Ok(path)
    }

    async fn require_bucket(&self, bucket: &str) -> Result<()> {
        if self.bucket_exists(bucket).await? {
            Ok(())
        } else {
            Err(Error::NoSuchBucket(bucket.to_string()))
        }
    }

    /// Create the directory chain for a key, refusing rather than clobbering when an
    /// existing object already occupies part of the path.
    async fn ensure_chain(&self, target: &Path, key: &str) -> Result<()> {
        if let Some(parent) = target.parent() {
            let mut walked = PathBuf::new();
            for component in parent.components() {
                walked.push(component);
                match tokio::fs::metadata(&walked).await {
                    Ok(meta) if meta.is_file() => {
                        return Err(Error::KeyPathConflict(format!(
                            "{key}: an existing object occupies {}",
                            walked.display()
                        )));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            tokio::fs::create_dir_all(parent).await?;
        }
        if let Ok(meta) = tokio::fs::metadata(target).await
            && meta.is_dir()
        {
            return Err(Error::KeyPathConflict(format!(
                "{key}: a directory already occupies {}",
                target.display()
            )));
        }
        Ok(())
    }

    /// Read the sidecar, or build metadata from the payload when there is none.
    async fn load_meta(&self, bucket: &str, key: &str) -> Result<ObjectMeta> {
        let object_path = self.object_path(bucket, key)?;
        let file_meta = match tokio::fs::metadata(&object_path).await {
            Ok(meta) if meta.is_file() => meta,
            _ => return Err(Error::NoSuchKey(key.to_string())),
        };

        let meta_path = self.meta_path(bucket, key)?;
        if let Ok(raw) = tokio::fs::read(&meta_path).await {
            if let Ok(record) = serde_json::from_slice::<MetaRecord>(&raw) {
                return Ok(ObjectMeta {
                    key: key.to_string(),
                    size: record.size,
                    etag: record.etag,
                    last_modified: record.last_modified,
                    content_type: record.content_type,
                    user_metadata: record.user_metadata,
                    version_id: None,
                    system_headers: record.system_headers,
                });
            }
            tracing::warn!(key, "sidecar metadata is unreadable, synthesising it");
        }

        let body = tokio::fs::read(&object_path).await?;
        Ok(ObjectMeta {
            key: key.to_string(),
            size: file_meta.len(),
            etag: compute_etag(&body),
            last_modified: file_meta
                .modified()
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now()),
            content_type: guess_content_type(key).to_string(),
            user_metadata: BTreeMap::new(),
            version_id: None,
            system_headers: BTreeMap::new(),
        })
    }

    /// Remove directories left empty by a delete, so a browsed volume does not fill up
    /// with the skeletons of keys that are gone.
    async fn prune_upwards(&self, mut dir: PathBuf, stop_at: &Path) {
        while dir.starts_with(stop_at) && dir != stop_at {
            match tokio::fs::read_dir(&dir).await {
                Ok(mut entries) => match entries.next_entry().await {
                    Ok(None) => {
                        if tokio::fs::remove_dir(&dir).await.is_err() {
                            return;
                        }
                    }
                    _ => return,
                },
                Err(_) => return,
            }
            match dir.parent() {
                Some(parent) => dir = parent.to_path_buf(),
                None => return,
            }
        }
    }
}

#[async_trait::async_trait]
impl Store for DiskStore {
    async fn create_bucket(&self, bucket: &str) -> Result<()> {
        if self.bucket_exists(bucket).await? {
            return Err(Error::BucketAlreadyOwnedByYou(bucket.to_string()));
        }
        tokio::fs::create_dir_all(self.objects_dir(bucket)).await?;
        tokio::fs::create_dir_all(self.meta_dir(bucket)).await?;

        let record = BucketRecord {
            name: bucket.to_string(),
            created_at: Utc::now(),
        };
        let encoded = serde_json::to_vec_pretty(&record)
            .map_err(|error| Error::InvalidArgument(error.to_string()))?;
        tokio::fs::write(self.bucket_dir(bucket).join("bucket.json"), encoded).await?;
        Ok(())
    }

    async fn delete_bucket(&self, bucket: &str) -> Result<()> {
        self.require_bucket(bucket).await?;
        if !self.list_objects(bucket).await?.is_empty() {
            return Err(Error::BucketNotEmpty(bucket.to_string()));
        }
        tokio::fs::remove_dir_all(self.bucket_dir(bucket)).await?;
        Ok(())
    }

    async fn bucket_exists(&self, bucket: &str) -> Result<bool> {
        match tokio::fs::metadata(self.bucket_dir(bucket)).await {
            Ok(meta) => Ok(meta.is_dir()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    async fn list_buckets(&self) -> Result<Vec<BucketInfo>> {
        let mut found: Vec<BucketInfo> = Vec::new();
        let mut entries = match tokio::fs::read_dir(self.buckets_dir()).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(found),
            Err(error) => return Err(error.into()),
        };

        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let created_at = match tokio::fs::read(entry.path().join("bucket.json")).await {
                Ok(raw) => serde_json::from_slice::<BucketRecord>(&raw)
                    .map(|record| record.created_at)
                    .unwrap_or_else(|_| Utc::now()),
                Err(_) => entry
                    .metadata()
                    .await
                    .and_then(|meta| meta.modified())
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_else(|_| Utc::now()),
            };
            found.push(BucketInfo { name, created_at });
        }

        found.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(found)
    }

    async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        body: Bytes,
        options: PutOptions,
    ) -> Result<ObjectMeta> {
        self.require_bucket(bucket).await?;

        let object_path = self.object_path(bucket, key)?;
        let meta_path = self.meta_path(bucket, key)?;
        self.ensure_chain(&object_path, key).await?;
        self.ensure_chain(&meta_path, key).await?;

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

        tokio::fs::write(&object_path, &body).await?;

        let record = MetaRecord {
            content_type: meta.content_type.clone(),
            etag: meta.etag.clone(),
            size: meta.size,
            last_modified: meta.last_modified,
            user_metadata: meta.user_metadata.clone(),
            system_headers: meta.system_headers.clone(),
        };
        let encoded = serde_json::to_vec_pretty(&record)
            .map_err(|error| Error::InvalidArgument(error.to_string()))?;
        tokio::fs::write(&meta_path, encoded).await?;

        Ok(meta)
    }

    async fn get_object(&self, bucket: &str, key: &str) -> Result<ObjectData> {
        self.require_bucket(bucket).await?;
        let meta = self.load_meta(bucket, key).await?;
        let body = tokio::fs::read(self.object_path(bucket, key)?).await?;
        Ok(ObjectData {
            meta,
            body: Bytes::from(body),
        })
    }

    async fn head_object(&self, bucket: &str, key: &str) -> Result<ObjectMeta> {
        self.require_bucket(bucket).await?;
        self.load_meta(bucket, key).await
    }

    async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        self.require_bucket(bucket).await?;

        let object_path = self.object_path(bucket, key)?;
        let meta_path = self.meta_path(bucket, key)?;

        for path in [&object_path, &meta_path] {
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }

        if let Some(parent) = object_path.parent() {
            self.prune_upwards(parent.to_path_buf(), &self.objects_dir(bucket))
                .await;
        }
        if let Some(parent) = meta_path.parent() {
            self.prune_upwards(parent.to_path_buf(), &self.meta_dir(bucket))
                .await;
        }
        Ok(())
    }

    async fn list_objects(&self, bucket: &str) -> Result<Vec<ObjectMeta>> {
        self.require_bucket(bucket).await?;

        let root = self.objects_dir(bucket);
        let mut keys: Vec<String> = Vec::new();
        let mut stack = vec![root.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            while let Some(entry) = entries.next_entry().await? {
                let file_type = entry.file_type().await?;
                let path = entry.path();
                if file_type.is_dir() {
                    stack.push(path);
                } else if file_type.is_file()
                    && let Ok(relative) = path.strip_prefix(&root)
                    && let Some(key) = relative_path_to_key(relative)
                {
                    keys.push(key);
                }
            }
        }

        keys.sort();

        let mut found = Vec::with_capacity(keys.len());
        for key in keys {
            match self.load_meta(bucket, &key).await {
                Ok(meta) => found.push(meta),
                Err(Error::NoSuchKey(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(found)
    }

    async fn reset(&self) -> Result<()> {
        match tokio::fs::remove_dir_all(self.buckets_dir()).await {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        tokio::fs::create_dir_all(self.buckets_dir()).await?;
        Ok(())
    }
}
