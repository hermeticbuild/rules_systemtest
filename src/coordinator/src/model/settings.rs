//! Coordinator settings: allocator cadences and per-profile config.
//!
//! A *profile* is a named config bundle (budget + lifetime policy + opaque
//! plugin blob) resolved by `profile_id`. It is deliberately **not** a
//! "namespace" (a team/infra container); the
//! allocator groups fixtures by fingerprint, not by profile.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::budget::Capacity;

/// Identifies a profile. Supplied by clients as `FixtureDescription.profile_id`
/// ("" resolves to [`ProfileId::default`]).
#[derive(Clone, Debug, Hash, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ProfileId(pub String);

impl ProfileId {
    pub const DEFAULT: &'static str = "default";

    pub fn default_id() -> Self {
        ProfileId(Self::DEFAULT.to_string())
    }

    /// Resolves a client-supplied id, mapping "" to the default.
    pub fn resolve(raw: &str) -> Self {
        if raw.is_empty() {
            Self::default_id()
        } else {
            ProfileId(raw.to_string())
        }
    }
}

impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn default_max_lifetime_secs() -> u64 {
    36 * 60 * 60
}
fn default_failed_threshold() -> u32 {
    2
}

/// Per-profile configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProfileSettings {
    /// Resource ceiling for everything charged to this profile.
    pub budget: Capacity,
    #[serde(default = "default_max_lifetime_secs")]
    pub fixtures_max_lifetime_secs: u64,
    #[serde(default = "default_failed_threshold")]
    pub failed_fixtures_to_fail_lease_threshold: u32,
    /// Opaque per-plugin blob, forwarded to the plugin verbatim as
    /// `CreateRequest.profile_config`. Empty for the dummy.
    #[serde(default)]
    pub config: String,
}

impl ProfileSettings {
    pub fn fixtures_max_lifetime(&self) -> Duration {
        Duration::from_secs(self.fixtures_max_lifetime_secs)
    }
}

fn default_creator_secs() -> u64 {
    1
}
fn default_poller_secs() -> u64 {
    5
}
fn default_cleaner_secs() -> u64 {
    10
}
fn default_lease_timeout_secs() -> u64 {
    60 * 60
}

/// Allocator task cadences and the hard lease lifetime bound.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AllocatorSettings {
    #[serde(default = "default_creator_secs")]
    pub creator_interval_secs: u64,
    #[serde(default = "default_poller_secs")]
    pub poller_interval_secs: u64,
    #[serde(default = "default_cleaner_secs")]
    pub cleaner_interval_secs: u64,
    #[serde(default = "default_lease_timeout_secs")]
    pub lease_timeout_secs: u64,
}

impl Default for AllocatorSettings {
    fn default() -> Self {
        AllocatorSettings {
            creator_interval_secs: default_creator_secs(),
            poller_interval_secs: default_poller_secs(),
            cleaner_interval_secs: default_cleaner_secs(),
            lease_timeout_secs: default_lease_timeout_secs(),
        }
    }
}

impl AllocatorSettings {
    pub fn creator_interval(&self) -> Duration {
        Duration::from_secs(self.creator_interval_secs)
    }
    pub fn poller_interval(&self) -> Duration {
        Duration::from_secs(self.poller_interval_secs)
    }
    pub fn cleaner_interval(&self) -> Duration {
        Duration::from_secs(self.cleaner_interval_secs)
    }
    pub fn lease_timeout(&self) -> Duration {
        Duration::from_secs(self.lease_timeout_secs)
    }
}

/// Top-level coordinator settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub allocator: AllocatorSettings,
    pub profiles: HashMap<ProfileId, ProfileSettings>,
}

impl Settings {
    /// Parse settings from JSON.
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        serde_json::from_str(s).map_err(|e| anyhow::anyhow!("decoding settings JSON: {e}"))
    }

    /// Resolve a profile's settings, or `None` if unconfigured.
    pub fn profile(&self, id: &ProfileId) -> Option<&ProfileSettings> {
        self.profiles.get(id)
    }

    /// A single-profile default for local mode when no settings file is given:
    /// a generous budget so local Docker-style fixtures are never quota-blocked
    /// by the coordinator's estimate (the infrastructure stays authoritative).
    pub fn default_local() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(
            ProfileId::default_id(),
            ProfileSettings {
                budget: Capacity {
                    cpu_mili: 1_000_000,
                    memory_bytes: 1 << 50, // 1 PiB, i.e. effectively unbounded locally
                },
                fixtures_max_lifetime_secs: default_max_lifetime_secs(),
                failed_fixtures_to_fail_lease_threshold: default_failed_threshold(),
                config: String::new(),
            },
        );
        Settings {
            allocator: AllocatorSettings::default(),
            profiles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_local_has_default_profile() {
        let s = Settings::default_local();
        assert!(s.profile(&ProfileId::default_id()).is_some());
    }

    #[test]
    fn parses_minimal_json_with_defaults() {
        let json = r#"{
            "profiles": {
                "default": { "budget": { "cpu_mili": 1000, "memory_bytes": 1024 } }
            }
        }"#;
        let s = Settings::from_json(json).unwrap();
        let p = s.profile(&ProfileId::default_id()).unwrap();
        assert_eq!(p.budget.cpu_mili, 1000);
        assert_eq!(p.failed_fixtures_to_fail_lease_threshold, 2);
        assert_eq!(s.allocator.creator_interval_secs, 1);
    }
}
