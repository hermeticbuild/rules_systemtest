//! Shared test constructors for model types.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use systemtest_proto::systemtest::v1 as pb;

use crate::model::budget::Capacity;
use crate::model::fixture::{FixtureFingerprint, FixtureId, FixtureState};
use crate::model::lease::{LeaseFixtureStatus, LeaseId, LeaseState, UseScope};

pub(crate) fn fake_description() -> pb::FixtureDescription {
    pb::FixtureDescription {
        impl_id: "test".into(),
        name: "//:test".into(),
        ..Default::default()
    }
}

pub(crate) fn fake_fingerprint() -> FixtureFingerprint {
    FixtureFingerprint::from_raw("test-fp".into())
}

pub(crate) fn fake_lease() -> LeaseState {
    LeaseState {
        id: LeaseId::new_random(),
        owning_user_name: None,
        spec: fake_description(),
        fingerprint: fake_fingerprint(),
        use_scope: UseScope::Unrestricted,
        connections_needed: 1,
        created_at: SystemTime::UNIX_EPOCH,
        keepalive_timeout: None,
        last_keepalive_at: None,
        fixture_status: LeaseFixtureStatus::New,
    }
}

pub(crate) fn fake_fixture() -> FixtureState {
    FixtureState {
        id: FixtureId::new_random(),
        spec: fake_description(),
        fingerprint: fake_fingerprint(),
        status: pb::InstanceStatus::InstanceStarting,
        outputs: BTreeMap::new(),
        capacity_consumed: Capacity::zero(),
        messages: vec![],
        instance_data: vec![],
        created_at: SystemTime::UNIX_EPOCH,
        expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(36 * 60 * 60),
    }
}
