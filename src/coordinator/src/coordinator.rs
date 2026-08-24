//! In-memory `CoordinatorService` implementation.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use tonic::{Request, Response, Status};
use uuid::Uuid;

use systemtest_proto::systemtest::v1::{
    CreateRequest, CreationMetadata, KeepaliveRequest, KeepaliveResponse, LeaseStatus,
    ReleaseRequest, ReleaseResponse, Sharing, StatusRequest, StatusResponse, TakeLeaseRequest,
    TakeLeaseResponse, coordinator_service_server::CoordinatorService,
    fixture_description::Plugin as FixturePlugin, sharing::UseScope,
};

use crate::fixture::{FixtureFingerprint, FixtureId, FixtureRecord};
use crate::lease::{LeaseId, LeaseRecord};
use crate::plugin_client::{self, PluginProcess};

#[derive(Default)]
struct State {
    // TODO: Only one fixture per fingerprint is currently supported.
    fixtures: HashMap<FixtureFingerprint, FixtureRecord>,
    leases: HashMap<LeaseId, LeaseRecord>,
}

pub struct Coordinator {
    /// Directory for this coordinator instance's plugin socket
    /// files. State itself is in-memory only.
    instance_dir: PathBuf,
    state: Mutex<State>,
}

impl Coordinator {
    pub fn new(instance_dir: PathBuf) -> Self {
        Self {
            instance_dir,
            state: Mutex::new(State::default()),
        }
    }

    /// A fresh socket path for a plugin instance.
    fn plugin_socket_path(&self) -> PathBuf {
        let id = Uuid::new_v4().simple().to_string();
        self.instance_dir
            .join("plugins")
            .join(format!("{}.sock", &id[..8]))
    }
}

/// Whether `sharing` admits reuse.
fn sharing_admits(sharing: &Sharing, creator_client_id: &str, requester_client_id: &str) -> bool {
    match sharing.use_scope.as_ref() {
        Some(UseScope::Unrestricted(_)) => true,
        Some(UseScope::ClientRestricted(_)) | None => creator_client_id == requester_client_id,
        Some(UseScope::SingleUse(_)) => false,
    }
}

#[tonic::async_trait]
impl CoordinatorService for Coordinator {
    async fn take_lease(
        &self,
        request: Request<TakeLeaseRequest>,
    ) -> Result<Response<TakeLeaseResponse>, Status> {
        let req = request.into_inner();
        let desc = req
            .fixture
            .ok_or_else(|| Status::invalid_argument("fixture description is required"))?;
        let binary_path = match &desc.plugin {
            Some(FixturePlugin::BinaryPath(p)) => p.clone(),
            Some(FixturePlugin::ContainerRef(_)) => {
                // TODO
                return Err(Status::unimplemented(
                    "remote-mode (container_ref) plugins are not supported by this scaffold coordinator",
                ));
            }
            None => return Err(Status::invalid_argument("fixture.plugin is required")),
        };

        let fp = FixtureFingerprint::compute(&desc, &binary_path)
            .map_err(|e| Status::internal(format!("computing fingerprint: {e:#}")))?;

        // 2. Reuse: attach to a live fixture with the same fingerprint if
        // sharing admits this client and a slot is free.
        {
            let mut state = self.state.lock().unwrap();
            if let Some(record) = state.fixtures.get_mut(&fp) {
                let max_slots = record.sharing.maximum_concurrent_connections.max(1);
                if sharing_admits(&record.sharing, &record.creating_client_id, &req.client_id)
                    && record.used_slots < max_slots
                {
                    record.used_slots += 1;
                    let outputs = record.outputs.clone();
                    let lease_id = LeaseId::random();
                    state
                        .leases
                        .insert(lease_id.clone(), LeaseRecord { fingerprint: fp });
                    return Ok(Response::new(TakeLeaseResponse {
                        status: LeaseStatus::Succeeded as i32,
                        lease_id: lease_id.into_string(),
                        outputs,
                        messages: vec!["connecting to existing fixture".to_string()],
                    }));
                }
            }
        } // lock dropped before the (possibly slow) provisioning below.

        // 3. Provision: spawn the plugin, DescribePlugin, then CreateFixture.
        let sock = self.plugin_socket_path();
        let startup_timeout = Duration::from_secs(if desc.startup_timeout_seconds > 0 {
            desc.startup_timeout_seconds as u64
        } else {
            300
        });
        let mut plugin = PluginProcess::spawn(&binary_path, &sock, startup_timeout)
            .await
            .map_err(|e| Status::internal(format!("spawning plugin: {e:#}")))?;

        plugin_client::describe(&mut plugin.client)
            .await
            .map_err(|e| Status::internal(format!("DescribePlugin: {e:#}")))?;

        let create_req = CreateRequest {
            impl_id: desc.impl_id.clone(),
            inputs: desc.inputs.clone(),
            metadata: Some(CreationMetadata {
                fixture_id: FixtureId::random().into_string(),
                client_id: req.client_id.clone(),
                owner: req.owner.clone(),
                labels: BTreeMap::new(),
            }),
        };
        let result = plugin_client::create_fixture(&mut plugin.client, create_req)
            .await
            .map_err(|e| Status::internal(format!("CreateFixture: {e:#}")))?;

        let sharing = desc.sharing.clone().unwrap_or(Sharing {
            use_scope: Some(UseScope::ClientRestricted(Default::default())),
            maximum_concurrent_connections: 1,
        });
        let outputs = result.outputs.clone();
        let lease_id = LeaseId::random();

        let mut state = self.state.lock().unwrap();
        state.fixtures.insert(
            fp.clone(),
            FixtureRecord {
                outputs: outputs.clone(),
                sharing,
                creating_client_id: req.client_id.clone(),
                used_slots: 1,
                _plugin: plugin,
            },
        );
        state
            .leases
            .insert(lease_id.clone(), LeaseRecord { fingerprint: fp });

        Ok(Response::new(TakeLeaseResponse {
            status: LeaseStatus::Succeeded as i32,
            lease_id: lease_id.into_string(),
            outputs,
            messages: vec![],
        }))
    }

    async fn keepalive(
        &self,
        request: Request<KeepaliveRequest>,
    ) -> Result<Response<KeepaliveResponse>, Status> {
        let req = request.into_inner();
        let state = self.state.lock().unwrap();
        if !state.leases.contains_key(&LeaseId::from(req.lease_id)) {
            return Err(Status::not_found("unknown lease_id"));
        }
        Ok(Response::new(KeepaliveResponse {}))
    }

    async fn release_lease(
        &self,
        request: Request<ReleaseRequest>,
    ) -> Result<Response<ReleaseResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.state.lock().unwrap();
        if let Some(lease) = state.leases.remove(&LeaseId::from(req.lease_id)) {
            if let Some(record) = state.fixtures.get_mut(&lease.fingerprint) {
                record.used_slots = (record.used_slots - 1).max(0);
            }
        }
        Ok(Response::new(ReleaseResponse {}))
    }

    async fn status(
        &self,
        request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let req = request.into_inner();
        let state = self.state.lock().unwrap();
        let lease = state
            .leases
            .get(&LeaseId::from(req.lease_id))
            .ok_or_else(|| Status::not_found("unknown lease_id"))?;
        let outputs = state
            .fixtures
            .get(&lease.fingerprint)
            .map(|r| r.outputs.clone())
            .unwrap_or_default();
        Ok(Response::new(StatusResponse {
            status: LeaseStatus::Succeeded as i32,
            outputs,
            messages: vec![],
        }))
    }
}
