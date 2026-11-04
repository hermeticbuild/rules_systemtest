//! Spawns a plugin child process and talks to it as a `CoordinatorPlugin`
//! client over a Unix domain socket.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::time::sleep;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use systemtest_proto::systemtest::v1::{
    CreateRequest, CreateResponse, DescribeResponses, Empty, GetFixtureInfoRequest,
    GetFixtureInfoResponse, coordinator_plugin_client::CoordinatorPluginClient,
};

/// A plugin process the coordinator spawned, plus a connected client.
pub struct PluginProcess {
    child: Child,
    pub client: CoordinatorPluginClient<Channel>,
}

impl PluginProcess {
    /// Spawns `binary_path --socket <socket_path>` and connects to it.
    pub async fn spawn(
        binary_path: &str,
        socket_path: &Path,
        startup_timeout: Duration,
    ) -> Result<Self> {
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating plugin socket dir {parent:?}"))?;
        }
        // Best-effort: clear a stale socket file from a previous run.
        let _ = std::fs::remove_file(socket_path);

        let mut child = Command::new(binary_path)
            .arg("--socket")
            .arg(socket_path)
            .spawn()
            .with_context(|| format!("spawning plugin binary {binary_path}"))?;

        let channel = connect_with_retry(&mut child, socket_path, startup_timeout).await?;
        let client = CoordinatorPluginClient::new(channel);
        Ok(Self { child, client })
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        // Best-effort: if the coordinator itself is exiting, don't leave the
        // plugin child running.
        // TODO: We should SIGTERM and wait for graceful shutdown before SIGKILL.
        let _ = self.child.start_kill();
    }
}

/// Dials the plugin's UDS with a tonic channel, retrying until `timeout`.
async fn connect_with_retry(
    child: &mut Child,
    socket_path: &Path,
    timeout: Duration,
) -> Result<Channel> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match try_connect(socket_path).await {
            Ok(channel) => return Ok(channel),
            Err(err) => {
                if let Some(status) = child.try_wait().context("polling plugin process")? {
                    return Err(err).context(format!(
                        "plugin process exited ({status}) before serving {socket_path:?}"
                    ));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(err).context("timed out connecting to plugin socket");
                }
                sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn try_connect(socket_path: &Path) -> Result<Channel> {
    let socket_path = socket_path.to_path_buf();
    // The URI is unused but `Endpoint::try_from` requires a well-formed one.
    let channel = Endpoint::try_from("http://[::]:50051")?
        .connect_with_connector(service_fn(move |_: Uri| {
            let socket_path = socket_path.clone();
            async move {
                let stream = UnixStream::connect(&socket_path).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await?;
    Ok(channel)
}

/// Calls `DescribePlugin`, used as a health check.
pub async fn describe(client: &mut CoordinatorPluginClient<Channel>) -> Result<DescribeResponses> {
    Ok(client.describe_plugin(Empty {}).await?.into_inner())
}

/// Calls the unary `CreateFixture`, returning as soon as creation is
/// initiated. Readiness is driven by the reconciler via `get_fixture_info`.
pub async fn create_fixture(
    client: &mut CoordinatorPluginClient<Channel>,
    request: CreateRequest,
) -> Result<CreateResponse> {
    Ok(client.create_fixture(request).await?.into_inner())
}

/// Calls `GetFixtureInfo` to re-derive a fixture's status and outputs.
pub async fn get_fixture_info(
    client: &mut CoordinatorPluginClient<Channel>,
    request: GetFixtureInfoRequest,
) -> Result<GetFixtureInfoResponse> {
    Ok(client.get_fixture_info(request).await?.into_inner())
}
