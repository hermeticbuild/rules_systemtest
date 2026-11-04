//! Leases: a client's claim on provisioned fixtures, and the allocator's
//! per-lease progress bookkeeping.

use std::time::{Duration, SystemTime};

use systemtest_proto::systemtest::v1 as pb;

use crate::model::fixture::FixtureFingerprint;

/// Identifies one lease. Prefixed to avoid mixing with other id kinds.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct LeaseId(String);

const LEASE_ID_PFX: &str = "lid-";

impl LeaseId {
    pub fn new_random() -> Self {
        LeaseId(format!(
            "{LEASE_ID_PFX}{}",
            uuid::Uuid::new_v4().as_hyphenated()
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for LeaseId {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.starts_with(LEASE_ID_PFX) {
            Ok(LeaseId(value))
        } else {
            Err(anyhow::anyhow!(
                "({value}) does not look like a valid LeaseId"
            ))
        }
    }
}

/// Policy for sharing a fixture across leases.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UseScope {
    /// Shares with any other `Unrestricted` lease, across clients.
    Unrestricted,
    /// Shares only with `ExclusiveToClient` leases carrying the same client id.
    ExclusiveToClient(String),
    /// Never shares, not even with another `SingleUse` lease.
    SingleUse,
}

impl UseScope {
    /// Builds the scope from a proto `Sharing` policy and the resolved client id.
    /// Absent sharing defaults to client-restricted, per DESIGN.md.
    pub fn from_sharing(sharing: Option<&pb::Sharing>, client_id: &str) -> Self {
        match sharing.and_then(|s| s.use_scope.as_ref()) {
            Some(pb::sharing::UseScope::Unrestricted(_)) => UseScope::Unrestricted,
            Some(pb::sharing::UseScope::SingleUse(_)) => UseScope::SingleUse,
            Some(pb::sharing::UseScope::ClientRestricted(_)) | None => {
                UseScope::ExclusiveToClient(client_id.to_string())
            }
        }
    }

    /// Whether two scopes can share one fixture. `SingleUse` is compatible with
    /// nothing concrete (only an as-yet-unconstrained `Unrestricted` slot can
    /// reduce *into* it, via [`reduce`]).
    pub fn is_compatible(&self, other: &UseScope) -> bool {
        if *self == UseScope::Unrestricted || *other == UseScope::Unrestricted {
            return true;
        }
        // Neither is Unrestricted: two concrete scopes.
        if matches!(self, UseScope::SingleUse) || matches!(other, UseScope::SingleUse) {
            return false;
        }
        self == other
    }

    /// The most restrictive of two compatible scopes, or `None` if incompatible.
    pub fn reduce<'a>(&'a self, other: &'a UseScope) -> Option<&'a UseScope> {
        if !self.is_compatible(other) {
            return None;
        }
        if *self == UseScope::Unrestricted {
            Some(other)
        } else {
            Some(self)
        }
    }
}

/// How far a lease has progressed toward having its fixtures allocated.
/// Set and maintained by the allocator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseFixtureStatus {
    /// Just created.
    New,
    /// No fixtures assigned yet; waiting (likely out of quota).
    Enqueued { attempt: u32, position: u32 },
    /// Fixtures assigned but not all running.
    Creating { attempt: u32 },
    /// All assigned fixtures running and healthy.
    Ready,
    /// Terminal failure.
    Error { message: Option<String> },
}

/// Variant-only view of [`LeaseFixtureStatus`], for comparing transitions while
/// ignoring payloads (e.g. a changing queue `position`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseFixtureStatusKind {
    New,
    Enqueued,
    Creating,
    Ready,
    Error,
}

