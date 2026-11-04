//! Poller task: refreshes each live fixture's status/outputs via
//! `GetFixtureInfo`, and fails fixtures that time out while starting.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, warn};

use crate::allocator::PeriodicTask;
use crate::model::fixture::FixtureState;
use crate::plugin::FixtureFactory;
use crate::store::{Mutation, Store};
use systemtest_proto::systemtest::v1 as pb;

/// Fallback start timeout when a description leaves `startup_timeout_seconds` 0.
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(300);

pub struct Poller {
    store: Arc<dyn Store>,
    factory: Arc<dyn FixtureFactory>,
    now: fn() -> SystemTime,
}

impl std::fmt::Debug for Poller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Poller").finish()
    }
}

impl Poller {
    pub fn new(
        store: Arc<dyn Store>,
        factory: Arc<dyn FixtureFactory>,
        now: fn() -> SystemTime,
    ) -> Self {
        Poller {
            store,
            factory,
            now,
        }
    }

    pub async fn run_once(&self) -> Result<()> {
        let fixtures = self.store.pollable_fixtures().await?;
        let mut updates = Vec::new();
        let mut first_err: Option<anyhow::Error> = None;
        let now = (self.now)();

        for mut fixture in fixtures {
            let mut changed = false;
            match self
                .factory
                .get_info(&fixture.spec.impl_id, &fixture.instance_data)
                .await
            {
                Ok(info) => {
                    changed |= apply_info(&mut fixture, info);
                }
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
            if timed_out_starting(&fixture, now) {
                warn!(fixture = fixture.id.as_str(), "timed out starting; failing");
                fail(
                    &mut fixture,
                    "timed out waiting for fixture to become healthy",
                );
                changed = true;
            }
            if changed {
                debug!(fixture = fixture.id.as_str(), status = ?fixture.status, "updated");
                updates.push(Mutation::UpsertFixture(Box::new(fixture)));
            }
        }

        if !updates.is_empty() {
            self.store.apply(&updates).await?;
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Merge a `GetFixtureInfo` result into the record; returns whether anything
/// changed. `Error` is terminal: only capacity keeps updating.
fn apply_info(fixture: &mut FixtureState, info: crate::plugin::FixtureInfo) -> bool {
    let mut changed = false;
    if !info.consumed.is_zero() && fixture.capacity_consumed != info.consumed {
        fixture.capacity_consumed = info.consumed;
        changed = true;
    }
    if fixture.status == pb::InstanceStatus::InstanceError {
        return changed;
    }
    if fixture.status != info.status {
        fixture.status = info.status;
        changed = true;
    }
    if fixture.outputs != info.outputs {
        fixture.outputs = info.outputs;
        changed = true;
    }
    if fixture.messages != info.messages {
        fixture.messages = info.messages;
        changed = true;
    }
    changed
}

fn timed_out_starting(fixture: &FixtureState, now: SystemTime) -> bool {
    if fixture.status != pb::InstanceStatus::InstanceStarting {
        return false;
    }
    let timeout = match fixture.spec.startup_timeout_seconds {
        n if n > 0 => Duration::from_secs(n as u64),
        _ => DEFAULT_STARTUP_TIMEOUT,
    };
    now.duration_since(fixture.created_at)
        .map(|waited| waited > timeout)
        .unwrap_or(false)
}

fn fail(fixture: &mut FixtureState, message: &str) {
    fixture.status = pb::InstanceStatus::InstanceError;
    fixture.messages.push(message.to_string());
}

#[async_trait]
impl PeriodicTask for Poller {
    async fn run(&self) -> Result<()> {
        self.run_once().await
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::model::budget::Capacity;
    use crate::model::testing::fake_fixture;
    use crate::plugin::testing::FakeFactory;
    use crate::store::memory::InMemoryStore;

    #[tokio::test]
    async fn poll_transitions_starting_to_running() {
        let store = Arc::new(InMemoryStore::new());
        let fixture = fake_fixture(); // Starting
        let id = fixture.id.clone();
        store
            .apply(&[Mutation::UpsertFixture(Box::new(fixture))])
            .await
            .unwrap();
        let factory = Arc::new(FakeFactory::running(Capacity::zero()));
        let poller = Poller::new(store.clone(), factory, || UNIX_EPOCH);

        poller.run_once().await.unwrap();

        let fixtures = store.pollable_fixtures().await.unwrap();
        assert_eq!(fixtures.len(), 1);
        assert_eq!(fixtures[0].status, pb::InstanceStatus::InstanceRunning);
        assert_eq!(fixtures[0].id, id);
    }

    #[tokio::test]
    async fn starting_fixture_times_out() {
        let store = Arc::new(InMemoryStore::new());
        let mut fixture = fake_fixture();
        fixture.status = pb::InstanceStatus::InstanceStarting;
        fixture.created_at = UNIX_EPOCH;
        fixture.spec.startup_timeout_seconds = 10;
        store
            .apply(&[Mutation::UpsertFixture(Box::new(fixture))])
            .await
            .unwrap();
        // Factory keeps saying "starting"; the poller should time it out.
        let mut factory = FakeFactory::running(Capacity::zero());
        factory.info_status = pb::InstanceStatus::InstanceStarting;
        let poller = Poller::new(store.clone(), Arc::new(factory), || {
            UNIX_EPOCH + Duration::from_secs(20)
        });

        poller.run_once().await.unwrap();

        let fixtures = store.pollable_fixtures().await.unwrap();
        assert_eq!(fixtures[0].status, pb::InstanceStatus::InstanceError);
    }
}
