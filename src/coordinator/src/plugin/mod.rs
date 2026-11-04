//! The coordinator's seam to fixture provisioning. The allocator depends only
//! on this trait; the real implementation is the gRPC plugin client (added
//! with the process manager), and tests use an in-process fake.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use systemtest_proto::systemtest::v1 as pb;

use crate::model::budget::Capacity;
use crate::model::fixture::FixtureId;

pub mod grpc;
pub mod process;

#[cfg(test)]
pub(crate) mod testing;

/// Metadata handed to the plugin when creating a fixture.
#[derive(Clone, Debug)]
pub struct CreateMetadata {
    pub fixture_id: FixtureId,
    pub client_id: String,
    pub owner: Option<String>,
    pub attempt: u32,
}

/// What a plugin returns once creation is initiated.
#[derive(Clone, Debug)]
pub struct CreateOutput {
    pub instance_data: Vec<u8>,
    pub outputs: BTreeMap<String, pb::Value>,
    pub consumed: Capacity,
    pub idle_timeout: Duration,
    /// Absolute lifetime bound requested by the plugin, if any.
    pub max_lifetime: Option<Duration>,
}

/// A fixture's current status as reported by `GetFixtureInfo`.
#[derive(Clone, Debug)]
pub struct FixtureInfo {
    pub status: pb::InstanceStatus,
    pub outputs: BTreeMap<String, pb::Value>,
    pub messages: Vec<String>,
    pub consumed: Capacity,
}

/// Abstracts how fixtures are provisioned. Implementations must not
/// over-estimate capacity (under-estimating is allowed).
#[async_trait]
pub trait FixtureFactory: Send + Sync {
    /// Guess the resources a fixture of `spec` will need.
    async fn estimate_capacity(
        &self,
        spec: &pb::FixtureDescription,
        profile_config: &[u8],
    ) -> Result<Capacity>;

    /// Initiate creation, returning as soon as it is underway.
    async fn create(
        &self,
        spec: &pb::FixtureDescription,
        metadata: &CreateMetadata,
        profile_config: &[u8],
    ) -> Result<CreateOutput>;

    /// Re-derive a fixture's status and outputs.
    async fn get_info(&self, impl_id: &str, instance_data: &[u8]) -> Result<FixtureInfo>;

    /// Tear a fixture down.
    async fn destroy(&self, impl_id: &str, instance_data: &[u8]) -> Result<()>;
}
