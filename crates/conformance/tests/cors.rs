//! CORS configuration through the real SDK.
//!
//! The preflight itself is exercised from the browser side in `tests/suites/cors.test.ts`,
//! because an S3 SDK never sends an `OPTIONS` request — only a browser does.

use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::types::{CorsConfiguration, CorsRule};

async fn harness_with_bucket() -> conformance::Harness {
    let h = conformance::harness().await;
    h.s3.create_bucket()
        .bucket("my-bucket")
        .send()
        .await
        .unwrap();
    h
}

fn rule() -> CorsRule {
    CorsRule::builder()
        .allowed_origins("https://example.com")
        .allowed_methods("GET")
        .allowed_methods("PUT")
        .allowed_headers("*")
        .expose_headers("ETag")
        .max_age_seconds(3000)
        .build()
        .unwrap()
}

#[tokio::test]
async fn should_round_trip_a_cors_configuration() {
    let h = harness_with_bucket().await;

    h.s3.put_bucket_cors()
        .bucket("my-bucket")
        .cors_configuration(
            CorsConfiguration::builder()
                .cors_rules(rule())
                .build()
                .unwrap(),
        )
        .send()
        .await
        .unwrap();

    let got =
        h.s3.get_bucket_cors()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();

    let rules = got.cors_rules();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].allowed_origins(), ["https://example.com"]);
    assert_eq!(rules[0].allowed_methods(), ["GET", "PUT"]);
    assert_eq!(rules[0].expose_headers(), ["ETag"]);
    assert_eq!(rules[0].max_age_seconds(), Some(3000));
}

#[tokio::test]
async fn should_report_no_such_cors_configuration_before_one_is_set() {
    let h = harness_with_bucket().await;

    let error =
        h.s3.get_bucket_cors()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap_err()
            .into_service_error();

    assert_eq!(error.code(), Some("NoSuchCORSConfiguration"));
}

#[tokio::test]
async fn should_forget_the_configuration_once_deleted() {
    let h = harness_with_bucket().await;
    h.s3.put_bucket_cors()
        .bucket("my-bucket")
        .cors_configuration(
            CorsConfiguration::builder()
                .cors_rules(rule())
                .build()
                .unwrap(),
        )
        .send()
        .await
        .unwrap();

    h.s3.delete_bucket_cors()
        .bucket("my-bucket")
        .send()
        .await
        .unwrap();

    let error =
        h.s3.get_bucket_cors()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap_err()
            .into_service_error();
    assert_eq!(error.code(), Some("NoSuchCORSConfiguration"));
}

/// Configuration is stored as an object under a hidden prefix, so a listing must not
/// show it — otherwise configuring a bucket would appear to put data in it.
#[tokio::test]
async fn should_keep_the_configuration_out_of_listings() {
    let h = harness_with_bucket().await;
    h.s3.put_bucket_cors()
        .bucket("my-bucket")
        .cors_configuration(
            CorsConfiguration::builder()
                .cors_rules(rule())
                .build()
                .unwrap(),
        )
        .send()
        .await
        .unwrap();

    let listed =
        h.s3.list_objects_v2()
            .bucket("my-bucket")
            .send()
            .await
            .unwrap();

    assert_eq!(listed.key_count(), Some(0));
}
