//! Creator task: assigns leases to fixtures, provisions what's missing
//! and reclaims capacity under pressure.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{debug, error, info, warn};

use crate::allocator::PeriodicTask;
use crate::allocator::lease_updater::{
    FixtureAction, LeaseDecision, Outcome, WorkingSet, advance_leases,
};
use crate::credentials::{CredentialStore, IssuedForLease, IssuedHandle};
use crate::model::budget::{Capacity, Quota, QuotaError};
use crate::model::fixture::{FixtureFingerprint, FixtureId, FixtureState};
use crate::model::lease::{LeaseFixtureStatus, LeaseId, LeaseState};
use crate::model::settings::{ProfileId, Settings};
use crate::plugin::{CreateMetadata, FixtureFactory};
use crate::store::{Mutation, Store};
use systemtest_proto::systemtest::v1 as pb;

/// Validity requested when issuing per-consumer credentials. Must outlast the
/// lease; the plugin reclaims on lapse.
const CREDENTIAL_VALIDITY: Duration = Duration::from_secs(3600);

pub struct Creator {
    store: Arc<dyn Store>,
    factory: Arc<dyn FixtureFactory>,
    credentials: Arc<CredentialStore>,
    settings: Settings,
    now: fn() -> SystemTime,
}

impl std::fmt::Debug for Creator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Creator").finish()
    }
}

impl Creator {
    pub fn new(
        store: Arc<dyn Store>,
        factory: Arc<dyn FixtureFactory>,
        credentials: Arc<CredentialStore>,
        settings: Settings,
        now: fn() -> SystemTime,
    ) -> Self {
        Creator {
            store,
            factory,
            credentials,
            settings,
            now,
        }
    }

    pub async fn run_once(&self) -> Result<()> {
        // A single profile (default). Multi-profile budgeting lands with the
        // budget generalization.
        let profile_id = ProfileId::default_id();
        let profile = self
            .settings
            .profile(&profile_id)
            .context("default profile not configured")?
            .clone();
        let profile_config = profile.config.clone().into_bytes();

        let fingerprints = self.store.groups_needing_work().await?;
        if fingerprints.is_empty() {
            return Ok(());
        }

        // Load each group and pre-fetch its capacity estimate (the only factory
        // I/O before the pure pass).
        let mut groups = HashMap::new();
        let mut estimates: HashMap<FixtureFingerprint, Capacity> = HashMap::new();
        for fp in fingerprints {
            let group = self.store.group(&fp).await?;
            if let Some(spec) = group
                .leases
                .values()
                .map(|l| &l.spec)
                .chain(group.fixtures.values().map(|f| &f.spec))
                .next()
            {
                let estimate = match self.factory.estimate_capacity(spec, &profile_config).await {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(
                            "estimate_capacity failed for {}: {e:#}; assuming zero",
                            fp.as_str()
                        );
                        Capacity::zero()
                    }
                };
                estimates.insert(fp.clone(), estimate);
            }
            groups.insert(fp, group);
        }

        let used = self.store.budget_usage(&profile_id).await?;
        let (quota, already_over) = match Quota::new(profile.budget.clone(), &used) {
            Ok(q) => (q, Capacity::zero()),
            // Already over budget (e.g. config change); switch straight to reclaim.
            Err(QuotaError::Exceeded { by }) => (
                Quota::new(profile.budget.clone(), &Capacity::zero())
                    .expect("budget minus nothing is within budget"),
                by,
            ),
        };

        // Index leases so decisions can be executed against their spec.
        let leases_by_id: HashMap<LeaseId, LeaseState> = groups
            .values()
            .flat_map(|g| g.leases.iter().map(|(id, l)| (id.clone(), l.clone())))
            .collect();

        let mut ws = WorkingSet {
            groups,
            quota,
            estimates,
        };
        let outcome = if already_over.is_zero() {
            advance_leases(&mut ws, profile.failed_fixtures_to_fail_lease_threshold)
        } else {
            Outcome {
                decisions: HashMap::new(),
                capacity_pressure: already_over,
            }
        };

        let pressure = outcome.capacity_pressure.clone();
        self.execute_decisions(outcome.decisions, &leases_by_id, &profile_config, &profile)
            .await;

