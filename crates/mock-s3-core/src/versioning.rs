//! Bucket versioning.
//!
//! S3 keeps every write to a versioned key and hands each one a version id, so a client
//! can read or delete an older write, and a delete leaves a *delete marker* behind
//! rather than removing anything.
//!
//! Two properties of real S3 shape how this is stored, and together they make versioning
//! additive rather than a change to the volume layout:
//!
//! - An object written before versioning was ever enabled has the version id `null`. So
//!   the plain `objects/<key>` path the rest of the engine already uses *is* the null
//!   version. Every bucket and every hand-seeded volume that predates versioning keeps
//!   working untouched, and reads back as a single `null` version.
//! - Suspending versioning discards nothing; it only means the next write replaces the
//!   null version again. That is exactly a write to `objects/<key>`.
//!
//! Everything else is an ordinary object under the hidden prefix, the same way multipart
//! parts and bucket configuration are stored, so neither backend grows a method and the
//! volume still shows what is in it:
//!
//! ```text
//! objects/<key>                       the null version
//! .mock-s3/versions/<version>/<key>   a non-null version
//! .mock-s3/markers/<version>/<key>    a delete marker, stored as zero bytes
//! ```
//!
//! The version id comes before the key so that a key which ends in `/`, or which is a
//! prefix of another key, still resolves to a path: the version id is one fixed segment,
//! and what follows is the key exactly as
//! [`key_to_relative_path`](crate::store::key_to_relative_path) resolves it anywhere
//! else.
//!
//! One invariant holds the design together: **a key has at most one null entry**, either
//! the object at `objects/<key>` or a delete marker with the id `null`, never both.
//! Writing one removes the other.

use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use chrono::{DateTime, Utc};

use crate::error::{Error, Result};
use crate::model::{self, ObjectData, ObjectMeta, PutOptions};
use crate::{Engine, bucket_config};

/// Non-null versions, one directory per version id.
pub const VERSION_PREFIX: &str = ".mock-s3/versions/";

/// Delete markers, one directory per version id, each holding a zero-byte object.
pub const MARKER_PREFIX: &str = ".mock-s3/markers/";

/// The versioning status document. Plain text rather than the XML a client sent, because
/// the engine has to act on it and knows nothing about XML.
const STATUS_KEY: &str = ".mock-s3/config/versioning";

/// The version id S3 reports for an object stored while the bucket was not versioned.
pub const NULL_VERSION_ID: &str = "null";

/// What versioning a bucket has. Absent configuration is [`Self::Unversioned`], which is
/// what a new bucket reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VersioningStatus {
    #[default]
    Unversioned,
    Enabled,
    Suspended,
}

impl VersioningStatus {
    /// The wire spelling, or an empty string for a bucket that was never configured —
    /// which is how S3 reports it, as a `VersioningConfiguration` with no `Status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unversioned => "",
            Self::Enabled => "Enabled",
            Self::Suspended => "Suspended",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text.trim() {
            "Enabled" => Self::Enabled,
            "Suspended" => Self::Suspended,
            _ => Self::Unversioned,
        }
    }

    /// Whether a write keeps the previous one instead of replacing it.
    pub fn keeps_versions(self) -> bool {
        self == Self::Enabled
    }

    /// Whether the bucket has ever been versioned. A suspended bucket still holds, and
    /// still serves, the versions it collected while it was enabled.
    pub fn is_configured(self) -> bool {
        self != Self::Unversioned
    }
}

/// One entry of a `ListObjectVersions` answer.
#[derive(Debug, Clone)]
pub struct ObjectVersion {
    pub key: String,
    pub version_id: String,
    /// Whether this is the version a request that names no version id would reach.
    pub is_latest: bool,
    pub last_modified: DateTime<Utc>,
    /// The stored object, or `None` when this entry is a delete marker. A marker has no
    /// payload, no size and no ETag, which is why this is an `Option` rather than
    /// metadata with empty fields.
    pub meta: Option<ObjectMeta>,
}

impl ObjectVersion {
    pub fn is_delete_marker(&self) -> bool {
        self.meta.is_none()
    }
}

/// What a delete did, which is more than a caller can infer from it having succeeded.
#[derive(Debug, Clone, Default)]
pub struct DeleteOutcome {
    /// The version removed, or the delete marker created. `None` in a bucket that was
    /// never versioned, where there are no version ids to report.
    pub version_id: Option<String>,
    /// A delete marker was created, or the entry removed was itself a delete marker.
    pub delete_marker: bool,
}

/// A fresh version id: microseconds since the epoch, then a counter, in hex.
///
/// Opaque to a client, as S3's are, but ordered — two writes in the same microsecond are
/// still told apart, so sorting versions newest-first is deterministic rather than
/// dependent on clock resolution.
fn next_version_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let micros = Utc::now().timestamp_micros().max(0) as u64;
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed) & 0xffff_ffff;
    format!("{micros:016x}{sequence:08x}")
}

