//! The allocator: periodic reconcile tasks over the [`Store`].
//!
//! Fixed-interval sweep (minus image GC). Four independent tasks become three here — creator, poller, cleaner —
//! each with a consecutive-failure budget and a shared shutdown signal.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Error, Result};
use async_trait::async_trait;
use futures::{FutureExt, select};
use tracing::{error, warn};

use crate::model::settings::Settings;
use crate::plugin::FixtureFactory;
use crate::store::Store;
use crate::util::Condvar;

pub mod cleaner;
pub mod creator;
pub mod lease_updater;
pub mod poller;

use cleaner::Cleaner;
use creator::Creator;
use poller::Poller;

const MAX_SUBSEQUENT_FAILURES: u16 = 5;

/// One periodic reconcile task.
#[async_trait]
pub trait PeriodicTask: Send + Sync {
    async fn run(&self) -> Result<()>;
}

/// Owns the reconcile tasks and their cadences.
pub struct FixtureAllocator {
    creator: Creator,
    poller: Poller,
    cleaner: Cleaner,
    creator_interval: Duration,
    poller_interval: Duration,
    cleaner_interval: Duration,
}

impl FixtureAllocator {
    pub fn new(
        store: Arc<dyn Store>,
        factory: Arc<dyn FixtureFactory>,
        settings: Settings,
    ) -> Self {
        let now = SystemTime::now as fn() -> SystemTime;
        let alloc = settings.allocator.clone();
        FixtureAllocator {
            creator: Creator::new(store.clone(), factory.clone(), settings, now),
            poller: Poller::new(store.clone(), factory.clone(), now),
            cleaner: Cleaner::new(store, factory, now, alloc.lease_timeout()),
            creator_interval: alloc.creator_interval(),
            poller_interval: alloc.poller_interval(),
            cleaner_interval: alloc.cleaner_interval(),
        }
    }

    /// Runs all tasks until one exhausts its failure budget; on that, signals
    /// the others to drain and returns the error.
    pub async fn run(self) -> Result<()> {
        let stop = Arc::new(Condvar::new());
        let tasks = vec![
            tokio::spawn(task_loop(
                stop.clone(),
                self.creator,
                self.creator_interval,
                "creator",
            )),
            tokio::spawn(task_loop(
                stop.clone(),
                self.poller,
                self.poller_interval,
                "poller",
            )),
            tokio::spawn(task_loop(
                stop,
                self.cleaner,
                self.cleaner_interval,
                "cleaner",
            )),
        ];
        for joined in futures::future::join_all(tasks).await {
            joined??;
        }
        Ok(())
    }
}

async fn task_loop(
    stop: Arc<Condvar>,
    task: impl PeriodicTask,
    interval: Duration,
    name: &'static str,
) -> Result<()> {
    let mut subsequent_failures: u16 = 0;
    let mut ticker = tokio::time::interval(interval);
    loop {
        select! {
            _ = ticker.tick().fuse() => {}
            _ = stop.wait().fuse() => return Ok(()),
        }
        match task.run().await {
            Ok(()) => subsequent_failures = 0,
            Err(e) => {
                warn!("task {name} failed: {e:#}");
                if subsequent_failures > MAX_SUBSEQUENT_FAILURES {
                    error!("task {name} exceeded failure budget, stopping allocator");
                    stop.notify().await;
                    return Err::<(), Error>(e);
                }
                subsequent_failures += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;
    use crate::model::budget::Capacity;
    use crate::model::lease::LeaseFixtureStatus;
    use crate::model::testing::fake_lease;
    use crate::plugin::testing::FakeFactory;
    use crate::store::Mutation;
    use crate::store::memory::InMemoryStore;

    #[tokio::test]
    async fn create_poll_create_drives_lease_to_ready() {
        let store = Arc::new(InMemoryStore::new());
        let lease = fake_lease();
        let lease_id = lease.id.clone();
        store
            .apply(&[Mutation::CreateLease(Box::new(lease))])
            .await
            .unwrap();

        let factory = Arc::new(FakeFactory::running(Capacity {
            cpu_mili: 10,
            memory_bytes: 1024,
        }));
        let settings = Settings::default_local();
        let now = || UNIX_EPOCH + Duration::from_secs(10);

        let creator = Creator::new(store.clone(), factory.clone(), settings, now);
        let poller = Poller::new(store.clone(), factory.clone(), now);

        // Tick 1: creator provisions and assigns; lease -> Creating.
        creator.run_once().await.unwrap();
        // Tick 2: poller flips the fixture Starting -> Running.
        poller.run_once().await.unwrap();
        // Tick 3: creator advances Creating -> Ready.
        creator.run_once().await.unwrap();

        let lease = store.lease(&lease_id).await.unwrap().unwrap();
        assert_eq!(lease.fixture_status, LeaseFixtureStatus::Ready);
    }
}