        if !pressure.is_zero() {
            info!("capacity pressure {pressure}; reclaiming unassigned fixtures");
            self.reclaim(&ws, &pressure).await;
        }
        Ok(())
    }

    async fn execute_decisions(
        &self,
        decisions: HashMap<LeaseId, LeaseDecision>,
        leases_by_id: &HashMap<LeaseId, LeaseState>,
        profile_config: &[u8],
        profile: &crate::model::settings::ProfileSettings,
    ) {
        for (lease_id, decision) in decisions {
            let Some(lease) = leases_by_id.get(&lease_id) else {
                continue;
            };
            // Issue credentials once the fixture's namespace exists — on
            // readiness (so Status→SUCCEEDED carries them) and also on failure,
            // so the client can pull logs/status to debug the broken fixture.
            let became_terminal = matches!(
                decision.status,
                LeaseFixtureStatus::Ready | LeaseFixtureStatus::Error { .. }
            );
            let result = match decision.action {
                None => {
                    self.store
                        .apply(&[set_status(&lease_id, decision.status)])
                        .await
                }
                Some(FixtureAction::Assign { fixtures }) => {
                    let mut muts: Vec<Mutation> = fixtures
                        .into_iter()
                        .map(|f| Mutation::Assign {
                            lease: lease_id.clone(),
                            fixture: f,
                        })
                        .collect();
                    muts.push(set_status(&lease_id, decision.status));
                    self.store.apply(&muts).await
                }
                Some(FixtureAction::UnassignAll) => {
                    self.store
                        .apply(&[
                            Mutation::ClearAssignments(lease_id.clone()),
                            set_status(&lease_id, decision.status),
                        ])
                        .await
                }
                Some(FixtureAction::Create {
                    fixture_id,
                    estimate,
                }) => {
                    self.create_saga(
                        lease,
                        fixture_id,
                        estimate,
                        decision.status,
                        profile_config,
                        profile,
                    )
                    .await
                }
            };
            if let Err(e) = result {
                error!("advancing lease {}: {e:#}", lease_id.as_str());
                continue;
            }
            // Issue per-consumer credentials once, as the lease reaches a
            // terminal state. issue_credentials_for no-ops when the lease has no
            // created fixture (e.g. a create-time failure left no namespace).
            if became_terminal {
                self.issue_credentials_for(lease).await;
            }
        }
    }

    /// Issue per-consumer access material for a ready lease and hold it in the
    /// credential store. Idempotent; tolerant of plugins that issue nothing.
    async fn issue_credentials_for(&self, lease: &LeaseState) {
        if self.credentials.contains(&lease.id) {
            return;
        }
        let fixtures = self
            .store
            .fixtures_for_lease(&lease.id)
            .await
            .unwrap_or_default();
        let mut credentials = BTreeMap::new();
        let mut handles = Vec::new();
        for fixture in fixtures {
            match self
                .factory
                .issue_access_credentials(
                    &fixture.spec.impl_id,
                    &fixture.instance_data,
                    CREDENTIAL_VALIDITY,
                )
                .await
            {
                Ok(issued) => {
                    credentials.extend(issued.credentials);
                    if !issued.handle.is_empty() {
                        handles.push(IssuedHandle {
                            impl_id: fixture.spec.impl_id.clone(),
                            instance_data: fixture.instance_data.clone(),
                            handle: issued.handle,
                        });
                    }
                }
                Err(e) => warn!("issuing credentials for lease {}: {e:#}", lease.id.as_str()),
            }
        }
        self.credentials.put(
            lease.id.clone(),
            IssuedForLease {
                credentials,
                handles,
            },
        );
    }

    /// Persist a `Creating` fixture record, then provision it, then wire it to
    /// the lease. The record exists before the side effect, so a crash recovers.
    async fn create_saga(
        &self,
        lease: &LeaseState,
        fixture_id: FixtureId,
        estimate: Capacity,
        status: LeaseFixtureStatus,
        profile_config: &[u8],
        profile: &crate::model::settings::ProfileSettings,
    ) -> Result<()> {
        let now = (self.now)();
        let mut record = FixtureState {
            id: fixture_id.clone(),
            spec: lease.spec.clone(),
            fingerprint: lease.fingerprint.clone(),
            status: pb::InstanceStatus::InstanceStarting,
            outputs: Default::default(),
            capacity_consumed: estimate,
            messages: vec![],
            instance_data: vec![],
            created_at: now,
            expires_at: now + profile.fixtures_max_lifetime(),
        };
        // 1. Persist intent before the side effect.
        self.store
            .apply(&[Mutation::UpsertFixture(Box::new(record.clone()))])
            .await
            .context("persisting Creating record")?;

        // 2. Provision, with a single retry.
        let meta = |attempt| CreateMetadata {
            fixture_id: fixture_id.clone(),
            client_id: String::new(),
            owner: lease.owning_user_name.clone(),
            attempt,
        };
        let mut result = self
            .factory
            .create(&lease.spec, &meta(0), profile_config)
            .await;
        if result.is_err() {
            result = self
                .factory
                .create(&lease.spec, &meta(1), profile_config)
                .await;
        }

        match result {
            Ok(output) => {
                record.instance_data = output.instance_data;
                if !output.outputs.is_empty() {
                    record.outputs = output.outputs;
                }
                if !output.consumed.is_zero() {
                    record.capacity_consumed = output.consumed;
                }
                if let Some(max_lifetime) = output.max_lifetime {
                    record.expires_at = now + max_lifetime;
                }
                debug!(fixture = fixture_id.as_str(), "created");
                self.store
                    .apply(&[
                        Mutation::UpsertFixture(Box::new(record)),
                        Mutation::Assign {
                            lease: lease.id.clone(),
                            fixture: fixture_id,
                        },
                        set_status(&lease.id, status),
                    ])
                    .await
            }
            Err(e) => {
                // 3. Roll back the record and fail the lease.
                self.store
                    .apply(&[
                        Mutation::DeleteFixture(fixture_id),
                        set_status(
                            &lease.id,
                            LeaseFixtureStatus::Error {
                                message: Some(format!("can't create fixture: {e:#}")),
                            },
                        ),
                    ])
                    .await
            }
        }
    }

    /// Delete unassigned, non-`Starting` fixtures (errored first) until the
    /// reclaimed capacity covers `to_reclaim`.
    async fn reclaim(&self, ws: &WorkingSet, to_reclaim: &Capacity) {
        let mut candidates: Vec<&FixtureState> = ws
            .groups
            .values()
            .flat_map(|g| g.unassigned_fixtures())
            .filter(|f| f.status != pb::InstanceStatus::InstanceStarting)
            .collect();
        // Errored fixtures first, then the rest.
        candidates.sort_by_key(|f| f.status != pb::InstanceStatus::InstanceError);

        let mut reclaimed = Capacity::zero();
        for fixture in candidates {
            if !to_reclaim.any_greater(&reclaimed) {
                break;
            }
            if let Err(e) = self
                .factory
                .destroy(&fixture.spec.impl_id, &fixture.instance_data)
                .await
            {
                // Teardown failed: keep the record (and the capacity it holds)
                // so a later pass retries, rather than orphaning the resource.
                // Don't credit `reclaimed` — nothing was actually freed.
                warn!(
                    "destroying fixture {} during reclaim: {e:#}; keeping record to retry",
                    fixture.id.as_str()
                );
                continue;
            }
            if let Err(e) = self
                .store
                .apply(&[Mutation::DeleteFixture(fixture.id.clone())])
                .await
            {
                error!("deleting reclaimed fixture {}: {e:#}", fixture.id.as_str());
                continue;
            }
            // Capacity is freed only now that the record is gone.
            reclaimed += &fixture.capacity_consumed;
        }
    }
}

