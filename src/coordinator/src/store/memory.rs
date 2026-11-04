//! In-memory [`Store`]. The allocator's working state for local mode and the
//! backend for tests. At current scale each scoped read is a scan over small maps;
//! the scans become indices / a budget counter behind these same methods later.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;

use crate::model::budget::Capacity;
use crate::model::fixture::{FixtureFingerprint, FixtureId, FixtureState};
use crate::model::group::FixtureGroup;
use crate::model::lease::{LeaseId, LeaseState};
use crate::model::settings::ProfileId;
use crate::store::{Mutation, Store};

/// The whole coordinator state. Mutated only through [`apply`](World::apply).
#[derive(Default)]
pub struct World {
    pub leases: HashMap<LeaseId, LeaseState>,
    pub fixtures: HashMap<FixtureId, FixtureState>,
    pub assignments: HashMap<LeaseId, HashSet<FixtureId>>,
}

impl World {
    fn apply_one(&mut self, m: Mutation) {
        match m {
            Mutation::CreateLease(lease) => {
                self.leases.insert(lease.id.clone(), *lease);
            }
            Mutation::SetLeaseFixtureStatus { lease, status } => {
                if let Some(l) = self.leases.get_mut(&lease) {
                    l.fixture_status = status;
                }
            }
            Mutation::SetKeepalive { lease, at } => {
                if let Some(l) = self.leases.get_mut(&lease) {
                    l.last_keepalive_at = Some(at);
                }
            }
            Mutation::DeleteLease(lease) => {
                self.leases.remove(&lease);
                self.assignments.remove(&lease);
            }
            Mutation::UpsertFixture(fixture) => {
                self.fixtures.insert(fixture.id.clone(), *fixture);
            }
            Mutation::DeleteFixture(fixture) => {
                self.fixtures.remove(&fixture);
                for set in self.assignments.values_mut() {
                    set.remove(&fixture);
                }
            }
            Mutation::Assign { lease, fixture } => {
                self.assignments.entry(lease).or_default().insert(fixture);
            }
            Mutation::ClearAssignments(lease) => {
                self.assignments.remove(&lease);
            }
        }
    }
}

/// `Store` over an in-memory [`World`] behind a single lock. No method awaits
/// while holding the lock, so a plain `std::sync::Mutex` is correct.
pub struct InMemoryStore {
    inner: Mutex<World>,
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryStore {
    pub fn new() -> Self {
        InMemoryStore {
            inner: Mutex::new(World::default()),
        }
    }

    /// Seed with pre-built state (tests).
    pub fn seeded(world: World) -> Self {
        InMemoryStore {
            inner: Mutex::new(world),
        }
    }
}

#[async_trait]
impl Store for InMemoryStore {
    async fn groups_needing_work(&self) -> Result<Vec<FixtureFingerprint>> {
        let w = self.inner.lock().unwrap();
        let mut fps: HashSet<FixtureFingerprint> = HashSet::new();
        fps.extend(w.leases.values().map(|l| l.fingerprint.clone()));
        fps.extend(w.fixtures.values().map(|f| f.fingerprint.clone()));
        Ok(fps.into_iter().collect())
    }

    async fn group(&self, fingerprint: &FixtureFingerprint) -> Result<FixtureGroup> {
        let w = self.inner.lock().unwrap();
        let mut group = FixtureGroup::new(fingerprint.clone());
        for (id, fixture) in &w.fixtures {
            if fixture.fingerprint == *fingerprint {
                group.fixtures.insert(id.clone(), fixture.clone());
            }
        }
        for (id, lease) in &w.leases {
            if lease.fingerprint != *fingerprint {
                continue;
            }
            if let Some(assigned) = w.assignments.get(id) {
                let present: HashSet<FixtureId> = assigned
                    .iter()
                    .filter(|f| group.fixtures.contains_key(*f))
                    .cloned()
                    .collect();
                if !present.is_empty() {
                    group.assignments.insert(id.clone(), present);
                }
            }
            group.leases.insert(id.clone(), lease.clone());
        }
        Ok(group)
    }

    async fn pollable_fixtures(&self) -> Result<Vec<FixtureState>> {
        use systemtest_proto::systemtest::v1::InstanceStatus;
        let w = self.inner.lock().unwrap();
        Ok(w.fixtures
            .values()
            .filter(|f| {
                !matches!(
                    f.status,
                    InstanceStatus::InstanceDefunct | InstanceStatus::Unknown
                )
            })
            .cloned()
            .collect())
    }

    async fn leases_due(&self, now: SystemTime, max_lifetime: Duration) -> Result<Vec<LeaseId>> {
        let w = self.inner.lock().unwrap();
        let due = |before: SystemTime, max: Duration| {
            now.duration_since(before)
                .map(|d| d >= max)
                .unwrap_or(false)
        };
        Ok(w.leases
            .values()
            .filter(|l| {
                let aged_out = due(l.created_at, max_lifetime);
                let keepalive_lapsed = match (l.last_keepalive_at, l.keepalive_timeout) {
                    (Some(at), Some(timeout)) => due(at, timeout),
                    _ => false,
                };
                aged_out || keepalive_lapsed
            })
            .map(|l| l.id.clone())
            .collect())
    }

