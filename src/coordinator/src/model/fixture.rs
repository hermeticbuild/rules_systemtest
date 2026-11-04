//! Fixtures: identity, ids, and the allocator's per-fixture record.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result};
use prost::Message;
use sha2::{Digest, Sha256};
use systemtest_proto::systemtest::v1 as pb;

use crate::model::budget::Capacity;

/// Identifies one provisioned fixture instance. Minted per create and handed
/// to the plugin to key idempotency on.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct FixtureId(String);

const FIXTURE_ID_PFX: &str = "fid-";

impl FixtureId {
    pub fn new_random() -> Self {
        FixtureId(format!(
            "{FIXTURE_ID_PFX}{}",
            uuid::Uuid::new_v4().as_hyphenated()
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// The unique suffix, for names in external systems (k8s/AWS/...).
    pub fn name_hint(&self) -> &str {
        self.0.strip_prefix(FIXTURE_ID_PFX).unwrap_or(&self.0)
    }
}

impl TryFrom<String> for FixtureId {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.starts_with(FIXTURE_ID_PFX) {
            Ok(FixtureId(value))
        } else {
            Err(anyhow::anyhow!(
                "({value}) does not look like a valid FixtureId"
            ))
        }
    }
}

/// A fixture's identity: two descriptions with the same fingerprint are
/// interchangeable and may share one instance. Distinct from [`FixtureId`],
/// which names a single instance. sha256 over a canonical proto encoding.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct FixtureFingerprint(String);

impl FixtureFingerprint {
    /// Content-addresses `desc`, folding in the plugin binary's contents so the
    /// identity is stable across the sandbox paths the binary is staged at.
    pub fn compute(desc: &pb::FixtureDescription, plugin_binary_path: &str) -> Result<Self> {
        let mut desc = desc.clone();
        // `name` is human-readable, not part of identity.
        desc.name = String::new();
        if let Some(pb::fixture_description::Plugin::BinaryPath(_)) = desc.plugin {
            desc.plugin = Some(pb::fixture_description::Plugin::BinaryPath(hash_file(
                plugin_binary_path,
            )?));
        }
        Ok(Self(hex(&Sha256::digest(desc.encode_to_vec()))))
    }

    /// For tests and for reconstructing a stored fingerprint.
    pub fn from_raw(s: String) -> Self {
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn hash_file(path: &str) -> Result<String> {
    let bytes =
        fs::read(Path::new(path)).with_context(|| format!("reading plugin binary {path}"))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The maximum number of leases that may share one fixture of `desc`.
/// At least 1.
pub fn max_concurrent_connections(desc: &pb::FixtureDescription) -> u32 {
    desc.sharing
        .as_ref()
        .map(|s| s.maximum_concurrent_connections)
        .filter(|&n| n > 0)
        .unwrap_or(1)
        .try_into()
        .unwrap_or(u32::MAX)
}

/// All allocator-tracked data for a single fixture instance.
#[derive(Clone, Debug)]
pub struct FixtureState {
    pub id: FixtureId,
    pub spec: pb::FixtureDescription,
    pub fingerprint: FixtureFingerprint,
    pub status: pb::InstanceStatus,
    /// The fixture's product, forwarded verbatim to the runner. Refreshed by
    /// the poller from `GetFixtureInfo`.
    pub outputs: BTreeMap<String, pb::Value>,
    pub capacity_consumed: Capacity,
    pub messages: Vec<String>,
    /// Opaque plugin handle, round-tripped to `GetFixtureInfo`/`Destroy`.
    pub instance_data: Vec<u8>,
    pub created_at: SystemTime,
    /// Absolute lifetime bound; GC deletes at this point regardless of reuse.
    pub expires_at: SystemTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_hint_strips_prefix() {
        let id = FixtureId::try_from("fid-abc-123".to_string()).unwrap();
        assert_eq!(id.name_hint(), "abc-123");
    }

    #[test]
    fn fingerprint_ignores_name() {
        let a = pb::FixtureDescription {
            name: "one".into(),
            impl_id: "x".into(),
            ..Default::default()
        };
        let b = pb::FixtureDescription {
            name: "two".into(),
            ..a.clone()
        };
        // No BinaryPath plugin, so no file read.
        assert_eq!(
            FixtureFingerprint::compute(&a, "").unwrap(),
            FixtureFingerprint::compute(&b, "").unwrap()
        );
    }
}
