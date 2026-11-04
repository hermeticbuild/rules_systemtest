//! The lease state machine: `New → Enqueued → Creating → Ready/Error`.
//!
//! A **pure** function over a snapshot: it performs no I/O. Capacity estimates are pre-fetched by the
//! caller, and the actual `factory.create` side effect is left to the creator
//! (so the `Creating` record can be persisted before provisioning). The
//! updater only decides and mutates the in-memory [`WorkingSet`].

use std::collections::{HashMap, VecDeque};

use itertools::Itertools;
use tracing::debug;

use crate::model::budget::{Capacity, Quota, QuotaError};
use crate::model::fixture::{FixtureFingerprint, FixtureId};
use crate::model::group::{AssignedFixture, FixtureGroup};
use crate::model::lease::{LeaseFixtureStatus, LeaseId, LeaseState};
use systemtest_proto::systemtest::v1 as pb;

/// The groups of one profile plus the shared budget, assembled by the creator
/// for a single reconcile pass.
pub struct WorkingSet {
    pub groups: HashMap<FixtureFingerprint, FixtureGroup>,
    pub quota: Quota,
    /// Per-group capacity estimate, pre-fetched from the factory.
    pub estimates: HashMap<FixtureFingerprint, Capacity>,
}

/// What the creator should do to a lease's fixtures after a decision.
#[derive(Clone, Debug, PartialEq)]
pub enum FixtureAction {
    /// Create a new fixture with this id and estimate (not yet provisioned).
    Create {
        fixture_id: FixtureId,
        estimate: Capacity,
    },
    /// Assign these existing fixtures.
    Assign { fixtures: Vec<FixtureId> },
    /// Drop all assignments (retry from scratch).
    UnassignAll,
}

/// A decision for one lease: its new status and any fixture action to persist.
#[derive(Clone, Debug, PartialEq)]
pub struct LeaseDecision {
    pub status: LeaseFixtureStatus,
    pub action: Option<FixtureAction>,
}

/// The result of one pass.
pub struct Outcome {
    pub decisions: HashMap<LeaseId, LeaseDecision>,
    pub capacity_pressure: Capacity,
}

enum Partial {
    None,
    Pressure(Capacity),
    Status(LeaseFixtureStatus),
    StatusAndAction(LeaseFixtureStatus, FixtureAction),
}

/// Advance every non-terminal lease in `ws` one or more steps, FIFO by
/// creation time. Pure: mutates only `ws` (assignments and the shared quota).
pub fn advance_leases(ws: &mut WorkingSet, failed_threshold: u32) -> Outcome {
    let queue: VecDeque<LeaseState> = ws
        .groups
        .values()
        .flat_map(|g| g.leases.values())
        .filter(|l| !l.fixture_status.is_terminal())
        .sorted_by_key(|l| l.created_at)
        .cloned()
        .collect();

    let mut enqueued_position = queue
        .iter()
        .filter(|l| matches!(l.fixture_status, LeaseFixtureStatus::Enqueued { .. }))
        .count();

    let mut outcome = Outcome {
        decisions: HashMap::new(),
        capacity_pressure: Capacity::zero(),
    };

    for lease in queue {
        advance_lease(
            ws,
            failed_threshold,
            &lease,
            &mut enqueued_position,
            &mut outcome,
        );
    }
    outcome
}

fn advance_lease(
    ws: &mut WorkingSet,
    failed_threshold: u32,
    lease: &LeaseState,
    enqueued_position: &mut usize,
    outcome: &mut Outcome,
) {
    let mut status = lease.fixture_status.clone();
    let mut prev_kind = status.kind();
    loop {
        let partial = advance_single_step(ws, failed_threshold, lease, enqueued_position, &status);
        status = apply_partial(outcome, lease, partial);

        let kind = status.kind();
        if kind == prev_kind {
            break;
        }
        // If a fixture action was produced, the creator must persist it before
        // we can reason about the next step.
        if outcome
            .decisions
            .get(&lease.id)
            .is_some_and(|d| d.action.is_some())
        {
            break;
        }
        prev_kind = kind;
    }
    if status != lease.fixture_status {
        debug!(
            lease = lease.id.as_str(),
            "status {:?} -> {:?}", lease.fixture_status, status
        );
    }
}

/// Fold a partial into the lease's decision; returns the resulting status.
fn apply_partial(
    outcome: &mut Outcome,
    lease: &LeaseState,
    partial: Partial,
) -> LeaseFixtureStatus {
    let entry = outcome
        .decisions
        .entry(lease.id.clone())
        .or_insert_with(|| LeaseDecision {
            status: lease.fixture_status.clone(),
            action: None,
        });
    match partial {
        Partial::None => {}
        Partial::Pressure(by) => outcome.capacity_pressure += &by,
        Partial::Status(s) => entry.status = s,
        Partial::StatusAndAction(s, action) => {
            entry.status = s;
            entry.action = Some(action);
        }
    }
    entry.status.clone()
}

