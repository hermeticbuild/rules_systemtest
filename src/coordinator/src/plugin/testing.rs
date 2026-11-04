//! In-process [`FixtureFactory`] fake for allocator tests. No plugin process.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use systemtest_proto::systemtest::v1 as pb;

use crate::model::budget::Capacity;
use crate::plugin::{CreateMetadata, CreateOutput, FixtureFactory, FixtureInfo};

/// Configurable fake. Records create/destroy counts so tests can assert on
/// provisioning side effects.
pub(crate) struct FakeFactory {
    pub estimate: Capacity,
    /// Status every `get_info` reports.
    pub info_status: pb::InstanceStatus,
    pub info_consumed: Capacity,
    pub fail_create: bool,
    pub fail_destroy: bool,
    counts: Mutex<Counts>,
}

#[derive(Default, Clone)]
pub(crate) struct Counts {
    pub created: usize,
    pub destroyed: usize,
}

impl FakeFactory {
    /// A fake whose fixtures come up `Running` with the given estimate.
    pub fn running(estimate: Capacity) -> Self {
        FakeFactory {
            info_consumed: estimate.clone(),
            estimate,
            info_status: pb::InstanceStatus::InstanceRunning,
            fail_create: false,
            fail_destroy: false,
            counts: Mutex::new(Counts::default()),
        }
    }

    /// Like [`running`], but every `destroy` call fails (simulating a transient
    /// teardown error).
    pub fn failing_destroy(estimate: Capacity) -> Self {
        FakeFactory {
            fail_destroy: true,
            ..Self::running(estimate)
        }
    }

    pub fn counts(&self) -> Counts {
        self.counts.lock().unwrap().clone()
    }
}

#[async_trait]
impl FixtureFactory for FakeFactory {
    async fn estimate_capacity(
        &self,
        _spec: &pb::FixtureDescription,
        _profile_config: &[u8],
    ) -> Result<Capacity> {
        Ok(self.estimate.clone())
    }

    async fn create(
        &self,
        _spec: &pb::FixtureDescription,
        _metadata: &CreateMetadata,
        _profile_config: &[u8],
    ) -> Result<CreateOutput> {
        if self.fail_create {
            return Err(anyhow!("fake create failure"));
        }
        self.counts.lock().unwrap().created += 1;
        Ok(CreateOutput {
            instance_data: b"fake".to_vec(),
            outputs: BTreeMap::new(),
            consumed: self.estimate.clone(),
            idle_timeout: Duration::from_secs(60),
            max_lifetime: None,
        })
    }

    async fn get_info(&self, _impl_id: &str, _instance_data: &[u8]) -> Result<FixtureInfo> {
        Ok(FixtureInfo {
            status: self.info_status,
            outputs: BTreeMap::new(),
            messages: vec![],
            consumed: self.info_consumed.clone(),
        })
    }

    async fn destroy(&self, _impl_id: &str, _instance_data: &[u8]) -> Result<()> {
        if self.fail_destroy {
            return Err(anyhow!("fake destroy failure"));
        }
        self.counts.lock().unwrap().destroyed += 1;
        Ok(())
    }
}
