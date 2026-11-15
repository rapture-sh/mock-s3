/// Everything the engine can fail with.
///
/// These map onto S3 error codes in `mock-s3-proto`; the mapping lives there so this
/// crate stays free of protocol concerns.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no such bucket: {0}")]
    NoSuchBucket(String),

    #[error("bucket already exists: {0}")]
    BucketAlreadyExists(String),

    #[error("bucket already owned by you: {0}")]
    BucketAlreadyOwnedByYou(String),

    #[error("bucket is not empty: {0}")]
    BucketNotEmpty(String),

    #[error("no such key: {0}")]
    NoSuchKey(String),

    #[error("invalid bucket name: {0}")]
    InvalidBucketName(String),

    #[error("invalid object key: {0}")]
    InvalidKey(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// A key that cannot be mirrored onto the filesystem because something else already
    /// occupies the path it would take. See `AGENTS.md` for the known gap this covers.
    #[error("object key conflicts with an existing path: {0}")]
    KeyPathConflict(String),

    #[error("storage failure: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
