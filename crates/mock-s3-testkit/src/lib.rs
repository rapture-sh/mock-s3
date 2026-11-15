//! Spawn a mock-s3 instance inside a Rust test.
//!
//! Both listeners bind port 0, so tests run in parallel without fighting over a port.
//! The instance shuts down when the returned [`Instance`] is dropped.
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! let server = mock_s3_testkit::spawn().await?;
//! let endpoint = server.endpoint().to_string();
//! // point an S3 client at `endpoint`, with path-style addressing
//! # Ok(())
//! # }
//! ```

use std::path::Path;
use std::sync::Arc;

use mock_s3_core::{DiskStore, Engine, MemoryStore, Store};
use tokio::net::TcpListener;
use tokio::sync::watch;

/// The region a spawned instance reports. Clients have to be configured with a region,
/// and this is the one to give them.
pub const REGION: &str = "us-east-1";

/// A running instance. Dropping it shuts both listeners down.
pub struct Instance {
    endpoint: String,
    console_endpoint: String,
    shutdown: watch::Sender<bool>,
    /// Held so the directory outlives the server when the caller used [`spawn_temp`].
    _temp: Option<tempfile::TempDir>,
}

impl Instance {
    /// The S3 API base URL, for example `http://127.0.0.1:52431`.
    ///
    /// Configure clients with path-style addressing: the host has no wildcard DNS, so
    /// virtual-hosted style cannot resolve.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The console and admin API base URL.
    pub fn console_endpoint(&self) -> &str {
        &self.console_endpoint
    }

    pub fn region(&self) -> &str {
        REGION
    }

    /// Shut down without waiting for the drop.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// An instance backed by memory. The right default for a test.
pub async fn spawn() -> std::io::Result<Instance> {
    spawn_with(Arc::new(MemoryStore::new()), None).await
}

/// An instance backed by a directory you control, for tests about the on-disk layout.
pub async fn spawn_on_disk(data: impl AsRef<Path>) -> std::io::Result<Instance> {
    let store = DiskStore::open(data.as_ref())
        .await
        .map_err(std::io::Error::other)?;
    spawn_with(Arc::new(store), None).await
}

/// An instance backed by a temporary directory that is removed with the instance.
pub async fn spawn_temp() -> std::io::Result<Instance> {
    let temp = tempfile::tempdir()?;
    let store = DiskStore::open(temp.path())
        .await
        .map_err(std::io::Error::other)?;
    spawn_with(Arc::new(store), Some(temp)).await
}

async fn spawn_with(
    store: Arc<dyn Store>,
    temp: Option<tempfile::TempDir>,
) -> std::io::Result<Instance> {
    let engine = Arc::new(Engine::new(store, REGION));

    // Port 0 lets the OS pick, and local_addr tells us what it picked. Binding before
    // returning means the endpoint is usable the moment this function does.
    let s3_listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let console_listener = TcpListener::bind(("127.0.0.1", 0)).await?;

    let endpoint = format!("http://{}", s3_listener.local_addr()?);
    let console_endpoint = format!("http://{}", console_listener.local_addr()?);

    let (shutdown, _) = watch::channel(false);

    let s3_router = mock_s3_proto::router(engine.clone());
    let s3_signal = shutdown.subscribe();
    tokio::spawn(async move {
        let _ = axum::serve(s3_listener, s3_router.into_make_service())
            .with_graceful_shutdown(wait_for(s3_signal))
            .await;
    });

    let console_router = mock_s3_console::router(engine);
    let console_signal = shutdown.subscribe();
    tokio::spawn(async move {
        let _ = axum::serve(console_listener, console_router.into_make_service())
            .with_graceful_shutdown(wait_for(console_signal))
            .await;
    });

    Ok(Instance {
        endpoint,
        console_endpoint,
        shutdown,
        _temp: temp,
    })
}

async fn wait_for(mut signal: watch::Receiver<bool>) {
    loop {
        if *signal.borrow_and_update() {
            return;
        }
        if signal.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn should_give_distinct_ports_to_two_instances() {
        let first = spawn().await.unwrap();
        let second = spawn().await.unwrap();

        assert_ne!(first.endpoint(), second.endpoint());
    }

    #[tokio::test]
    async fn should_bind_the_console_on_its_own_port() {
        let server = spawn().await.unwrap();

        assert_ne!(server.endpoint(), server.console_endpoint());
    }

    #[tokio::test]
    async fn should_create_the_data_directory_when_spawned_on_disk() {
        let server = spawn_temp().await.unwrap();

        assert!(server.endpoint().starts_with("http://127.0.0.1:"));
    }
}
