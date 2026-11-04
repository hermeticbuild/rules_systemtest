//! A `FixtureGroup` is the set of interchangeable fixtures sharing one
//! [`FixtureFingerprint`], together with the leases assigned to them. It is
//! not a quota pool.

use std::collections::{HashMap, HashSet};

use itertools::Itertools;
use systemtest_proto::systemtest::v1 as pb;
use tracing::{error, warn};

use crate::model::fixture::{
    FixtureFingerprint, FixtureId, FixtureState, max_concurrent_connections,
};
use crate::model::lease::{LeaseId, LeaseState, UseScope};

/// All leases and fixtures sharing one fingerprint, plus their assignments.
pub struct FixtureGroup {
    pub fingerprint: FixtureFingerprint,
    pub leases: HashMap<LeaseId, LeaseState>,
    pub fixtures: HashMap<FixtureId, FixtureState>,
    pub assignments: HashMap<LeaseId, HashSet<FixtureId>>,
}

/// Connection/scope accounting for one fixture (existing or hypothetical).
struct FixtureUsageInfo {
    use_scope: UseScope,
    connections_remaining: u32,
}

impl FixtureUsageInfo {
    fn new(slots: u32) -> Self {
        FixtureUsageInfo {
            use_scope: UseScope::Unrestricted,
            connections_remaining: slots,
        }
    }

    /// Reserve `connections`, or `false` if there aren't enough (leaving the
    /// usage unchanged).
    fn use_connections(&mut self, connections: u32) -> bool {
        if connections > self.connections_remaining {
            return false;
        }
        self.connections_remaining -= connections;
        true
    }
}

/// A fixture a lease could be assigned to: an existing one, or the Nth new one
/// this pass would create.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum AssignedFixture {
    Existing(FixtureId),
    New { id: usize },
}

impl AssignedFixture {
    /// Splits into the set of existing ids and the count of distinct new ones.
    pub fn split(fixtures: &[AssignedFixture]) -> (HashSet<FixtureId>, usize) {
        let mut existing = HashSet::new();
        let mut new = HashSet::new();
        for f in fixtures {
            match f {
                AssignedFixture::Existing(id) => {
                    existing.insert(id.clone());
                }
                AssignedFixture::New { id } => {
                    new.insert(*id);
                }
            }
        }
        (existing, new.len())
    }
}

impl FixtureGroup {
    pub fn new(fingerprint: FixtureFingerprint) -> Self {
        FixtureGroup {
            fingerprint,
            leases: HashMap::new(),
            fixtures: HashMap::new(),
            assignments: HashMap::new(),
        }
    }

    /// Current per-fixture utilization from existing assignments.
    fn fixture_usage(&self) -> HashMap<FixtureId, FixtureUsageInfo> {
        let mut usage: HashMap<FixtureId, FixtureUsageInfo> = self
            .fixtures
            .iter()
            .map(|(id, f)| {
                (
                    id.clone(),
                    FixtureUsageInfo::new(max_concurrent_connections(&f.spec)),
                )
            })
            .collect();

        for (lease_id, fixture_ids) in &self.assignments {
            let Some(lease) = self.leases.get(lease_id) else {
                continue;
            };
            for fixture_id in fixture_ids {
                let Some(info) = usage.get_mut(fixture_id) else {
                    continue;
                };
                if !info.use_connections(lease.connections_needed) {
                    warn!("fixture {fixture_id:?} oversubscribed by lease {lease_id:?}");
                }
                match info.use_scope.reduce(&lease.use_scope) {
                    Some(reduced) => info.use_scope = reduced.clone(),
                    None => warn!(
                        "fixture {fixture_id:?} assigned to lease {lease_id:?} with incompatible scope"
                    ),
                }
            }
        }
        usage
    }

    /// Merge `new` into the recorded assignments. Used mid-pass so later leases
    /// in the same tick don't double-book a fixture.
    pub fn update_assignments(&mut self, lease: LeaseId, fixtures: HashSet<FixtureId>) {
        self.assignments.entry(lease).or_default().extend(fixtures);
    }

