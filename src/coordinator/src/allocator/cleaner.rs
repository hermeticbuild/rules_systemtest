//! Cleaner task: deletes leases past their keepalive/max-lifetime, and
//! destroys unassigned fixtures past their `expires_at`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, warn};

use crate::allocator::PeriodicTask;
use crate::plugin::FixtureFactory;
use crate::store::{Mutation, Store};

pub struct Cleaner {
    store: Arc<dyn Store>,
    factory: Arc<dyn FixtureFactory>,
    now: fn() -> SystemTime,
    lease_timeout: Duration,
}

impl std::fmt::Debug for Cleaner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cleaner")
            .field("lease_timeout", &self.lease_timeout)
            .finish()
    }
}

impl Cleaner {
    pub fn new(
        store: Arc<dyn Store>,
        factory: Arc<dyn FixtureFactory>,
        now: fn() -> SystemTime,
        lease_timeout: Duration,
    ) -> Self {
        Cleaner {
            store,
            factory,
            now,
            lease_timeout,
        }
    }

    pub async fn run_once(&self) -> Result<()> {
        let now = (self.now)();

        // Expired leases: delete the lease (its fixtures become unassigned and
        // are reclaimed/expired separately; tearing them down here would need
        // their external resources freed too).
        let due_leases = self.store.leases_due(now, self.lease_timeout).await?;
        if !due_leases.is_empty() {
            debug!(count = due_leases.len(), "deleting expired leases");
            let muts: Vec<Mutation> = due_leases.into_iter().map(Mutation::DeleteLease).collect();
            self.store.apply(&muts).await?;
        }

        // Expired, unassigned fixtures: destroy, then delete only if teardown
        // succeeded. A failed destroy keeps the record (and its `instance_data`)
        // so a later pass retries, rather than orphaning the external resource
        // on a transient error.
        for fixture in self.store.fixtures_due(now).await? {
            debug!(fixture = fixture.id.as_str(), "destroying expired fixture");
            if let Err(e) = self
                .factory
                .destroy(&fixture.spec.impl_id, &fixture.instance_data)
                .await
            {
                warn!(
                    "destroying fixture {}: {e:#}; keeping record to retry",
                    fixture.id.as_str()
                );
                continue;
            }
            self.store
                .apply(&[Mutation::DeleteFixture(fixture.id)])
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl PeriodicTask for Cleaner {
    async fn run(&self) -> Result<()> {
        self.run_once().await
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::model::budget::Capacity;
    use crate::model::lease::LeaseState;
    use crate::model::testing::{fake_fixture, fake_lease};
    use crate::plugin::testing::FakeFactory;
    use crate::store::memory::InMemoryStore;

    fn cleaner(store: Arc<dyn Store>, now: fn() -> SystemTime, lease_timeout: Duration) -> Cleaner {
        Cleaner::new(
            store,
            Arc::new(FakeFactory::running(Capacity::zero())),
            now,
            lease_timeout,
        )
    }

    #[tokio::test]
    async fn deletes_leases_past_max_lifetime() {
        let store = Arc::new(InMemoryStore::new());
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let fresh = LeaseState {
            created_at: now - Duration::from_secs(1),
            ..fake_lease()
        };
        let old = LeaseState {
            created_at: now - Duration::from_secs(50),
            ..fake_lease()
        };
        let fresh_id = fresh.id.clone();
        let old_id = old.id.clone();
        store
            .apply(&[
                Mutation::CreateLease(Box::new(fresh)),
                Mutation::CreateLease(Box::new(old)),
            ])
            .await
            .unwrap();

        cleaner(
            store.clone(),
            || UNIX_EPOCH + Duration::from_secs(100),
            Duration::from_secs(10),
        )
        .run_once()
        .await
        .unwrap();

        assert!(
            store.lease(&old_id).await.unwrap().is_none(),
            "old lease deleted"
        );
        assert!(
            store.lease(&fresh_id).await.unwrap().is_some(),
            "fresh lease kept"
        );
    }

    #[tokio::test]
    async fn destroys_expired_unassigned_fixture() {
        let store = Arc::new(InMemoryStore::new());
        let mut f = fake_fixture();
        f.expires_at = UNIX_EPOCH + Duration::from_secs(10);
        store
            .apply(&[Mutation::UpsertFixture(Box::new(f))])
            .await
            .unwrap();

        let factory = Arc::new(FakeFactory::running(Capacity::zero()));
        let cleaner = Cleaner::new(
            store.clone(),
            factory.clone(),
            || UNIX_EPOCH + Duration::from_secs(20),
            Duration::from_secs(3600),
        );
        cleaner.run_once().await.unwrap();

        assert_eq!(factory.counts().destroyed, 1);
        assert!(
            store
                .fixtures_due(UNIX_EPOCH + Duration::from_secs(20))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_destroy_keeps_fixture_for_retry() {
        let store = Arc::new(InMemoryStore::new());
        let mut f = fake_fixture();
        f.expires_at = UNIX_EPOCH + Duration::from_secs(10);
        store
            .apply(&[Mutation::UpsertFixture(Box::new(f))])
            .await
            .unwrap();

        let factory = Arc::new(FakeFactory::failing_destroy(Capacity::zero()));
        let cleaner = Cleaner::new(
            store.clone(),
            factory,
            || UNIX_EPOCH + Duration::from_secs(20),
            Duration::from_secs(3600),
        );

        // A transient teardown failure must not orphan the resource: the record
        // stays so the next pass retries, rather than being deleted anyway.
        cleaner.run_once().await.unwrap();

        assert!(
            !store
                .fixtures_due(UNIX_EPOCH + Duration::from_secs(20))
                .await
                .unwrap()
                .is_empty(),
            "fixture record kept after failed destroy"
        );
    }
}