fn advance_single_step(
    ws: &mut WorkingSet,
    failed_threshold: u32,
    lease: &LeaseState,
    enqueued_position: &mut usize,
    current: &LeaseFixtureStatus,
) -> Partial {
    match current {
        LeaseFixtureStatus::New => {
            let position = u32::try_from(*enqueued_position).unwrap_or(u32::MAX);
            *enqueued_position += 1;
            Partial::Status(LeaseFixtureStatus::Enqueued {
                attempt: 1,
                position,
            })
        }
        LeaseFixtureStatus::Enqueued { attempt, .. } => advance_enqueued(ws, lease, *attempt),
        LeaseFixtureStatus::Creating { attempt } => {
            advance_creating(ws, failed_threshold, lease, *attempt)
        }
        LeaseFixtureStatus::Ready => advance_ready(ws, lease),
        LeaseFixtureStatus::Error { .. } => Partial::None,
    }
}

fn error(message: impl Into<String>) -> Partial {
    Partial::Status(LeaseFixtureStatus::Error {
        message: Some(message.into()),
    })
}

fn advance_enqueued(ws: &mut WorkingSet, lease: &LeaseState, attempt: u32) -> Partial {
    let fp = lease.fingerprint.clone();
    let Some(group) = ws.groups.get_mut(&fp) else {
        return error(format!("no group for lease {}", lease.id.as_str()));
    };

    let possible = group.possible_assignments();
    let Some(assignments) = possible.get(&lease.id) else {
        return error(format!(
            "no possible assignment for enqueued lease {}",
            lease.id.as_str()
        ));
    };
    let (to_assign, to_create) = AssignedFixture::split(assignments);
    // Record the chosen existing fixtures so later leases this pass don't
    // double-book them.
    group.update_assignments(lease.id.clone(), to_assign.clone());

    if to_create == 0 {
        return Partial::StatusAndAction(
            LeaseFixtureStatus::Creating { attempt },
            FixtureAction::Assign {
                fixtures: to_assign.into_iter().collect(),
            },
        );
    }
    // One fixture per lease.
    debug_assert_eq!(to_create, 1);

    let Some(estimate) = ws.estimates.get(&fp).cloned() else {
        return error(format!(
            "no capacity estimate for lease {}",
            lease.id.as_str()
        ));
    };
    if estimate.any_greater(&ws.quota.total) {
        return error("estimated resource requirements exceed total available capacity");
    }
    match ws.quota.try_consume(&estimate) {
        Ok(()) => Partial::StatusAndAction(
            LeaseFixtureStatus::Creating { attempt },
            FixtureAction::Create {
                fixture_id: FixtureId::new_random(),
                estimate,
            },
        ),
        Err(QuotaError::Exceeded { by }) => Partial::Pressure(by),
    }
}

fn advance_creating(
    ws: &WorkingSet,
    failed_threshold: u32,
    lease: &LeaseState,
    attempt: u32,
) -> Partial {
    let statuses = assigned_statuses(ws, lease);
    if statuses.is_empty() {
        return error(format!(
            "lease {} is Creating with no fixtures",
            lease.id.as_str()
        ));
    }
    if statuses
        .iter()
        .any(|s| *s == pb::InstanceStatus::InstanceError)
    {
        return if attempt < failed_threshold {
            Partial::StatusAndAction(
                LeaseFixtureStatus::Enqueued {
                    attempt: attempt + 1,
                    position: 0,
                },
                FixtureAction::UnassignAll,
            )
        } else {
            error("fixture failed, retries exceeded")
        };
    }
    if statuses
        .iter()
        .all(|s| *s == pb::InstanceStatus::InstanceRunning)
    {
        Partial::Status(LeaseFixtureStatus::Ready)
    } else {
        Partial::None
    }
}

fn advance_ready(ws: &WorkingSet, lease: &LeaseState) -> Partial {
    if assigned_statuses(ws, lease)
        .iter()
        .any(|s| *s == pb::InstanceStatus::InstanceError)
    {
        error("a fixture of a ready lease failed")
    } else {
        Partial::None
    }
}

fn assigned_statuses(ws: &WorkingSet, lease: &LeaseState) -> Vec<pb::InstanceStatus> {
    let Some(group) = ws.groups.get(&lease.fingerprint) else {
        return vec![];
    };
    let Some(assigned) = group.assignments.get(&lease.id) else {
        return vec![];
    };
    assigned
        .iter()
        .filter_map(|id| group.fixtures.get(id).map(|f| f.status))
        .collect()
}