    /// What new assignments are possible without violating `UseScope` or
    /// over-allocating connections. Each unassigned lease gets exactly one
    /// [`AssignedFixture`] (existing where one fits, else a fresh `New`).
    pub fn possible_assignments(&self) -> HashMap<LeaseId, Vec<AssignedFixture>> {
        let mut by_scope: HashMap<UseScope, Vec<(AssignedFixture, FixtureUsageInfo)>> = self
            .fixture_usage()
            .into_iter()
            .map(|(id, info)| (AssignedFixture::Existing(id), info))
            .into_group_map_by(|(_, info)| info.use_scope.clone());

        let mut result: HashMap<LeaseId, Vec<AssignedFixture>> = HashMap::new();
        let mut next_new_id = 0usize;

        for lease in self.unassigned_leases() {
            let bucket = by_scope.entry(lease.use_scope.clone()).or_default();
            let eligible = bucket.iter_mut().find(|(assigned, info)| {
                if info.connections_remaining < lease.connections_needed {
                    return false;
                }
                match assigned {
                    AssignedFixture::Existing(fixture_id) => {
                        match self.fixtures.get(fixture_id).map(|f| f.status) {
                            Some(pb::InstanceStatus::InstanceStarting)
                            | Some(pb::InstanceStatus::InstanceRunning) => true,
                            Some(_) => false,
                            None => {
                                error!(
                                    "lease {:?} eyeing fixture {fixture_id:?} missing from the group",
                                    lease.id
                                );
                                false
                            }
                        }
                    }
                    AssignedFixture::New { .. } => true,
                }
            });

            let (assigned, info) = match eligible {
                Some(found) => found,
                None => {
                    next_new_id += 1;
                    bucket.push((
                        AssignedFixture::New { id: next_new_id },
                        FixtureUsageInfo::new(max_concurrent_connections(&lease.spec)),
                    ));
                    bucket.last_mut().expect("just pushed")
                }
            };

            if !info.use_connections(lease.connections_needed) {
                // Can't happen: eligibility just checked there was room.
                error!("lease {:?} could not use connections after check", lease.id);
                continue;
            }
            result
                .entry(lease.id.clone())
                .or_default()
                .push(assigned.clone());
        }

        result
    }

    /// Possible assignments restricted to existing fixtures.
    pub fn permitted_assignments(&self) -> HashMap<LeaseId, HashSet<FixtureId>> {
        self.possible_assignments()
            .into_iter()
            .filter_map(|(lease_id, assigned)| {
                let existing: HashSet<FixtureId> = assigned
                    .into_iter()
                    .filter_map(|a| match a {
                        AssignedFixture::Existing(id) => Some(id),
                        AssignedFixture::New { .. } => None,
                    })
                    .collect();
                (!existing.is_empty()).then_some((lease_id, existing))
            })
            .collect()
    }

    /// Leases with no fixture assigned.
    pub fn unassigned_leases(&self) -> Vec<&LeaseState> {
        self.leases
            .iter()
            .filter(|(id, _)| !self.assignments.contains_key(id))
            .map(|(_, lease)| lease)
            .collect()
    }

    /// Fixtures with no lease assigned.
    pub fn unassigned_fixtures(&self) -> Vec<&FixtureState> {
        let assigned: HashSet<&FixtureId> = self.assignments.values().flatten().collect();
        self.fixtures
            .iter()
            .filter(|(id, _)| !assigned.contains(id))
            .map(|(_, f)| f)
            .collect_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::testing::{fake_fixture, fake_lease};

    #[test]
    fn empty_group_has_no_assignments() {
        let g = FixtureGroup::new(FixtureFingerprint::from_raw("fp".into()));
        assert!(g.possible_assignments().is_empty());
    }

    #[test]
    fn one_lease_gets_a_new_fixture() {
        let mut g = FixtureGroup::new(FixtureFingerprint::from_raw("fp".into()));
        let lease = fake_lease();
        let id = lease.id.clone();
        g.leases.insert(lease.id.clone(), lease);
        assert_eq!(
            g.possible_assignments(),
            HashMap::from([(id, vec![AssignedFixture::New { id: 1 }])])
        );
    }

    #[test]
    fn running_fixture_is_reused() {
        let mut g = FixtureGroup::new(FixtureFingerprint::from_raw("fp".into()));
        let mut fixture = fake_fixture();
        fixture.status = pb::InstanceStatus::InstanceRunning;
        let fid = fixture.id.clone();
        g.fixtures.insert(fixture.id.clone(), fixture);
        let lease = fake_lease();
        let lid = lease.id.clone();
        g.leases.insert(lease.id.clone(), lease);

        let permitted = g.permitted_assignments();
        assert_eq!(permitted[&lid], HashSet::from([fid]));
    }

    #[test]
    fn errored_fixture_is_not_reused() {
        let mut g = FixtureGroup::new(FixtureFingerprint::from_raw("fp".into()));
        let mut fixture = fake_fixture();
        fixture.status = pb::InstanceStatus::InstanceError;
        g.fixtures.insert(fixture.id.clone(), fixture);
        let lease = fake_lease();
        let id = lease.id.clone();
        g.leases.insert(lease.id.clone(), lease);
        assert_eq!(
            g.possible_assignments(),
            HashMap::from([(id, vec![AssignedFixture::New { id: 1 }])])
        );
    }

    #[test]
    fn two_single_use_leases_get_two_fixtures() {
        let mut g = FixtureGroup::new(FixtureFingerprint::from_raw("fp".into()));
        for _ in 0..2 {
            let mut lease = fake_lease();
            lease.use_scope = UseScope::SingleUse;
            g.leases.insert(lease.id.clone(), lease);
        }
        let possible = g.possible_assignments();
        assert_eq!(possible.len(), 2);
        let new_ids: HashSet<_> = possible
            .values()
            .flatten()
            .map(|a| match a {
                AssignedFixture::New { id } => *id,
                _ => panic!("expected new fixtures"),
            })
            .collect();
        assert_eq!(
            new_ids.len(),
            2,
            "each single-use lease needs its own fixture"
        );
    }
}
