//! Per-bucket configuration documents.
//!
//! Stored the same way multipart parts are: as objects under a hidden prefix, so both
//! backends support them without the `Store` trait growing, and so the volume shows what
//! a bucket is configured with.
//!
//! The document is kept exactly as the client sent it, which is how a `GET` can return
//! what a `PUT` put there. Anything that needs to act on a setting parses it on read —
//! the engine does not know what a CORS rule means.

use bytes::Bytes;

use crate::error::{Error, Result};
use crate::model::PutOptions;
use crate::{Engine, HIDDEN_PREFIX};

/// Where configuration documents live, inside the hidden prefix.
pub const CONFIG_PREFIX: &str = ".mock-s3/config/";

/// The CORS document, which is the only setting mock-s3 acts on rather than merely
/// accepting. Browsers cannot talk to the server at all without it.
pub const CORS: &str = "cors";

fn config_key(name: &str) -> String {
    format!("{CONFIG_PREFIX}{name}.xml")
}

impl Engine {
    pub async fn put_bucket_config(&self, bucket: &str, name: &str, body: Bytes) -> Result<()> {
        self.head_bucket(bucket).await?;

        self.store
            .put_object(
                bucket,
                &config_key(name),
                body,
                PutOptions::with_content_type("application/xml"),
            )
            .await?;
        Ok(())
    }

    /// The stored document, or `None` when the bucket has never been configured.
    pub async fn get_bucket_config(&self, bucket: &str, name: &str) -> Result<Option<Bytes>> {
        self.head_bucket(bucket).await?;

        match self.store.get_object(bucket, &config_key(name)).await {
            Ok(data) => Ok(Some(data.body)),
            Err(Error::NoSuchKey(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    pub async fn delete_bucket_config(&self, bucket: &str, name: &str) -> Result<()> {
        self.head_bucket(bucket).await?;
        self.store.delete_object(bucket, &config_key(name)).await
    }
}

/// Is this key part of mock-s3's own bookkeeping rather than a client's data?
pub fn is_hidden(key: &str) -> bool {
    key.starts_with(HIDDEN_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_put_a_config_key_inside_the_hidden_prefix() {
        let key = config_key(CORS);

        assert!(is_hidden(&key), "{key} must be hidden from listings");
        assert_eq!(key, ".mock-s3/config/cors.xml");
    }

    #[test]
    fn should_not_hide_an_ordinary_key() {
        assert!(!is_hidden("photos/cat.jpg"));
        assert!(!is_hidden("mock-s3/notes.txt"));
    }
}
