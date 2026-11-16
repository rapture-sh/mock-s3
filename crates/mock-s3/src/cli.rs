use std::net::IpAddr;
use std::path::PathBuf;

use clap::Parser;

/// Every flag has a `MOCK_S3_`-prefixed environment variable, which is usually easier to
/// set in a compose file than an argument list.
#[derive(Debug, Parser)]
#[command(
    name = "mock-s3",
    version,
    about = "An S3-compatible object storage server for local development and testing"
)]
pub struct Cli {
    /// Port for the S3 API.
    #[arg(long, env = "MOCK_S3_PORT", default_value_t = 9000)]
    pub port: u16,

    /// Port for the web console and admin API.
    #[arg(long, env = "MOCK_S3_CONSOLE_PORT", default_value_t = 9001)]
    pub console_port: u16,

    /// Address to bind both listeners to.
    #[arg(long, env = "MOCK_S3_BIND", default_value = "0.0.0.0")]
    pub bind: IpAddr,

    /// Where objects are written. The container image sets this to /data.
    #[arg(long, env = "MOCK_S3_DATA", default_value = "./data")]
    pub data: PathBuf,

    /// Keep everything in memory and discard it on exit.
    #[arg(long, env = "MOCK_S3_IN_MEMORY")]
    pub in_memory: bool,

    /// Do not start the console listener at all.
    #[arg(long, env = "MOCK_S3_NO_CONSOLE")]
    pub no_console: bool,

    /// Region reported in responses.
    #[arg(long, env = "MOCK_S3_REGION", default_value = "us-east-1")]
    pub region: String,

    /// Serve virtual-hosted addressing under this host suffix, so that
    /// `my-bucket.<suffix>` addresses `my-bucket`.
    ///
    /// Off by default, because with it set every hostname's first label would be read as
    /// a bucket name, including `localhost`.
    #[arg(long, env = "MOCK_S3_BASE_DOMAIN")]
    pub base_domain: Option<String>,

    /// Log level, or a full tracing filter such as `mock_s3=debug,axum=info`.
    #[arg(long, env = "MOCK_S3_LOG", default_value = "info")]
    pub log: String,

    /// Log output format. `json` is one object per line, for a CI job that parses it.
    #[arg(long, env = "MOCK_S3_LOG_FORMAT", default_value = "text")]
    pub log_format: LogFormat,

    /// How many recent requests to keep for `GET /_admin/requests`. Zero turns it off.
    #[arg(long, env = "MOCK_S3_REQUEST_LOG_SIZE", default_value_t = 1000)]
    pub request_log_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LogFormat {
    Text,
    Json,
}
