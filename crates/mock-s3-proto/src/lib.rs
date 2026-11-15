//! The S3 wire protocol for mock-s3.
//!
//! This crate is the only one that knows S3 is XML over HTTP. Handlers parse a request,
//! call the engine in `mock-s3-core`, and map the result back onto the status codes,
//! headers and XML shapes that real S3 uses. No storage logic lives here.

pub mod chunked;
pub mod error;
pub mod ops;
pub mod query;
pub mod request_id;
pub mod response;
pub mod router;
pub mod sigv4;
pub mod token;
pub mod xml;

pub use error::S3Error;
pub use router::{ProtoConfig, router};
