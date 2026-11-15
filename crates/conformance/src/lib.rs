//! Harness for the conformance suite.
//!
//! Every test here drives a real `aws-sdk-s3` client against an in-process instance.
//! That is the point: a hand-written HTTP request only proves the handler matches our
//! own understanding of S3, while the real SDK proves it matches what callers send.

use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use mock_s3_testkit::Instance;

/// A running instance and a client pointed at it. Both die with the harness.
pub struct Harness {
    pub server: Instance,
    pub s3: Client,
}

/// The default harness: memory-backed.
pub async fn harness() -> Harness {
    let server = mock_s3_testkit::spawn()
        .await
        .expect("the test harness could not bind a port");
    let s3 = client(server.endpoint());
    Harness { server, s3 }
}

/// A harness backed by a temporary directory, for tests about the on-disk layout.
pub async fn harness_on_disk() -> Harness {
    let server = mock_s3_testkit::spawn_temp()
        .await
        .expect("the test harness could not open a data directory");
    let s3 = client(server.endpoint());
    Harness { server, s3 }
}

/// Build a client the way a user of mock-s3 has to: an endpoint override, path-style
/// addressing, and any credentials at all.
pub fn client(endpoint: &str) -> Client {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .endpoint_url(endpoint)
        .region(Region::new(mock_s3_testkit::REGION.to_string()))
        .credentials_provider(Credentials::new(
            "test",
            "test",
            None,
            None,
            "mock-s3-conformance",
        ))
        // Without this the SDK would try `bucket.127.0.0.1`, which has no DNS.
        .force_path_style(true)
        .build();

    Client::from_conf(config)
}
