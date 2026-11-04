//! Manages plugin child processes, one shared process per `impl_id`.
//!
//! Keys by `impl_id` (one binary per impl in local mode) so that
//! `GetFixtureInfo`/`DestroyFixture` — which carry only an `impl_id` — can find
//! the right process. Keying by binary-content hash is a remote-mode
//! refinement. SIGTERM-before-SIGKILL is a TODO (see [`PluginProcess`] Drop);
//! today the child is SIGKILLed when the manager is dropped.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tonic::transport::Channel;

use crate::plugin_client::PluginProcess;
use systemtest_proto::systemtest::v1::coordinator_plugin_client::CoordinatorPluginClient;

pub struct PluginProcessManager {
    instance_dir: PathBuf,
    startup_timeout: Duration,
    plugins: Mutex<HashMap<String, PluginProcess>>,
}

impl PluginProcessManager {
    pub fn new(instance_dir: PathBuf, startup_timeout: Duration) -> Self {
        PluginProcessManager {
            instance_dir,
            startup_timeout,
            plugins: Mutex::new(HashMap::new()),
        }
    }

    /// A client for `impl_id`, spawning the plugin from `binary_path` on first
    /// use and sharing it thereafter.
    pub async fn client(
        &self,
        impl_id: &str,
        binary_path: &str,
    ) -> Result<CoordinatorPluginClient<Channel>> {
        let mut plugins = self.plugins.lock().await;
        if let Some(proc) = plugins.get(impl_id) {
            return Ok(proc.client.clone());
        }
        let socket = self.socket_path(impl_id);
        let proc = PluginProcess::spawn(binary_path, &socket, self.startup_timeout).await?;
        let client = proc.client.clone();
        plugins.insert(impl_id.to_string(), proc);
        Ok(client)
    }

    /// A client for an already-spawned `impl_id` (for `GetFixtureInfo`/`Destroy`).
    pub async fn existing_client(&self, impl_id: &str) -> Result<CoordinatorPluginClient<Channel>> {
        let plugins = self.plugins.lock().await;
        plugins
            .get(impl_id)
            .map(|proc| proc.client.clone())
            .ok_or_else(|| anyhow!("no running plugin for impl_id {impl_id}"))
    }

    fn socket_path(&self, impl_id: &str) -> PathBuf {
        let hash: String = Sha256::digest(impl_id.as_bytes())
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        self.instance_dir
            .join("plugins")
            .join(format!("{hash}.sock"))
    }
}