fn version_storage_key(version_id: &str, key: &str) -> String {
    format!("{VERSION_PREFIX}{version_id}/{key}")
}

fn marker_storage_key(version_id: &str, key: &str) -> String {
    format!("{MARKER_PREFIX}{version_id}/{key}")
}

/// Split a hidden storage key back into the version id and the key a client asked for.
///
/// Returns `None` for anything not under `prefix`, which is how multipart parts and
/// configuration documents are passed over.
fn split_storage_key<'key>(storage_key: &'key str, prefix: &str) -> Option<(&'key str, &'key str)> {
    let rest = storage_key.strip_prefix(prefix)?;
    let (version_id, key) = rest.split_once('/')?;

    if version_id.is_empty() || key.is_empty() {
        return None;
    }
    Some((version_id, key))
}

/// Whether a storage key holds something a client can list or read: an object, one of
/// its versions, or a delete marker. Everything else under the hidden prefix — multipart
/// parts, configuration — is mock-s3's own bookkeeping.
pub(crate) fn is_client_visible(storage_key: &str) -> bool {
    split_storage_key(storage_key, VERSION_PREFIX).is_some()
        || split_storage_key(storage_key, MARKER_PREFIX).is_some()
        || !bucket_config::is_hidden(storage_key)
}

/// Turn a bucket's raw storage keys into version entries, by key and then newest first.
///
/// Pure, so the ordering and the `is_latest` rule can be tested without a backend.
pub(crate) fn collect_versions(stored: Vec<ObjectMeta>) -> Vec<ObjectVersion> {
    let mut versions: Vec<ObjectVersion> = Vec::new();

    for meta in stored {
        if let Some((version_id, key)) = split_storage_key(&meta.key, VERSION_PREFIX) {
            let (version_id, key) = (version_id.to_string(), key.to_string());
            versions.push(ObjectVersion {
                last_modified: meta.last_modified,
                meta: Some(ObjectMeta {
                    key: key.clone(),
                    version_id: Some(version_id.clone()),
                    ..meta
                }),
                key,
                version_id,
                is_latest: false,
            });
        } else if let Some((version_id, key)) = split_storage_key(&meta.key, MARKER_PREFIX) {
            versions.push(ObjectVersion {
                key: key.to_string(),
                version_id: version_id.to_string(),
                is_latest: false,
                last_modified: meta.last_modified,
                meta: None,
            });
        } else if !bucket_config::is_hidden(&meta.key) {
            versions.push(ObjectVersion {
                key: meta.key.clone(),
                version_id: NULL_VERSION_ID.to_string(),
                is_latest: false,
                last_modified: meta.last_modified,
                meta: Some(ObjectMeta {
                    version_id: Some(NULL_VERSION_ID.to_string()),
                    ..meta
                }),
            });
        }
    }

    // Key ascending, then newest first within the key: the order S3 lists versions in,
    // and the order that puts a key's current version first. The version id breaks a tie
    // because it carries the same ordering as the clock, only finer.
    versions.sort_by(|left, right| {
        left.key.cmp(&right.key).then_with(|| {
            right
                .last_modified
                .cmp(&left.last_modified)
                .then_with(|| right.version_id.cmp(&left.version_id))
        })
    });

    for index in 0..versions.len() {
        versions[index].is_latest = index == 0 || versions[index - 1].key != versions[index].key;
    }

    versions
}

/// Which stored object a request for a version resolves to.
enum Resolved {
    Object {
        storage_key: String,
        version_id: Option<String>,
    },
    /// The version asked for is a delete marker, so there is nothing to read.
    Marker {
        version_id: String,
        /// Whether it was reached as the key's current version rather than by naming its
        /// id. S3 answers the two differently: a 404 for the current version, a 405 for
        /// a marker named outright.
        current: bool,
    },
}

impl Engine {
    /// Set a bucket's versioning status. Only `Enabled` and `Suspended` can be set:
    /// there is no request that takes a bucket back to never having been versioned.
    pub async fn put_bucket_versioning(
        &self,
        bucket: &str,
        status: VersioningStatus,
    ) -> Result<()> {
        self.head_bucket(bucket).await?;

        if !status.is_configured() {
            return Err(Error::InvalidArgument(
                "the versioning status must be Enabled or Suspended".into(),
            ));
        }

        self.store
            .put_object(
                bucket,
                STATUS_KEY,
                Bytes::from_static(status.as_str().as_bytes()),
                PutOptions::with_content_type("text/plain"),
            )
            .await?;
        Ok(())
    }

