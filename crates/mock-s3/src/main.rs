//! Argument parsing and listener wiring. Everything else lives in the other crates.

mod cli;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use mock_s3_core::{DiskStore, Engine, MemoryStore, Store};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    init_tracing(&cli.log, cli.log_format);

    let (store, backend): (Arc<dyn Store>, String) = if cli.in_memory {
        (Arc::new(MemoryStore::new()), "memory".to_string())
    } else {
        let store = DiskStore::open(&cli.data)
            .await
            .with_context(|| format!("could not open the data directory {}", cli.data.display()))?;
        let described = store.root().display().to_string();
        (Arc::new(store), described)
    };

    let engine = Arc::new(Engine::new(store, cli.region.clone()));

    let s3_addr = SocketAddr::new(cli.bind, cli.port);
    let s3_listener = TcpListener::bind(s3_addr)
        .await
        .with_context(|| format!("could not bind the S3 API to {s3_addr}"))?;

    let console_listener = if cli.no_console {
        None
    } else {
        let addr = SocketAddr::new(cli.bind, cli.console_port);
        Some(
            TcpListener::bind(addr)
                .await
                .with_context(|| format!("could not bind the console to {addr}"))?,
        )
    };

    tracing::info!(region = %cli.region, storage = %backend, "mock-s3 starting");
    tracing::info!("S3 API      http://localhost:{}", cli.port);
    match &console_listener {
        Some(_) => tracing::info!("console     http://localhost:{}", cli.console_port),
        None => tracing::info!("console     disabled"),
    }
    match &cli.base_domain {
        Some(domain) => tracing::info!(
            "addressing  path-style, and virtual-hosted under *.{}",
            domain
        ),
        None => tracing::info!("addressing  path-style; set forcePathStyle on your client"),
    }

    // One log, written by the S3 listener and read by the console.
    let request_log = Arc::new(mock_s3_proto::RequestLog::new(cli.request_log_size));

    let proto_config = mock_s3_proto::ProtoConfig {
        base_domain: cli.base_domain.clone(),
        request_log: request_log.clone(),
    };

    let s3 = {
        let router = mock_s3_proto::router(engine.clone(), proto_config);
        tokio::spawn(async move {
            axum::serve(s3_listener, router.into_make_service())
                .with_graceful_shutdown(shutdown())
                .await
        })
    };

    let console = console_listener.map(|listener| {
        let router = mock_s3_console::router(engine.clone(), request_log.clone());
        tokio::spawn(async move {
            axum::serve(listener, router.into_make_service())
                .with_graceful_shutdown(shutdown())
                .await
        })
    });

    s3.await
        .context("the S3 listener panicked")?
        .context("the S3 listener failed")?;
    if let Some(console) = console {
        console
            .await
            .context("the console listener panicked")?
            .context("the console listener failed")?;
    }

    tracing::info!("mock-s3 stopped");
    Ok(())
}

fn init_tracing(filter: &str, format: cli::LogFormat) {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));

    match format {
        cli::LogFormat::Json => fmt().with_env_filter(filter).json().init(),
        // No timestamp in text mode: container runtimes add their own, and a doubled
        // one is just noise in the common case.
        cli::LogFormat::Text => fmt()
            .with_env_filter(filter)
            .with_target(false)
            .without_time()
            .init(),
    }
}

/// Both listeners wait on the same signal, so Ctrl-C stops the whole process cleanly.
async fn shutdown() {
    if tokio::signal::ctrl_c().await.is_err() {
        // Without a signal handler there is nothing to wait for, so stay up and let the
        // process be killed instead of shutting down immediately.
        std::future::pending::<()>().await;
    }
}