    async fn fixtures_due(&self, now: SystemTime) -> Result<Vec<FixtureState>> {
        let w = self.inner.lock().unwrap();
        let assigned: HashSet<&FixtureId> = w.assignments.values().flatten().collect();
        Ok(w.fixtures
            .values()
            .filter(|f| now >= f.expires_at && !assigned.contains(&f.id))
            .cloned()
            .collect())
    }

    async fn budget_usage(&self, profile: &ProfileId) -> Result<Capacity> {
        let w = self.inner.lock().unwrap();
        let mut total = Capacity::zero();
        for f in w.fixtures.values() {
            if ProfileId::resolve(&f.spec.profile_id) == *profile {
                total += &f.capacity_consumed;
            }
        }
        Ok(total)
    }

    async fn lease(&self, id: &LeaseId) -> Result<Option<LeaseState>> {
        let w = self.inner.lock().unwrap();
        Ok(w.leases.get(id).cloned())
    }

    async fn fixtures_for_lease(&self, id: &LeaseId) -> Result<Vec<FixtureState>> {
        let w = self.inner.lock().unwrap();
        let Some(assigned) = w.assignments.get(id) else {
            return Ok(vec![]);
        };
        Ok(assigned
            .iter()
            .filter_map(|fid| w.fixtures.get(fid).cloned())
            .collect())
    }

    async fn apply(&self, muts: &[Mutation]) -> Result<()> {
        let mut w = self.inner.lock().unwrap();
        for m in muts {
            w.apply_one(m.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::testing::{fake_fixture, fake_lease};

    #[tokio::test]
    async fn create_read_delete_lease() {
        let store = InMemoryStore::new();
        let lease = fake_lease();
        let id = lease.id.clone();
        store
            .apply(&[Mutation::CreateLease(Box::new(lease))])
            .await
            .unwrap();
        assert!(store.lease(&id).await.unwrap().is_some());
        store
            .apply(&[Mutation::DeleteLease(id.clone())])
            .await
            .unwrap();
        assert!(store.lease(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn group_bundles_matching_fingerprint() {
        let store = InMemoryStore::new();
        let lease = fake_lease();
        let fixture = fake_fixture();
        let fp = lease.fingerprint.clone();
        let lid = lease.id.clone();
        let fid = fixture.id.clone();
        store
            .apply(&[
                Mutation::CreateLease(Box::new(lease)),
                Mutation::UpsertFixture(Box::new(fixture)),
                Mutation::Assign {
                    lease: lid.clone(),
                    fixture: fid.clone(),
                },
            ])
            .await
            .unwrap();

        let group = store.group(&fp).await.unwrap();
        assert_eq!(group.leases.len(), 1);
        assert_eq!(group.fixtures.len(), 1);
        assert_eq!(group.assignments[&lid], HashSet::from([fid]));
    }

    #[tokio::test]
    async fn budget_usage_sums_fixtures() {
        let store = InMemoryStore::new();
        let mut f = fake_fixture();
        f.capacity_consumed = Capacity {
            cpu_mili: 100,
            memory_bytes: 2048,
        };
        store
            .apply(&[Mutation::UpsertFixture(Box::new(f))])
            .await
            .unwrap();
        let used = store.budget_usage(&ProfileId::default_id()).await.unwrap();
        assert_eq!(used.cpu_mili, 100);
        assert_eq!(used.memory_bytes, 2048);
    }

    #[tokio::test]
    async fn fixtures_due_respects_expiry() {
        let store = InMemoryStore::new();
        let mut f = fake_fixture();
        f.expires_at = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let fid = f.id.clone();
        store
            .apply(&[Mutation::UpsertFixture(Box::new(f))])
            .await
            .unwrap();

        let before = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        assert!(store.fixtures_due(before).await.unwrap().is_empty());
        let after = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let due = store.fixtures_due(after).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, fid);
    }

    #[tokio::test]
    async fn assigned_fixture_is_not_due_even_if_expired() {
        let store = InMemoryStore::new();
        let lease = fake_lease();
        let mut f = fake_fixture();
        f.expires_at = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let lid = lease.id.clone();
        let fid = f.id.clone();
        store
            .apply(&[
                Mutation::CreateLease(Box::new(lease)),
                Mutation::UpsertFixture(Box::new(f)),
                Mutation::Assign {
                    lease: lid,
                    fixture: fid,
                },
            ])
            .await
            .unwrap();
        let after = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        assert!(
            store.fixtures_due(after).await.unwrap().is_empty(),
            "a fixture with a live lease must not be due for deletion"
        );
    }

    #[tokio::test]
    async fn delete_fixture_clears_assignment() {
        let store = InMemoryStore::new();
        let lease = fake_lease();
        let fixture = fake_fixture();
        let lid = lease.id.clone();
        let fid = fixture.id.clone();
        store
            .apply(&[
                Mutation::CreateLease(Box::new(lease)),
                Mutation::UpsertFixture(Box::new(fixture)),
                Mutation::Assign {
                    lease: lid.clone(),
                    fixture: fid.clone(),
                },
                Mutation::DeleteFixture(fid),
            ])
            .await
            .unwrap();
        assert!(store.fixtures_for_lease(&lid).await.unwrap().is_empty());
    }
}