    pub async fn bucket_versioning(&self, bucket: &str) -> Result<VersioningStatus> {
        self.head_bucket(bucket).await?;
        self.versioning_status(bucket).await
    }

    /// The status, without re-checking that the bucket exists.
    ///
    /// Every object operation consults this, and the store call that follows reports a
    /// missing bucket anyway, so a second existence check per request is wasted work.
    pub(crate) async fn versioning_status(&self, bucket: &str) -> Result<VersioningStatus> {
        match self.store.get_object(bucket, STATUS_KEY).await {
            Ok(data) => Ok(VersioningStatus::parse(
                std::str::from_utf8(&data.body).unwrap_or_default(),
            )),
            Err(Error::NoSuchKey(_)) => Ok(VersioningStatus::Unversioned),
            Err(other) => Err(other),
        }
    }

    /// Every version and delete marker in a bucket, by key and then newest first.
    pub(crate) async fn all_versions(&self, bucket: &str) -> Result<Vec<ObjectVersion>> {
        Ok(collect_versions(self.store.list_objects(bucket).await?))
    }

    /// Every version of one key, newest first.
    pub async fn object_versions(&self, bucket: &str, key: &str) -> Result<Vec<ObjectVersion>> {
        Ok(self
            .all_versions(bucket)
            .await?
            .into_iter()
            .filter(|version| version.key == key)
            .collect())
    }

    async fn resolve(&self, bucket: &str, key: &str, version_id: Option<&str>) -> Result<Resolved> {
        let plain = || Resolved::Object {
            storage_key: key.to_string(),
            version_id: None,
        };
        let absent = |version_id: &str| Error::NoSuchVersion {
            key: key.to_string(),
            version_id: version_id.to_string(),
        };

        let status = self.versioning_status(bucket).await?;

        // A bucket that was never versioned holds one object per key and has no history
        // to search, which keeps the common path exactly as cheap as it was.
        if !status.is_configured() {
            return match version_id {
                None | Some(NULL_VERSION_ID) => Ok(plain()),
                // The id cannot name anything here. S3 tells a malformed id (400) from
                // one that merely does not exist (404); mock-s3 cannot, and reporting
                // the version as absent is the more useful of the two.
                Some(other) => Err(absent(other)),
            };
        }

        match version_id {
            // Named outright: look the id up directly rather than reading the history.
            Some(wanted) => {
                let storage_key = if wanted == NULL_VERSION_ID {
                    key.to_string()
                } else {
                    version_storage_key(wanted, key)
                };
                if self.store.head_object(bucket, &storage_key).await.is_ok() {
                    return Ok(Resolved::Object {
                        storage_key,
                        version_id: Some(wanted.to_string()),
                    });
                }
                if self
                    .store
                    .head_object(bucket, &marker_storage_key(wanted, key))
                    .await
                    .is_ok()
                {
                    return Ok(Resolved::Marker {
                        version_id: wanted.to_string(),
                        current: false,
                    });
                }
                Err(absent(wanted))
            }

            // The current version is whichever is newest, which needs the key's history.
            None => {
                let Some(current) = self.object_versions(bucket, key).await?.into_iter().next()
                else {
                    return Err(Error::NoSuchKey(key.to_string()));
                };

                if current.is_delete_marker() {
                    return Ok(Resolved::Marker {
                        version_id: current.version_id,
                        current: true,
                    });
                }
                let storage_key = if current.version_id == NULL_VERSION_ID {
                    key.to_string()
                } else {
                    version_storage_key(&current.version_id, key)
                };
                Ok(Resolved::Object {
                    storage_key,
                    version_id: Some(current.version_id),
                })
            }
        }
    }

