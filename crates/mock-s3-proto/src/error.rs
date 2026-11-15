use axum::response::{IntoResponse, Response};
use http::{StatusCode, header};
use mock_s3_core::Error as CoreError;

use crate::xml;

/// `true` on a response about an object whose current version is a delete marker.
pub const DELETE_MARKER: &str = "x-amz-delete-marker";

/// The version a response is about: the one read, written, or deleted.
pub const VERSION_ID: &str = "x-amz-version-id";

/// An S3 error on its way to the wire.
///
/// The code and status pairing is the contract SDKs depend on, so it is set in one place
/// and taken from the AWS error reference rather than invented:
/// <https://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html>
#[derive(Debug)]
pub struct S3Error {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub resource: Option<String>,
    /// Headers that belong on the error itself rather than in its body.
    ///
    /// A delete marker is the reason this exists: the 404 that a deleted key answers
    /// with carries `x-amz-delete-marker` and the marker's version id, and that is the
    /// only way a client can tell "deleted" from "never existed".
    pub headers: Vec<(&'static str, String)>,
}

impl S3Error {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            resource: None,
            headers: Vec::new(),
        }
    }

    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    pub fn not_implemented(what: &str) -> Self {
        Self::new(
            StatusCode::NOT_IMPLEMENTED,
            "NotImplemented",
            format!("mock-s3 does not implement {what} yet."),
        )
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "InvalidArgument", message)
    }
}

impl From<CoreError> for S3Error {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::NoSuchBucket(bucket) => S3Error::new(
                StatusCode::NOT_FOUND,
                "NoSuchBucket",
                "The specified bucket does not exist.",
            )
            .with_resource(bucket),

            CoreError::BucketAlreadyExists(bucket) => S3Error::new(
                StatusCode::CONFLICT,
                "BucketAlreadyExists",
                "The requested bucket name is not available.",
            )
            .with_resource(bucket),

            // What S3 returns in every region except us-east-1 when you re-create a
            // bucket you already own.
            CoreError::BucketAlreadyOwnedByYou(bucket) => S3Error::new(
                StatusCode::CONFLICT,
                "BucketAlreadyOwnedByYou",
                "Your previous request to create the named bucket succeeded and you already own it.",
            )
            .with_resource(bucket),

            CoreError::BucketNotEmpty(bucket) => S3Error::new(
                StatusCode::CONFLICT,
                "BucketNotEmpty",
                "The bucket you tried to delete is not empty.",
            )
            .with_resource(bucket),

            CoreError::NoSuchKey(key) => S3Error::new(
                StatusCode::NOT_FOUND,
                "NoSuchKey",
                "The specified key does not exist.",
            )
            .with_resource(key),

            CoreError::NoSuchVersion { key, version_id } => S3Error::new(
                StatusCode::NOT_FOUND,
                "NoSuchVersion",
                "The version ID specified in the request does not match an existing version.",
            )
            .with_resource(format!("{key}?versionId={version_id}")),

            // A deleted key in a versioned bucket is still a 404, and the headers are
            // the only thing that separates it from a key that was never written.
            CoreError::DeleteMarker { key, version_id } => S3Error::new(
                StatusCode::NOT_FOUND,
                "NoSuchKey",
                "The specified key does not exist.",
            )
            .with_resource(key)
            .with_header(DELETE_MARKER, "true")
            .with_header(VERSION_ID, version_id),

            // Naming a delete marker outright is a 405, not a 404: the marker is there,
            // it just has no body to return. The `Allow` header says what is left.
            CoreError::DeleteMarkerVersion { key, version_id } => S3Error::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "MethodNotAllowed",
                "The specified method is not allowed against this resource.",
            )
            .with_resource(key)
            .with_header(DELETE_MARKER, "true")
            .with_header(VERSION_ID, version_id)
            .with_header("allow", "DELETE"),

            CoreError::InvalidBucketName(detail) => S3Error::new(
                StatusCode::BAD_REQUEST,
                "InvalidBucketName",
                "The specified bucket is not valid.",
            )
            .with_resource(detail),

            CoreError::InvalidKey(detail) => S3Error::invalid_argument(detail),
            CoreError::InvalidArgument(detail) => S3Error::invalid_argument(detail),

            // A mock-s3 limitation rather than something S3 would reject, so the message
            // says so instead of pretending the request was malformed.
            CoreError::KeyPathConflict(detail) => S3Error::invalid_argument(format!(
                "{detail}. mock-s3 mirrors keys onto the filesystem, so a key cannot \
                 both hold data and act as a directory for other keys."
            )),

            CoreError::NoSuchUpload(upload_id) => S3Error::new(
                StatusCode::NOT_FOUND,
                "NoSuchUpload",
                "The specified upload does not exist. The upload ID may be invalid, or the \
                 upload may have been aborted or completed.",
            )
            .with_resource(upload_id),

            CoreError::InvalidPart(detail) => S3Error::new(
                StatusCode::BAD_REQUEST,
                "InvalidPart",
                "One or more of the specified parts could not be found, or the entity tag \
                 did not match.",
            )
            .with_resource(detail),

            CoreError::InvalidPartOrder => S3Error::new(
                StatusCode::BAD_REQUEST,
                "InvalidPartOrder",
                "The list of parts was not in ascending order. Parts must be ordered by \
                 part number.",
            ),

            CoreError::EntityTooSmall { number, size } => S3Error::new(
                StatusCode::BAD_REQUEST,
                "EntityTooSmall",
                "Your proposed upload is smaller than the minimum allowed object size.",
            )
            .with_resource(format!("part {number} is {size} bytes")),

            CoreError::Io(error) => {
                tracing::error!(%error, "storage failure");
                S3Error::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    "We encountered an internal error. Please try again.",
                )
            }
        }
    }
}

impl IntoResponse for S3Error {
    fn into_response(self) -> Response {
        // The id in the body and the id in the header have to be the same one, which is
        // why it is minted here rather than left to the layer that fills it in for
        // successful responses.
        let id = crate::request_id::next();
        let body = xml::error_document(self.code, &self.message, self.resource.as_deref(), &id);

        let mut response = (
            self.status,
            [(header::CONTENT_TYPE, "application/xml")],
            body,
        )
            .into_response();

        if let Ok(value) = http::HeaderValue::from_str(&id) {
            response.headers_mut().insert(
                http::HeaderName::from_static(crate::request_id::HEADER),
                value,
            );
        }

        for (name, value) in &self.headers {
            if let Ok(name) = http::HeaderName::try_from(*name) {
                crate::response::set(response.headers_mut(), name, value);
            }
        }

        response
    }
}
