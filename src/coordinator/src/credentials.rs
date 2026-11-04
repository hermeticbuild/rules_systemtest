//! In-memory store of per-consumer access credentials.
//!
//! Credentials are issued per lease when its fixtures become ready and held
//! here for the lease's lifetime. Invariant 4: never persisted — this lives in
//! memory only, in either mode. The issuance validity is the leak backstop, so
//! losing this cache (e.g. a crash) is safe.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use systemtest_proto::systemtest::v1 as pb;

use crate::model::lease::LeaseId;

/// A per-fixture issued handle, retained so the slice can be returned early on
/// release.
#[derive(Clone)]
pub struct IssuedHandle {
    pub impl_id: String,
    pub instance_data: Vec<u8>,
    pub handle: Vec<u8>,
}

/// Everything issued for one lease.
#[derive(Default)]
pub struct IssuedForLease {
    pub credentials: BTreeMap<String, pb::Value>,
    pub handles: Vec<IssuedHandle>,
}

/// Lease id -> issued material. Shared between the allocator (writes on
/// readiness) and the service (reads for `Status`, drains on release).
#[derive(Default)]
pub struct CredentialStore {
    inner: Mutex<HashMap<LeaseId, IssuedForLease>>,
}

impl CredentialStore {
    pub fn new() -> Self {
        CredentialStore::default()
    }

    /// Whether credentials have already been issued for `lease` (so the
    /// allocator issues exactly once).
    pub fn contains(&self, lease: &LeaseId) -> bool {
        self.inner.lock().unwrap().contains_key(lease)
    }

    pub fn put(&self, lease: LeaseId, issued: IssuedForLease) {
        self.inner.lock().unwrap().insert(lease, issued);
    }

    /// A copy of the credentials for `lease` (empty if none).
    pub fn credentials(&self, lease: &LeaseId) -> BTreeMap<String, pb::Value> {
        self.inner
            .lock()
            .unwrap()
            .get(lease)
            .map(|i| i.credentials.clone())
            .unwrap_or_default()
    }

    /// Remove and return a lease's issued material (for release on lease end).
    pub fn take(&self, lease: &LeaseId) -> Option<IssuedForLease> {
        self.inner.lock().unwrap().remove(lease)
    }
}