fn set_status(lease: &LeaseId, status: LeaseFixtureStatus) -> Mutation {
    Mutation::SetLeaseFixtureStatus {
        lease: lease.clone(),
        status,
    }
}

#[async_trait]
impl PeriodicTask for Creator {
    async fn run(&self) -> Result<()> {
        self.run_once().await
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::model::settings::Settings;
    use crate::model::testing::fake_lease;
    use crate::plugin::testing::FakeFactory;
    use crate::store::memory::InMemoryStore;

    fn settings() -> Settings {
        Settings::default_local()
    }

    #[tokio::test]
    async fn creates_and_assigns_a_fixture_for_a_new_lease() {
        let store = Arc::new(InMemoryStore::new());
        let lease = fake_lease();
        let lease_id = lease.id.clone();
        store
            .apply(&[Mutation::CreateLease(Box::new(lease))])
            .await
            .unwrap();

        let factory = Arc::new(FakeFactory::running(Capacity {
            cpu_mili: 100,
            memory_bytes: 1024,
        }));
        let creator = Creator::new(
            store.clone(),
            factory.clone(),
            Arc::new(CredentialStore::new()),
            settings(),
            || UNIX_EPOCH + Duration::from_secs(10),
        );

        creator.run_once().await.unwrap();

        assert_eq!(factory.counts().created, 1);
        let fixtures = store.fixtures_for_lease(&lease_id).await.unwrap();
        assert_eq!(fixtures.len(), 1, "lease should have one fixture assigned");
        let lease = store.lease(&lease_id).await.unwrap().unwrap();
        assert!(matches!(
            lease.fixture_status,
            LeaseFixtureStatus::Creating { .. }
        ));
    }

    #[tokio::test]
    async fn create_failure_fails_the_lease_and_leaves_no_fixture() {
        let store = Arc::new(InMemoryStore::new());
        let lease = fake_lease();
        let lease_id = lease.id.clone();
        store
            .apply(&[Mutation::CreateLease(Box::new(lease))])
            .await
            .unwrap();

        let mut factory = FakeFactory::running(Capacity {
            cpu_mili: 100,
            memory_bytes: 1024,
        });
        factory.fail_create = true;
        let creator = Creator::new(
            store.clone(),
            Arc::new(factory),
            Arc::new(CredentialStore::new()),
            settings(),
            || UNIX_EPOCH + Duration::from_secs(10),
        );

        creator.run_once().await.unwrap();

        assert!(
            store
                .fixtures_for_lease(&lease_id)
                .await
                .unwrap()
                .is_empty()
        );
        let lease = store.lease(&lease_id).await.unwrap().unwrap();
        assert!(matches!(
            lease.fixture_status,
            LeaseFixtureStatus::Error { .. }
        ));
    }
}
