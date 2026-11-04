//! The coordinator's authoritative state.
//!
//! Scoped reads + atomic mutation batches — never a whole-world load. Each read
//! is named so a backend can index it later without the allocator changing.
//! Only [`memory::InMemoryStore`] ships today; a Postgres backend comes with
//! remote mode.

use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;

use crate::model::budget::Capacity;
use crate::model::fixture::{FixtureFingerprint, FixtureId, FixtureState};
use crate::model::group::FixtureGroup;
use crate::model::lease::{LeaseFixtureStatus, LeaseId, LeaseState};
use crate::model::settings::ProfileId;

pub mod memory;

/// A single state change. A slice of these is applied atomically by
/// [`Store::apply`].
#[derive(Clone, Debug)]
pub enum Mutation {
    CreateLease(Box<LeaseState>),
    SetLeaseFixtureStatus {
        lease: LeaseId,
        status: LeaseFixtureStatus,
    },
    SetKeepalive {
        lease: LeaseId,
        at: SystemTime,
    },
    DeleteLease(LeaseId),
    UpsertFixture(Box<FixtureState>),
    DeleteFixture(FixtureId),
    Assign {
        lease: LeaseId,
        fixture: FixtureId,
    },
    ClearAssignments(LeaseId),
}

/// Scoped reads + one atomic write. See the module docs. Reads are each
/// internally consistent; a caller may compose several per tick and tolerate
/// skew across them (the allocator is the single writer and the RPC frontend
/// only appends leases / touches timers, and the reconcile loop repairs stale
/// decisions).
#[async_trait]
pub trait Store: Send + Sync {
    /// Fingerprints of every non-empty group (has at least one lease or
    /// fixture). Drives the creator — assignment, reclaim, and old-fixture GC.
    async fn groups_needing_work(&self) -> Result<Vec<FixtureFingerprint>>;

    /// One group's working set — its fixtures, leases, and assignments — read
    /// atomically so matching sees a consistent picture.
    async fn group(&self, fingerprint: &FixtureFingerprint) -> Result<FixtureGroup>;

    /// Live fixtures the poller should refresh (non-terminal status).
    async fn pollable_fixtures(&self) -> Result<Vec<FixtureState>>;

    /// Leases whose keepalive has lapsed or whose age exceeds `max_lifetime`.
    async fn leases_due(&self, now: SystemTime, max_lifetime: Duration) -> Result<Vec<LeaseId>>;

    /// Fixtures past their absolute `expires_at` that are **not** currently
    /// assigned (a live lease is never destroyed — such a fixture is retired,
    /// not deleted, until its lease releases).
    async fn fixtures_due(&self, now: SystemTime) -> Result<Vec<FixtureState>>;

    /// Resources charged against a profile's budget (sum of live fixtures).
    async fn budget_usage(&self, profile: &ProfileId) -> Result<Capacity>;

    /// Read one lease.
    async fn lease(&self, id: &LeaseId) -> Result<Option<LeaseState>>;

    /// Read the fixtures currently assigned to a lease.
    async fn fixtures_for_lease(&self, id: &LeaseId) -> Result<Vec<FixtureState>>;

    /// Apply `muts` in order, atomically.
    async fn apply(&self, muts: &[Mutation]) -> Result<()>;
}
