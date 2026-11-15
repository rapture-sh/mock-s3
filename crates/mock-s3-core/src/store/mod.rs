use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use bytes::Bytes;

use crate::error::{Error, Result};
use crate::model::{BucketInfo, ObjectData, ObjectMeta};

pub mod disk;
pub mod memory;

pub use disk::DiskStore;
pub use memory::MemoryStore;

/// A key ending in `/` is a legal S3 key — the AWS console creates them as folder
/// markers — but it cannot be mirrored onto the filesystem as a file. The payload goes
/// in a file of this name inside the directory instead.
pub const DIR_MARKER: &str = ".s3-dir-marker";

/// What a backend has to be able to do. Disk is the default and memory is the
/// `--in-memory` opt-out, so every operation has to work on both.
#[async_trait::async_trait]
pub trait Store: Send + Sync + 'static {
    async fn create_bucket(&self, bucket: &str) -> Result<()>;
    async fn delete_bucket(&self, bucket: &str) -> Result<()>;
    async fn bucket_exists(&self, bucket: &str) -> Result<bool>;
    async fn list_buckets(&self) -> Result<Vec<BucketInfo>>;

    async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        body: Bytes,
        content_type: &str,
        user_metadata: BTreeMap<String, String>,
    ) -> Result<ObjectMeta>;

    async fn get_object(&self, bucket: &str, key: &str) -> Result<ObjectData>;
    async fn head_object(&self, bucket: &str, key: &str) -> Result<ObjectMeta>;
    async fn delete_object(&self, bucket: &str, key: &str) -> Result<()>;

    /// Every key in the bucket, sorted. `listing::list` turns this into a page.
    async fn list_objects(&self, bucket: &str) -> Result<Vec<ObjectMeta>>;

    /// Drop everything. The admin API calls this between test runs.
    async fn reset(&self) -> Result<()>;
}

/// Turn an object key into a relative filesystem path, safely.
///
/// This is the only place in the project allowed to do it. An object key is an arbitrary
/// string, so `../`, a leading `/` and a Windows drive prefix are all legal keys and all
/// have to be rejected rather than resolved. The path is built from validated segments
/// only, so traversal is impossible by construction rather than by a check afterwards.
pub fn key_to_relative_path(key: &str) -> Result<PathBuf> {
    let reject = |why: &str| Err(Error::InvalidKey(format!("{key}: {why}")));

    let segments: Vec<&str> = key.split('/').collect();
    let mut path = PathBuf::new();

    for (index, segment) in segments.iter().enumerate() {
        let is_last = index + 1 == segments.len();

        if segment.is_empty() {
            // A trailing slash is a folder marker and is representable; an empty
            // segment anywhere else (`a//b`) or a leading slash is not.
            if is_last && index > 0 {
                path.push(DIR_MARKER);
                continue;
            }
            return reject("cannot be stored: the key has an empty path segment");
        }
        if *segment == "." || *segment == ".." {
            return reject("cannot be stored: the key contains a relative path segment");
        }
        if segment.contains('\\') {
            return reject("cannot be stored: the key contains a backslash");
        }
        if segment.contains(':') {
            return reject("cannot be stored: the key contains a colon");
        }
        if *segment == DIR_MARKER {
            return reject("cannot be stored: the key uses a reserved segment name");
        }
        path.push(segment);
    }

    Ok(path)
}

/// The inverse of [`key_to_relative_path`], for walking a data directory back into keys.
///
/// Returns `None` for a path that could not have come from a key, which is how a stray
/// file in the volume gets ignored rather than listed as a broken object.
pub fn relative_path_to_key(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return None,
        }
    }

    if parts.is_empty() {
        return None;
    }
    if parts.last().map(String::as_str) == Some(DIR_MARKER) {
        parts.pop();
        if parts.is_empty() {
            return None;
        }
        return Some(format!("{}/", parts.join("/")));
    }
    Some(parts.join("/"))
}

/// A content type for a key that arrived without one, which is what happens to files
/// seeded into the volume by hand.
pub fn guess_content_type(key: &str) -> &'static str {
    let extension = key.rsplit_once('.').map(|(_, ext)| ext).unwrap_or_default();

    match extension.to_ascii_lowercase().as_str() {
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "mp4" => "video/mp4",
        "wasm" => "application/wasm",
        _ => crate::model::DEFAULT_CONTENT_TYPE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod key_to_relative_path {
        use super::*;

        #[test]
        fn should_mirror_a_nested_key_onto_nested_directories() {
            let path = key_to_relative_path("photos/2024/cat.jpg").unwrap();

            assert_eq!(path, PathBuf::from("photos").join("2024").join("cat.jpg"));
        }

        #[test]
        fn should_reject_a_key_that_climbs_out_of_the_data_directory() {
            assert!(key_to_relative_path("../../etc/passwd").is_err());
            assert!(key_to_relative_path("photos/../../../etc/passwd").is_err());
        }

        #[test]
        fn should_reject_a_key_with_a_leading_slash() {
            assert!(key_to_relative_path("/etc/passwd").is_err());
        }

        #[test]
        fn should_reject_a_key_with_a_windows_drive_prefix() {
            assert!(key_to_relative_path("C:/Windows/System32/x").is_err());
            assert!(key_to_relative_path("C:\\Windows\\System32\\x").is_err());
        }

        #[test]
        fn should_reject_a_key_with_an_empty_middle_segment() {
            assert!(key_to_relative_path("a//b").is_err());
        }

        #[test]
        fn should_store_a_trailing_slash_key_as_a_directory_marker() {
            let path = key_to_relative_path("photos/").unwrap();

            assert_eq!(path, PathBuf::from("photos").join(DIR_MARKER));
        }

        #[test]
        fn should_reject_a_key_that_uses_the_reserved_marker_name() {
            assert!(key_to_relative_path(&format!("photos/{DIR_MARKER}")).is_err());
        }
    }

    mod relative_path_to_key {
        use super::*;

        #[test]
        fn should_round_trip_a_nested_key() {
            let key = "photos/2024/cat.jpg";
            let path = key_to_relative_path(key).unwrap();

            assert_eq!(relative_path_to_key(&path).as_deref(), Some(key));
        }

        #[test]
        fn should_round_trip_a_folder_marker_key() {
            let key = "photos/";
            let path = key_to_relative_path(key).unwrap();

            assert_eq!(relative_path_to_key(&path).as_deref(), Some(key));
        }

        #[test]
        fn should_ignore_a_path_that_escapes_the_root() {
            assert!(relative_path_to_key(Path::new("../x")).is_none());
        }
    }
}
