//! The runner-facing `CoordinatorService`, backed by the [`Store`].
//!
//! Handlers are thin: `TakeLease` persists a `New` lease and returns `PENDING`;
//! the background allocator drives it to `SUCCEEDED`; `Status` reads reconciled
//! state. No provisioning happens in an RPC handler.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tonic::{Request, Response, Status};

use crate::credentials::CredentialStore;
use crate::model::fixture::FixtureFingerprint;
use crate::model::lease::{
    LeaseFixtureStatus, LeaseId, LeaseState, UseScope, lease_status_from_instance_statuses,
};
use crate::plugin::FixtureFactory;
use crate::store::{Mutation, Store};
use systemtest_proto::systemtest::v1 as pb;
use systemtest_proto::systemtest::v1::coordinator_service_server::CoordinatorService;

/// How long a lease may go without a keepalive before the cleaner GCs it. The
/// runner heartbeats every 15s.
const KEEPALIVE_GRACE: Duration = Duration::from_secs(60);

pub struct Service {
    store: Arc<dyn Store>,
    factory: Arc<dyn FixtureFactory>,
    credentials: Arc<CredentialStore>,
}

impl Service {
    pub fn new(
        store: Arc<dyn Store>,
        factory: Arc<dyn FixtureFactory>,
        credentials: Arc<CredentialStore>,
    ) -> Self {
        Service {
            store,
            factory,
            credentials,
        }
    }
}

fn parse_lease_id(raw: String) -> Result<LeaseId, Status> {
    LeaseId::try_from(raw).map_err(|e| Status::invalid_argument(e.to_string()))
}

#[tonic::async_trait]
impl CoordinatorService for Service {
    async fn take_lease(
        &self,
        request: Request<pb::TakeLeaseRequest>,
    ) -> Result<Response<pb::TakeLeaseResponse>, Status> {
        let req = request.into_inner();
        let desc = req
            .fixture
            .ok_or_else(|| Status::invalid_argument("fixture description is required"))?;

        let binary_path = match &desc.plugin {
            Some(pb::fixture_description::Plugin::BinaryPath(p)) => p.clone(),
            Some(pb::fixture_description::Plugin::ContainerRef(_)) => {
                return Err(Status::unimplemented(
                    "remote-mode (container_ref) plugins are not supported in local mode",
                ));
            }
            None => return Err(Status::invalid_argument("fixture.plugin is required")),
        };

        let fingerprint = FixtureFingerprint::compute(&desc, &binary_path)
            .map_err(|e| Status::internal(format!("computing fingerprint: {e:#}")))?;
        let use_scope = UseScope::from_sharing(desc.sharing.as_ref(), &req.client_id);
        let now = SystemTime::now();
        let lease = LeaseState {
            id: LeaseId::new_random(),
            owning_user_name: (!req.owner.is_empty()).then(|| req.owner.clone()),
            spec: desc,
            fingerprint,
            use_scope,
            connections_needed: req.associated_tests_count.max(1),
            created_at: now,
            keepalive_timeout: Some(KEEPALIVE_GRACE),
            last_keepalive_at: Some(now),
            fixture_status: LeaseFixtureStatus::New,
        };
        let lease_id = lease.id.clone();

        self.store
            .apply(&[Mutation::CreateLease(Box::new(lease))])
            .await
            .map_err(|e| Status::internal(format!("persisting lease: {e:#}")))?;

        Ok(Response::new(pb::TakeLeaseResponse {
            status: pb::LeaseStatus::Pending as i32,
            lease_id: lease_id.into_string(),
            outputs: BTreeMap::new(),
            messages: vec!["lease accepted".to_string()],
            credentials: BTreeMap::new(),
        }))
    }

    async fn status(
        &self,
        request: Request<pb::StatusRequest>,
    ) -> Result<Response<pb::StatusResponse>, Status> {
        let id = parse_lease_id(request.into_inner().lease_id)?;
        let lease = self
            .store
            .lease(&id)
            .await
            .map_err(|e| Status::internal(format!("reading lease: {e:#}")))?
            .ok_or_else(|| Status::not_found("unknown lease_id"))?;
        let fixtures = self
            .store
            .fixtures_for_lease(&id)
            .await
            .map_err(|e| Status::internal(format!("reading fixtures: {e:#}")))?;

        let (status, outputs, messages) = match &lease.fixture_status {
            LeaseFixtureStatus::New
            | LeaseFixtureStatus::Enqueued { .. }
            | LeaseFixtureStatus::Creating { .. } => {
                (pb::LeaseStatus::Pending, BTreeMap::new(), vec![])
            }
            LeaseFixtureStatus::Ready => {
                let st = lease_status_from_instance_statuses(fixtures.iter().map(|f| &f.status));
                let outputs = if st == pb::LeaseStatus::Succeeded {
                    let mut merged = BTreeMap::new();
                    for f in &fixtures {
                        merged.extend(f.outputs.clone());
                    }
                    merged
                } else {
                    BTreeMap::new()
                };
                (st, outputs, vec![])
            }
            LeaseFixtureStatus::Error { message } => (
                pb::LeaseStatus::Failed,
                BTreeMap::new(),
                message.clone().into_iter().collect(),
            ),
        };

        // Hand credentials to the client on success (to use the fixture) and on
        // failure (to debug the broken fixture's namespace while its lease lives).
        let credentials = match status {
            pb::LeaseStatus::Succeeded | pb::LeaseStatus::Failed => {
                self.credentials.credentials(&id)
            }
            _ => BTreeMap::new(),
        };

        Ok(Response::new(pb::StatusResponse {
            status: status as i32,
            outputs,
            messages,
            credentials,
        }))
    }

    async fn keepalive(
        &self,
        request: Request<pb::KeepaliveRequest>,
    ) -> Result<Response<pb::KeepaliveResponse>, Status> {
        let id = parse_lease_id(request.into_inner().lease_id)?;
        if self
            .store
            .lease(&id)
            .await
            .map_err(|e| Status::internal(format!("reading lease: {e:#}")))?
            .is_none()
        {
            return Err(Status::not_found("unknown lease_id"));
        }
        self.store
            .apply(&[Mutation::SetKeepalive {
                lease: id,
                at: SystemTime::now(),
            }])
            .await
            .map_err(|e| Status::internal(format!("recording keepalive: {e:#}")))?;
        Ok(Response::new(pb::KeepaliveResponse {}))
    }

    async fn release_lease(
        &self,
        request: Request<pb::ReleaseRequest>,
    ) -> Result<Response<pb::ReleaseResponse>, Status> {
        let id = parse_lease_id(request.into_inner().lease_id)?;
        // Early-return any per-consumer slices (best-effort; issuance validity
        // is the backstop).
        if let Some(issued) = self.credentials.take(&id) {
            for h in issued.handles {
                let _ = self
                    .factory
                    .release_access_credentials(&h.impl_id, &h.instance_data, &h.handle)
                    .await;
            }
        }
        self.store
            .apply(&[Mutation::DeleteLease(id)])
            .await
            .map_err(|e| Status::internal(format!("releasing lease: {e:#}")))?;
        Ok(Response::new(pb::ReleaseResponse {}))
    }
}