impl LeaseFixtureStatus {
    pub fn kind(&self) -> LeaseFixtureStatusKind {
        match self {
            LeaseFixtureStatus::New => LeaseFixtureStatusKind::New,
            LeaseFixtureStatus::Enqueued { .. } => LeaseFixtureStatusKind::Enqueued,
            LeaseFixtureStatus::Creating { .. } => LeaseFixtureStatusKind::Creating,
            LeaseFixtureStatus::Ready => LeaseFixtureStatusKind::Ready,
            LeaseFixtureStatus::Error { .. } => LeaseFixtureStatusKind::Error,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, LeaseFixtureStatus::Error { .. })
    }
}

/// All bookkeeping for a single lease. Relationships to fixtures are tracked
/// separately (in the store's assignment map).
#[derive(Clone, Debug)]
pub struct LeaseState {
    pub id: LeaseId,
    pub owning_user_name: Option<String>,
    /// The fixture recipe this lease wants. (This repo passes a
    /// `FixtureDescription` directly, not a wrapping `LeaseDescription`.)
    pub spec: pb::FixtureDescription,
    /// Identity of `spec` (computed once at the edge and persisted), used to
    /// group leases and fixtures without recomputing.
    pub fingerprint: FixtureFingerprint,
    pub use_scope: UseScope,
    pub connections_needed: u32,
    pub created_at: SystemTime,
    pub keepalive_timeout: Option<Duration>,
    pub last_keepalive_at: Option<SystemTime>,
    pub fixture_status: LeaseFixtureStatus,
}

/// Derives a runner-facing [`pb::LeaseStatus`] from the statuses of the
/// fixtures assigned to a lease.
pub fn lease_status_from_instance_statuses<'a>(
    statuses: impl Iterator<Item = &'a pb::InstanceStatus>,
) -> pb::LeaseStatus {
    let mut has_error = false;
    let mut has_starting = false;
    let mut empty = true;

    for status in statuses {
        empty = false;
        match status {
            pb::InstanceStatus::Unknown
            | pb::InstanceStatus::InstanceDefunct
            | pb::InstanceStatus::InstanceError
            | pb::InstanceStatus::InstanceShuttingDown => has_error = true,
            pb::InstanceStatus::InstanceStarting => has_starting = true,
            pb::InstanceStatus::InstanceRunning => {}
        }
    }

    if has_error {
        pb::LeaseStatus::Failed
    } else if has_starting || empty {
        pb::LeaseStatus::Pending
    } else {
        pb::LeaseStatus::Succeeded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_use_never_shares_but_unrestricted_reduces_into_it() {
        assert!(!UseScope::SingleUse.is_compatible(&UseScope::SingleUse));
        assert!(!UseScope::SingleUse.is_compatible(&UseScope::ExclusiveToClient("a".into())));
        // An unconstrained slot can still take on SingleUse (one-to-one assignment).
        assert!(UseScope::Unrestricted.is_compatible(&UseScope::SingleUse));
        assert_eq!(
            UseScope::Unrestricted.reduce(&UseScope::SingleUse),
            Some(&UseScope::SingleUse)
        );
        assert_eq!(UseScope::SingleUse.reduce(&UseScope::SingleUse), None);
    }

    #[test]
    fn exclusive_shares_only_same_client() {
        let a = UseScope::ExclusiveToClient("a".into());
        let b = UseScope::ExclusiveToClient("b".into());
        assert!(a.is_compatible(&a));
        assert!(!a.is_compatible(&b));
        assert!(UseScope::Unrestricted.is_compatible(&a));
    }

    #[test]
    fn lease_status_derivation() {
        use pb::InstanceStatus::*;
        use pb::LeaseStatus;
        assert_eq!(
            lease_status_from_instance_statuses([].iter()),
            LeaseStatus::Pending
        );
        assert_eq!(
            lease_status_from_instance_statuses([InstanceError].iter()),
            LeaseStatus::Failed
        );
        assert_eq!(
            lease_status_from_instance_statuses([InstanceRunning, InstanceStarting].iter()),
            LeaseStatus::Pending
        );
        assert_eq!(
            lease_status_from_instance_statuses([InstanceRunning, InstanceRunning].iter()),
            LeaseStatus::Succeeded
        );
    }
}
