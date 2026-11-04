//! `coordinator_local` -- CLI entry point for the local-mode coordinator.
//!
//! Usage: `coordinator_local --workspace <path> --socket <sockpath>`.
//!
//! Wires an in-memory store, a gRPC plugin factory, and the sweep allocator
//! behind the runner-facing `CoordinatorService`.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

use coordinator::allocator::FixtureAllocator;
use coordinator::credentials::CredentialStore;
use coordinator::model::settings::Settings;
use coordinator::plugin::FixtureFactory;
use coordinator::plugin::grpc::GrpcPluginFactory;
use coordinator::service::Service;
use coordinator::store::Store;
use coordinator::store::memory::InMemoryStore;
use systemtest_proto::systemtest::v1::coordinator_service_server::CoordinatorServiceServer;

/// How long to wait for a freshly spawned plugin to serve its socket.
const PLUGIN_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Parser, Debug)]
#[command(name = "coordinator_local")]
struct Args {
    /// Workspace root this coordinator instance is scoped to.
    #[arg(long)]
    workspace: PathBuf,

    /// Unix domain socket to serve `CoordinatorService` on.
    #[arg(long)]
    socket: PathBuf,
}

/// Takes the exclusive lock that makes this the only coordinator serving
/// `dir`, or returns `None` if another one already holds it.
fn lock_instance(dir: &Path) -> Result<Option<File>> {
    let path = dir.join(".lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening lock file {path:?}"))?;

    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e).with_context(|| format!("locking {path:?}")),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        workspace: _,
        socket,
    } = Args::parse();

    let instance_dir = socket
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&instance_dir)
        .with_context(|| format!("creating socket directory {instance_dir:?}"))?;

    let Some(_lock) = lock_instance(&instance_dir)? else {
        // Another instance is already running.
        return Ok(());
    };

    // No live coordinator can own this socket, so a leftover file is stale.
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("binding coordinator socket at {socket:?}"))?;
    let incoming = UnixListenerStream::new(listener);

    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let factory: Arc<dyn FixtureFactory> = Arc::new(GrpcPluginFactory::new(
        instance_dir.clone(),
        PLUGIN_STARTUP_TIMEOUT,
    ));
    let credentials = Arc::new(CredentialStore::new());
    let settings = Settings::default_local();

    // Reconcile leases in the background.
    let allocator = FixtureAllocator::new(
        store.clone(),
        factory.clone(),
        credentials.clone(),
        settings,
    );
    tokio::spawn(async move {
        if let Err(e) = allocator.run().await {
            eprintln!("allocator stopped: {e:#}");
        }
    });

    let service = Service::new(store, factory, credentials);
    Server::builder()
        .add_service(CoordinatorServiceServer::new(service))
        .serve_with_incoming(incoming)
        .await
        .context("coordinator server exited")?;

    Ok(())
}
