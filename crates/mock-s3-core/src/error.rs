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

    #[error("no version {version_id} of {key}")]
    NoSuchVersion { key: String, version_id: String },

    /// The current version of the key is a delete marker, so the key reads as absent
    /// even though its earlier versions are still there.
    #[error("the current version of {key} is a delete marker")]
    DeleteMarker { key: String, version_id: String },

    /// A version id was named and it turned out to be a delete marker. S3 separates this
    /// from the case above: a marker reached as the current version is a 404, while one
    /// named outright is a 405, because the only thing you may do to it is delete it.
    #[error("version {version_id} of {key} is a delete marker")]
    DeleteMarkerVersion { key: String, version_id: String },

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

    #[error("no such multipart upload: {0}")]
    NoSuchUpload(String),

    #[error("invalid part: {0}")]
    InvalidPart(String),

    #[error("parts were not listed in ascending order")]
    InvalidPartOrder,

    #[error("part {number} is {size} bytes, below the minimum for a non-final part")]
    EntityTooSmall { number: u32, size: u64 },

    #[error("storage failure: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
