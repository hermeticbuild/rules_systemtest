//! Leases: a client's claim on a provisioned fixture.

use uuid::Uuid;

use crate::fixture::FixtureFingerprint;

/// Identifies one lease.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LeaseId(String);

impl LeaseId {
    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for LeaseId {
    /// Lease ids arrive from the runner as bare strings on the wire.
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// A runner's claim on a fixture. Only enough to route Keepalive/Release/
/// Status back to the right fixture.
pub struct LeaseRecord {
    pub fingerprint: FixtureFingerprint,
}