    /// Read one version, or the current one when `version_id` is `None`.
    pub async fn get_object_version(
        &self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<ObjectData> {
        model::validate_key(key)?;

        match self.resolve(bucket, key, version_id).await? {
            Resolved::Object {
                storage_key,
                version_id,
            } => {
                let mut data = self.store.get_object(bucket, &storage_key).await?;
                data.meta.key = key.to_string();
                data.meta.version_id = version_id;
                Ok(data)
            }
            Resolved::Marker {
                version_id,
                current,
            } => Err(marker_error(key, version_id, current)),
        }
    }

    /// Metadata for one version, or the current one when `version_id` is `None`.
    pub async fn head_object_version(
        &self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<ObjectMeta> {
        model::validate_key(key)?;

        match self.resolve(bucket, key, version_id).await? {
            Resolved::Object {
                storage_key,
                version_id,
            } => {
                let mut meta = self.store.head_object(bucket, &storage_key).await?;
                meta.key = key.to_string();
                meta.version_id = version_id;
                Ok(meta)
            }
            Resolved::Marker {
                version_id,
                current,
            } => Err(marker_error(key, version_id, current)),
        }
    }

    /// Delete one version, or — in a versioned bucket with no version id named — leave a
    /// delete marker behind instead of removing anything.
    pub async fn delete_object_version(
        &self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<DeleteOutcome> {
        model::validate_key(key)?;

        let status = self.versioning_status(bucket).await?;

        if let Some(wanted) = version_id {
            if !status.is_configured() && wanted != NULL_VERSION_ID {
                return Err(Error::NoSuchVersion {
                    key: key.to_string(),
                    version_id: wanted.to_string(),
                });
            }
            return self.delete_one_version(bucket, key, wanted).await;
        }

        match status {
            // Nothing to keep: the delete removes the object, and a key that is not
            // there is not an error, which is what S3 does.
            VersioningStatus::Unversioned => {
                self.store.delete_object(bucket, key).await?;
                Ok(DeleteOutcome::default())
            }

            // A delete marker becomes the new current version. The object stays readable
            // by version id, which is the whole point of versioning.
            VersioningStatus::Enabled => {
                let marker = next_version_id();
                self.write_marker(bucket, key, &marker).await?;
                Ok(DeleteOutcome {
                    version_id: Some(marker),
                    delete_marker: true,
                })
            }

            // S3 removes the null version and inserts a delete marker whose id is also
            // `null`. Removing the object first is what keeps one null entry per key
            // true, so a later read of `versionId=null` is never ambiguous.
            VersioningStatus::Suspended => {
                self.store.delete_object(bucket, key).await?;
                self.write_marker(bucket, key, NULL_VERSION_ID).await?;
                Ok(DeleteOutcome {
                    version_id: Some(NULL_VERSION_ID.to_string()),
                    delete_marker: true,
                })
            }
        }
    }

    /// Remove one version outright, delete markers included. This is the only delete that
    /// destroys data in a versioned bucket.
    async fn delete_one_version(
        &self,
        bucket: &str,
        key: &str,
        version_id: &str,
    ) -> Result<DeleteOutcome> {
        let found = |delete_marker: bool| {
            Ok(DeleteOutcome {
                version_id: Some(version_id.to_string()),
                delete_marker,
            })
        };

        let storage_key = if version_id == NULL_VERSION_ID {
            key.to_string()
        } else {
            version_storage_key(version_id, key)
        };
        if self.store.head_object(bucket, &storage_key).await.is_ok() {
            self.store.delete_object(bucket, &storage_key).await?;
            return found(false);
        }

        let marker = marker_storage_key(version_id, key);
        if self.store.head_object(bucket, &marker).await.is_ok() {
            self.store.delete_object(bucket, &marker).await?;
            return found(true);
        }

        Err(Error::NoSuchVersion {
            key: key.to_string(),
            version_id: version_id.to_string(),
        })
    }

    async fn write_marker(&self, bucket: &str, key: &str, version_id: &str) -> Result<()> {
        self.store
            .put_object(
                bucket,
                &marker_storage_key(version_id, key),
                Bytes::new(),
                PutOptions::default(),
            )
            .await?;
        Ok(())
    }

    /// Store a new version of a key, or replace the null version when the bucket is not
    /// collecting versions.
    pub(crate) async fn put_object_version(
        &self,
        bucket: &str,
        key: &str,
        body: Bytes,
        options: PutOptions,
    ) -> Result<ObjectMeta> {
        let status = self.versioning_status(bucket).await?;

        if !status.keeps_versions() {
            let meta = self.store.put_object(bucket, key, body, options).await?;

            // The new null version supersedes a null delete marker, and only one of the
            // two may exist at a time.
            if status.is_configured() {
                self.store
                    .delete_object(bucket, &marker_storage_key(NULL_VERSION_ID, key))
                    .await?;
            }
            return Ok(meta);
        }

        let version_id = next_version_id();
        let mut meta = self
            .store
            .put_object(
                bucket,
                &version_storage_key(&version_id, key),
                body,
                options,
            )
            .await?;

        meta.key = key.to_string();
        meta.version_id = Some(version_id);
        Ok(meta)
    }
}

/// The error a delete marker turns into, which depends on how it was reached.
fn marker_error(key: &str, version_id: String, current: bool) -> Error {
    if current {
        Error::DeleteMarker {
            key: key.to_string(),
            version_id,
        }
    } else {
        Error::DeleteMarkerVersion {
            key: key.to_string(),
            version_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::store::MemoryStore;

    /// An engine on the in-memory backend with one bucket, ready to write to.
    async fn engine(bucket: &str) -> Engine {
        let engine = Engine::new(Arc::new(MemoryStore::new()), "us-east-1");
        engine
            .create_bucket(bucket)
            .await
            .expect("the test bucket should be created");
        engine
    }

    async fn put(engine: &Engine, bucket: &str, key: &str, body: &'static str) -> Option<String> {
        engine
            .put_object(
                bucket,
                key,
                Bytes::from_static(body.as_bytes()),
                PutOptions::default(),
            )
            .await
            .expect("the put should succeed")
            .version_id
    }

    async fn body_of(engine: &Engine, bucket: &str, key: &str, version: Option<&str>) -> String {
        let data = engine
            .get_object_version(bucket, key, version)
            .await
            .expect("the version should be readable");
        String::from_utf8(data.body.to_vec()).expect("test payloads are text")
    }

    fn stored(key: &str, seconds: i64) -> ObjectMeta {
        ObjectMeta {
            key: key.to_string(),
            size: 3,
            etag: "abc".into(),
            last_modified: DateTime::from_timestamp(seconds, 0).expect("a valid timestamp"),
            content_type: "text/plain".into(),
            user_metadata: BTreeMap::new(),
            version_id: None,
            system_headers: BTreeMap::new(),
        }
    }

    mod versioning_status {
        use super::*;

        #[test]
        fn should_report_an_unconfigured_bucket_as_unversioned() {
            assert_eq!(VersioningStatus::parse(""), VersioningStatus::Unversioned);
            assert_eq!(
                VersioningStatus::parse("Nonsense"),
                VersioningStatus::Unversioned
            );
        }

        #[test]
        fn should_round_trip_each_status_it_can_be_set_to() {
            for status in [VersioningStatus::Enabled, VersioningStatus::Suspended] {
                assert_eq!(VersioningStatus::parse(status.as_str()), status);
            }
        }

        #[test]
        fn should_keep_versions_only_while_enabled() {
            assert!(VersioningStatus::Enabled.keeps_versions());
            assert!(!VersioningStatus::Suspended.keeps_versions());
            assert!(!VersioningStatus::Unversioned.keeps_versions());
        }

        /// A suspended bucket still answers for the versions it already holds, so it
        /// counts as configured even though new writes no longer add to them.
        #[test]
        fn should_treat_a_suspended_bucket_as_still_versioned() {
            assert!(VersioningStatus::Suspended.is_configured());
            assert!(!VersioningStatus::Unversioned.is_configured());
        }
    }

    mod next_version_id {
        use super::*;

        #[test]
        fn should_mint_an_id_that_can_be_a_single_path_segment() {
            let id = next_version_id();

            assert!(
                id.chars().all(|character| character.is_ascii_hexdigit()),
                "{id} has to survive being a directory name and a query value"
            );
            assert_ne!(id, NULL_VERSION_ID);
        }

        #[test]
        fn should_order_two_ids_by_when_they_were_minted() {
            let first = next_version_id();
            let second = next_version_id();

            assert!(first < second, "{first} should sort before {second}");
        }
    }

    mod split_storage_key {
        use super::*;

        #[test]
        fn should_split_a_version_back_into_its_id_and_key() {
            let storage_key = version_storage_key("v1", "photos/cat.jpg");

            assert_eq!(
                split_storage_key(&storage_key, VERSION_PREFIX),
                Some(("v1", "photos/cat.jpg"))
            );
        }

        /// A key ending in `/` is legal, and putting the version id first is what keeps
        /// it representable.
        #[test]
        fn should_split_a_key_that_ends_in_a_slash() {
            let storage_key = version_storage_key("v1", "photos/");

            assert_eq!(
                split_storage_key(&storage_key, VERSION_PREFIX),
                Some(("v1", "photos/"))
            );
        }

        #[test]
        fn should_pass_over_other_bookkeeping() {
            assert_eq!(
                split_storage_key(".mock-s3/uploads/abc/1", VERSION_PREFIX),
                None
            );
            assert_eq!(split_storage_key("photos/cat.jpg", VERSION_PREFIX), None);
        }

        #[test]
        fn should_reject_a_prefix_with_no_key_after_the_version() {
            assert_eq!(
                split_storage_key(".mock-s3/versions/v1/", VERSION_PREFIX),
                None
            );
            assert_eq!(
                split_storage_key(".mock-s3/versions/v1", VERSION_PREFIX),
                None
            );
        }
    }

    mod collect_versions {
        use super::*;

        #[test]
        fn should_report_a_plain_object_as_the_null_version() {
            let versions = collect_versions(vec![stored("notes.txt", 10)]);

            assert_eq!(versions.len(), 1);
            assert_eq!(versions[0].version_id, NULL_VERSION_ID);
            assert!(versions[0].is_latest);
            assert!(!versions[0].is_delete_marker());
        }

        #[test]
        fn should_order_versions_of_a_key_newest_first() {
            let versions = collect_versions(vec![
                stored(&version_storage_key("a", "notes.txt"), 10),
                stored(&version_storage_key("b", "notes.txt"), 30),
                stored(&version_storage_key("c", "notes.txt"), 20),
            ]);

            let ids: Vec<&str> = versions
                .iter()
                .map(|version| version.version_id.as_str())
                .collect();
            assert_eq!(ids, vec!["b", "c", "a"]);
            assert!(versions[0].is_latest);
            assert!(!versions[1].is_latest);
        }

        #[test]
        fn should_mark_the_latest_version_of_every_key() {
            let versions = collect_versions(vec![
                stored(&version_storage_key("a", "one.txt"), 10),
                stored(&version_storage_key("b", "one.txt"), 20),
                stored(&version_storage_key("c", "two.txt"), 10),
            ]);

            let latest: Vec<(&str, &str)> = versions
                .iter()
                .filter(|version| version.is_latest)
                .map(|version| (version.key.as_str(), version.version_id.as_str()))
                .collect();
            assert_eq!(latest, vec![("one.txt", "b"), ("two.txt", "c")]);
        }

        #[test]
        fn should_report_a_marker_as_a_delete_marker_with_no_payload() {
            let versions =
                collect_versions(vec![stored(&marker_storage_key("a", "notes.txt"), 10)]);

            assert_eq!(versions.len(), 1);
            assert!(versions[0].is_delete_marker());
            assert!(versions[0].meta.is_none());
        }

        /// A delete marker written after a version hides it, which is what makes the key
        /// look absent to a plain `GET`.
        #[test]
        fn should_let_a_marker_become_the_latest_version() {
            let versions = collect_versions(vec![
                stored(&version_storage_key("a", "notes.txt"), 10),
                stored(&marker_storage_key("b", "notes.txt"), 20),
            ]);

            assert!(versions[0].is_latest);
            assert!(versions[0].is_delete_marker());
        }

        #[test]
        fn should_report_a_version_under_the_key_a_client_asked_for() {
            let versions = collect_versions(vec![stored(
                &version_storage_key("a", "photos/cat.jpg"),
                10,
            )]);

            assert_eq!(versions[0].key, "photos/cat.jpg");
            let meta = versions[0].meta.as_ref().expect("a version has metadata");
            assert_eq!(meta.key, "photos/cat.jpg");
            assert_eq!(meta.version_id.as_deref(), Some("a"));
        }

        #[test]
        fn should_leave_out_other_bookkeeping() {
            let versions = collect_versions(vec![
                stored(".mock-s3/uploads/abc/manifest.json", 10),
                stored(".mock-s3/config/cors.xml", 10),
                stored("notes.txt", 10),
            ]);

            let keys: Vec<&str> = versions
                .iter()
                .map(|version| version.key.as_str())
                .collect();
            assert_eq!(keys, vec!["notes.txt"]);
        }

        #[test]
        fn should_order_an_older_null_version_after_a_newer_one() {
            let versions = collect_versions(vec![
                stored("notes.txt", 10),
                stored(&version_storage_key("a", "notes.txt"), 20),
            ]);

            assert_eq!(versions[0].version_id, "a");
            assert_eq!(versions[1].version_id, NULL_VERSION_ID);
        }

        /// Two writes inside the same clock tick still have an order, because the version
        /// id carries one.
        #[test]
        fn should_break_a_timestamp_tie_with_the_version_id() {
            let versions = collect_versions(vec![
                stored(&version_storage_key("aaa", "notes.txt"), 10),
                stored(&version_storage_key("bbb", "notes.txt"), 10),
            ]);

            assert_eq!(versions[0].version_id, "bbb");
            assert_eq!(versions[1].version_id, "aaa");
        }
    }

    mod put_object {
        use super::*;

        #[tokio::test]
        async fn should_not_assign_a_version_in_a_bucket_that_was_never_versioned() {
            let engine = engine("plain").await;

            assert_eq!(put(&engine, "plain", "notes.txt", "one").await, None);
        }

        #[tokio::test]
        async fn should_keep_the_previous_write_once_versioning_is_enabled() {
            let engine = engine("kept").await;
            engine
                .put_bucket_versioning("kept", VersioningStatus::Enabled)
                .await
                .unwrap();

            let first = put(&engine, "kept", "notes.txt", "one").await.unwrap();
            let second = put(&engine, "kept", "notes.txt", "two").await.unwrap();

            assert_ne!(first, second);
            assert_eq!(body_of(&engine, "kept", "notes.txt", None).await, "two");
            assert_eq!(
                body_of(&engine, "kept", "notes.txt", Some(&first)).await,
                "one"
            );
        }

        /// Objects stored before versioning was enabled are the `null` version, which is
        /// what makes enabling it on an existing bucket an additive change.
        #[tokio::test]
        async fn should_report_an_object_stored_before_versioning_as_the_null_version() {
            let engine = engine("existing").await;
            put(&engine, "existing", "notes.txt", "before").await;

            engine
                .put_bucket_versioning("existing", VersioningStatus::Enabled)
                .await
                .unwrap();

            let versions = engine
                .object_versions("existing", "notes.txt")
                .await
                .unwrap();
            assert_eq!(versions.len(), 1);
            assert_eq!(versions[0].version_id, NULL_VERSION_ID);
            assert_eq!(
                body_of(&engine, "existing", "notes.txt", Some(NULL_VERSION_ID)).await,
                "before"
            );
        }

        #[tokio::test]
        async fn should_replace_only_the_null_version_while_versioning_is_suspended() {
            let engine = engine("suspended").await;
            engine
                .put_bucket_versioning("suspended", VersioningStatus::Enabled)
                .await
                .unwrap();
            let kept = put(&engine, "suspended", "notes.txt", "one").await.unwrap();

            engine
                .put_bucket_versioning("suspended", VersioningStatus::Suspended)
                .await
                .unwrap();
            put(&engine, "suspended", "notes.txt", "two").await;
            put(&engine, "suspended", "notes.txt", "three").await;

            let ids: Vec<String> = engine
                .object_versions("suspended", "notes.txt")
                .await
                .unwrap()
                .into_iter()
                .map(|version| version.version_id)
                .collect();
            assert_eq!(ids, vec![NULL_VERSION_ID.to_string(), kept.clone()]);
            assert_eq!(
                body_of(&engine, "suspended", "notes.txt", None).await,
                "three"
            );
            assert_eq!(
                body_of(&engine, "suspended", "notes.txt", Some(&kept)).await,
                "one"
            );
        }
    }

    mod delete_object {
        use super::*;

        #[tokio::test]
        async fn should_leave_a_delete_marker_rather_than_remove_the_object() {
            let engine = engine("marked").await;
            engine
                .put_bucket_versioning("marked", VersioningStatus::Enabled)
                .await
                .unwrap();
            let written = put(&engine, "marked", "notes.txt", "one").await.unwrap();

            let outcome = engine.delete_object("marked", "notes.txt").await.unwrap();

            assert!(outcome.delete_marker);
            assert_ne!(outcome.version_id.as_deref(), Some(written.as_str()));
            assert_eq!(
                body_of(&engine, "marked", "notes.txt", Some(&written)).await,
                "one"
            );
        }

        #[tokio::test]
        async fn should_make_the_key_read_as_absent_once_a_marker_is_current() {
            let engine = engine("hidden").await;
            engine
                .put_bucket_versioning("hidden", VersioningStatus::Enabled)
                .await
                .unwrap();
            put(&engine, "hidden", "notes.txt", "one").await;
            engine.delete_object("hidden", "notes.txt").await.unwrap();

            let error = engine.get_object("hidden", "notes.txt").await.unwrap_err();

            assert!(
                matches!(error, Error::DeleteMarker { .. }),
                "expected a delete marker, got {error}"
            );
            assert!(
                engine
                    .list_objects("hidden", &crate::ListRequest::default())
                    .await
                    .unwrap()
                    .objects
                    .is_empty()
            );
        }

        /// A delete marker is only ever current until the next write, which is what makes
        /// a delete in a versioned bucket undoable.
        #[tokio::test]
        async fn should_let_a_later_write_supersede_a_delete_marker() {
            let engine = engine("rewritten").await;
            engine
                .put_bucket_versioning("rewritten", VersioningStatus::Enabled)
                .await
                .unwrap();
            put(&engine, "rewritten", "notes.txt", "one").await;
            engine
                .delete_object("rewritten", "notes.txt")
                .await
                .unwrap();

            put(&engine, "rewritten", "notes.txt", "two").await;

            assert_eq!(
                body_of(&engine, "rewritten", "notes.txt", None).await,
                "two"
            );
        }

        #[tokio::test]
        async fn should_remove_one_version_outright_when_its_id_is_named() {
            let engine = engine("pruned").await;
            engine
                .put_bucket_versioning("pruned", VersioningStatus::Enabled)
                .await
                .unwrap();
            let first = put(&engine, "pruned", "notes.txt", "one").await.unwrap();
            put(&engine, "pruned", "notes.txt", "two").await;

            let outcome = engine
                .delete_object_version("pruned", "notes.txt", Some(&first))
                .await
                .unwrap();

            assert!(!outcome.delete_marker);
            assert_eq!(outcome.version_id.as_deref(), Some(first.as_str()));
            assert_eq!(body_of(&engine, "pruned", "notes.txt", None).await, "two");
            assert!(matches!(
                engine
                    .get_object_version("pruned", "notes.txt", Some(&first))
                    .await
                    .unwrap_err(),
                Error::NoSuchVersion { .. }
            ));
        }

        /// Deleting the current delete marker is how a client undoes a delete, and S3
        /// reports that what it removed was a marker.
        #[tokio::test]
        async fn should_restore_the_object_when_its_delete_marker_is_removed() {
            let engine = engine("restored").await;
            engine
                .put_bucket_versioning("restored", VersioningStatus::Enabled)
                .await
                .unwrap();
            put(&engine, "restored", "notes.txt", "one").await;
            let marker = engine
                .delete_object("restored", "notes.txt")
                .await
                .unwrap()
                .version_id
                .unwrap();

            let outcome = engine
                .delete_object_version("restored", "notes.txt", Some(&marker))
                .await
                .unwrap();

            assert!(outcome.delete_marker);
            assert_eq!(body_of(&engine, "restored", "notes.txt", None).await, "one");
        }

        #[tokio::test]
        async fn should_report_a_version_id_that_names_nothing() {
            let engine = engine("absent").await;
            engine
                .put_bucket_versioning("absent", VersioningStatus::Enabled)
                .await
                .unwrap();
            put(&engine, "absent", "notes.txt", "one").await;

            let error = engine
                .delete_object_version("absent", "notes.txt", Some("deadbeef"))
                .await
                .unwrap_err();

            assert!(matches!(error, Error::NoSuchVersion { .. }));
        }

        /// S3 answers a marker named by its own id with a 405 rather than a 404: the only
        /// thing you may do to a delete marker is delete it.
        #[tokio::test]
        async fn should_refuse_to_read_a_delete_marker_named_by_its_id() {
            let engine = engine("named").await;
            engine
                .put_bucket_versioning("named", VersioningStatus::Enabled)
                .await
                .unwrap();
            put(&engine, "named", "notes.txt", "one").await;
            let marker = engine
                .delete_object("named", "notes.txt")
                .await
                .unwrap()
                .version_id
                .unwrap();

            let error = engine
                .get_object_version("named", "notes.txt", Some(&marker))
                .await
                .unwrap_err();

            assert!(
                matches!(error, Error::DeleteMarkerVersion { .. }),
                "expected a named delete marker, got {error}"
            );
        }

        /// The null version and a null delete marker cannot both exist, or reading
        /// `versionId=null` would be ambiguous.
        #[tokio::test]
        async fn should_replace_a_null_delete_marker_with_the_next_suspended_write() {
            let engine = engine("nulls").await;
            engine
                .put_bucket_versioning("nulls", VersioningStatus::Suspended)
                .await
                .unwrap();
            put(&engine, "nulls", "notes.txt", "one").await;
            engine.delete_object("nulls", "notes.txt").await.unwrap();

            put(&engine, "nulls", "notes.txt", "two").await;

            let versions = engine.object_versions("nulls", "notes.txt").await.unwrap();
            assert_eq!(versions.len(), 1);
            assert!(!versions[0].is_delete_marker());
            assert_eq!(body_of(&engine, "nulls", "notes.txt", None).await, "two");
        }

        #[tokio::test]
        async fn should_not_fail_a_delete_of_a_key_that_is_not_there() {
            let engine = engine("missing").await;

            assert!(engine.delete_object("missing", "notes.txt").await.is_ok());
        }
    }

    mod delete_bucket {
        use super::*;

        #[tokio::test]
        async fn should_refuse_while_a_version_is_still_there() {
            let engine = engine("holding").await;
            engine
                .put_bucket_versioning("holding", VersioningStatus::Enabled)
                .await
                .unwrap();
            put(&engine, "holding", "notes.txt", "one").await;
            engine.delete_object("holding", "notes.txt").await.unwrap();

            // The listing is empty, because the current version is a delete marker — but
            // the bucket still holds two entries a client can list and read.
            assert!(
                engine
                    .list_objects("holding", &crate::ListRequest::default())
                    .await
                    .unwrap()
                    .objects
                    .is_empty()
            );
            assert!(matches!(
                engine.delete_bucket("holding").await.unwrap_err(),
                Error::BucketNotEmpty(_)
            ));
        }

        #[tokio::test]
        async fn should_allow_it_once_every_version_is_deleted() {
            let engine = engine("emptied").await;
            engine
                .put_bucket_versioning("emptied", VersioningStatus::Enabled)
                .await
                .unwrap();
            let written = put(&engine, "emptied", "notes.txt", "one").await.unwrap();
            let marker = engine
                .delete_object("emptied", "notes.txt")
                .await
                .unwrap()
                .version_id
                .unwrap();

            for version in [written, marker] {
                engine
                    .delete_object_version("emptied", "notes.txt", Some(&version))
                    .await
                    .unwrap();
            }

            assert!(engine.delete_bucket("emptied").await.is_ok());
        }
    }
}
