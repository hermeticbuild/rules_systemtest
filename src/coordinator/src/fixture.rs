//! Fixtures: their identity, their ids, and the coordinator's record of one.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use prost::Message;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use systemtest_proto::systemtest::v1::{
    FixtureDescription, Sharing, Value, fixture_description::Plugin,
};

use crate::plugin_client::PluginProcess;

/// Identifies one provisioned fixture instance. Minted per `CreateFixture`
/// and handed to the plugin in `CreationMetadata` to key idempotency on.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FixtureId(String);

impl FixtureId {
    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

/// A fixture's identity: two descriptions with the same fingerprint are the
/// same fixture and may share one provisioned instance. Distinct from
/// [`FixtureId`], which names a single instance.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FixtureFingerprint(String);

impl FixtureFingerprint {
    /// sha256 over a canonical encoding of `desc`.
    pub fn compute(desc: &FixtureDescription, plugin_binary_path: &str) -> Result<Self> {
        let encoded = canonical_encoding(desc, plugin_binary_path)?;
        Ok(Self(hex_encode(&Sha256::digest(&encoded))))
    }
}

/// The bytes corresponding to a fixture's identity.
///
/// Protobuf encoding is deterministic enough to fingerprint with here because
/// prost writes fields in tag order and we use BTreeMap to keep maps stable.
fn canonical_encoding(desc: &FixtureDescription, plugin_binary_path: &str) -> Result<Vec<u8>> {
    let mut desc = desc.clone();
    // `name` is "human-readable; not part of identity"
    desc.name = String::new();
    // replace the path with the hash of the contents (makes the fingerprint deterministic
    // despite dynamic sandbox paths).
    if let Some(Plugin::BinaryPath(_)) = desc.plugin {
        desc.plugin = Some(Plugin::BinaryPath(hash_plugin_binary(plugin_binary_path)?));
    }
    Ok(desc.encode_to_vec())
}

/// Hex sha256 of the plugin binary's contents.
fn hash_plugin_binary(path: &str) -> Result<String> {
    let bytes =
        fs::read(Path::new(path)).with_context(|| format!("reading plugin binary at {path}"))?;
    Ok(hex_encode(&Sha256::digest(&bytes)))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A provisioned fixture, cached under its [`FixtureFingerprint`].
pub struct FixtureRecord {
    pub outputs: BTreeMap<String, Value>,
    pub sharing: Sharing,
    pub creating_client_id: String,
    pub used_slots: i64,
    /// Kept alive so the child plugin process (and its listening socket)
    /// stays up for the fixture's lifetime.
    pub _plugin: PluginProcess,
}
