//! The real [`FixtureFactory`]: a gRPC client to plugin processes.

use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use systemtest_proto::systemtest::v1 as pb;

use crate::model::budget::Capacity;
use crate::plugin::process::PluginProcessManager;
use crate::plugin::{CreateMetadata, CreateOutput, FixtureFactory, FixtureInfo, IssuedCredentials};
use crate::plugin_client;

pub struct GrpcPluginFactory {
    manager: PluginProcessManager,
}

impl GrpcPluginFactory {
    pub fn new(instance_dir: std::path::PathBuf, startup_timeout: Duration) -> Self {
        GrpcPluginFactory {
            manager: PluginProcessManager::new(instance_dir, startup_timeout),
        }
    }
}

fn binary_path(spec: &pb::FixtureDescription) -> Result<&str> {
    match &spec.plugin {
        Some(pb::fixture_description::Plugin::BinaryPath(p)) => Ok(p),
        Some(pb::fixture_description::Plugin::ContainerRef(_)) => Err(anyhow!(
            "remote-mode (container_ref) plugins are not supported in local mode"
        )),
        None => Err(anyhow!("fixture description has no plugin")),
    }
}

/// Sum resource pools into the `{cpu_mili, memory_bytes}` capacity. Resource
/// names are matched leniently until the budget model generalizes.
fn capacity_from_pools(pools: &[pb::ResourcePool]) -> Capacity {
    let mut cap = Capacity::zero();
    for pool in pools {
        for amount in &pool.resources {
            match amount.name.as_str() {
                "cpu" | "cpu_mili" | "cpu_milli" => cap.cpu_mili += amount.quantity,
                "memory" | "memory_bytes" => cap.memory_bytes += amount.quantity,
                _ => {}
            }
        }
    }
    cap
}

#[async_trait]
impl FixtureFactory for GrpcPluginFactory {
    async fn estimate_capacity(
        &self,
        spec: &pb::FixtureDescription,
        profile_config: &[u8],
    ) -> Result<Capacity> {
        let mut client = self
            .manager
            .client(&spec.impl_id, binary_path(spec)?)
            .await?;
        let req = pb::CreateRequest {
            impl_id: spec.impl_id.clone(),
            inputs: spec.inputs.clone(),
            metadata: None,
            profile_config: profile_config.to_vec(),
        };
        let requests = client.get_resource_requests(req).await?.into_inner();
        Ok(capacity_from_pools(&requests.consumes))
    }

    async fn create(
        &self,
        spec: &pb::FixtureDescription,
        metadata: &CreateMetadata,
        profile_config: &[u8],
    ) -> Result<CreateOutput> {
        let mut client = self
            .manager
            .client(&spec.impl_id, binary_path(spec)?)
            .await?;
        let req = pb::CreateRequest {
            impl_id: spec.impl_id.clone(),
            inputs: spec.inputs.clone(),
            metadata: Some(pb::CreationMetadata {
                fixture_id: metadata.fixture_id.as_str().to_string(),
                client_id: metadata.client_id.clone(),
                owner: metadata.owner.clone().unwrap_or_default(),
                labels: Default::default(),
                attempt: metadata.attempt,
            }),
            profile_config: profile_config.to_vec(),
        };
        let resp = plugin_client::create_fixture(&mut client, req).await?;
        Ok(CreateOutput {
            instance_data: resp.instance_data,
            outputs: resp.outputs,
            consumed: capacity_from_pools(&resp.consumed),
            idle_timeout: Duration::from_secs(resp.idle_timeout_seconds.max(0) as u64),
            max_lifetime: (resp.max_lifetime_seconds > 0)
                .then(|| Duration::from_secs(resp.max_lifetime_seconds as u64)),
        })
    }

    async fn get_info(&self, impl_id: &str, instance_data: &[u8]) -> Result<FixtureInfo> {
        let mut client = self.manager.existing_client(impl_id).await?;
        let req = pb::GetFixtureInfoRequest {
            impl_id: impl_id.to_string(),
            instance_data: instance_data.to_vec(),
        };
        let resp = plugin_client::get_fixture_info(&mut client, req).await?;
        Ok(FixtureInfo {
            status: pb::InstanceStatus::try_from(resp.status)
                .unwrap_or(pb::InstanceStatus::Unknown),
            outputs: resp.outputs,
            messages: resp.messages,
            consumed: capacity_from_pools(&resp.consumed),
        })
    }

    async fn destroy(&self, impl_id: &str, instance_data: &[u8]) -> Result<()> {
        let mut client = self.manager.existing_client(impl_id).await?;
        client
            .destroy_fixture(pb::DestroyRequest {
                impl_id: impl_id.to_string(),
                instance_data: instance_data.to_vec(),
            })
            .await?;
        Ok(())
    }

    async fn issue_access_credentials(
        &self,
        impl_id: &str,
        instance_data: &[u8],
        validity: Duration,
    ) -> Result<IssuedCredentials> {
        let mut client = self.manager.existing_client(impl_id).await?;
        let req = pb::IssueAccessCredentialsRequest {
            impl_id: impl_id.to_string(),
            instance_data: instance_data.to_vec(),
            validity_seconds: validity.as_secs() as i64,
        };
        match client.issue_access_credentials(req).await {
            Ok(resp) => {
                let resp = resp.into_inner();
                Ok(IssuedCredentials {
                    credentials: resp.credentials,
                    handle: resp.issued_handle,
                })
            }
            // The plugin issues no credentials: treat as empty (invariant 4).
            Err(status) if status.code() == tonic::Code::Unimplemented => {
                Ok(IssuedCredentials::default())
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn release_access_credentials(
        &self,
        impl_id: &str,
        instance_data: &[u8],
        handle: &[u8],
    ) -> Result<()> {
        let mut client = self.manager.existing_client(impl_id).await?;
        let req = pb::ReleaseAccessCredentialsRequest {
            impl_id: impl_id.to_string(),
            instance_data: instance_data.to_vec(),
            issued_handle: handle.to_vec(),
        };
        match client.release_access_credentials(req).await {
            Ok(_) => Ok(()),
            Err(status) if status.code() == tonic::Code::Unimplemented => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
