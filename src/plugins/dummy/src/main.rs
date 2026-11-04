//! Stub `CoordinatorPlugin` server for the rules_systemtest scaffold.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::{Map, Value as JsonValue, json};
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status, transport::Server};

use systemtest_proto::systemtest::v1::{
    CreateRequest, CreateResponse, DescribeResponse, DescribeResponses, DestroyRequest,
    DestroyResponse, Empty, GetFixtureInfoRequest, GetFixtureInfoResponse, InstanceStatus,
    IssueAccessCredentialsRequest, IssueAccessCredentialsResponse, ReleaseAccessCredentialsRequest,
    ResourcePool, ResourceRequests, Value,
    coordinator_plugin_server::{CoordinatorPlugin, CoordinatorPluginServer},
    value::{SchemaBlob, Value as ValueKind},
};

const IMPL_ID: &str = "@rules_systemtest//fixtures:dummy";

#[derive(Parser, Debug)]
#[command(name = "plugin")]
struct Args {
    /// Unix domain socket to serve `CoordinatorPlugin` on.
    #[arg(long = "socket")]
    socket: PathBuf,
}

struct StubPlugin;

#[tonic::async_trait]
impl CoordinatorPlugin for StubPlugin {
    async fn describe_plugin(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<DescribeResponses>, Status> {
        let mut schemas = BTreeMap::new();
        schemas.insert("systemtest.ports".to_string(), "{}".to_string());
        Ok(Response::new(DescribeResponses {
            responses: vec![DescribeResponse {
                impl_id: IMPL_ID.to_string(),
                version: "0.0.1".to_string(),
                schemas,
            }],
        }))
    }

    async fn get_resource_pool(
        &self,
        _request: Request<CreateRequest>,
    ) -> Result<Response<ResourcePool>, Status> {
        Ok(Response::new(ResourcePool {
            pool_id: "local".to_string(),
            resources: vec![],
        }))
    }

    async fn get_resource_requests(
        &self,
        _request: Request<CreateRequest>,
    ) -> Result<Response<ResourceRequests>, Status> {
        Ok(Response::new(ResourceRequests { consumes: vec![] }))
    }

    async fn create_fixture(
        &self,
        request: Request<CreateRequest>,
    ) -> Result<Response<CreateResponse>, Status> {
        let req = request.into_inner();

        // The "ports" input arrives as a Value.str holding JSON like
        // {"main":"6379"} (CONTRACT.md "Stub plugin behavior").
        let ports_input: String = req
            .inputs
            .get("ports")
            .and_then(|v| match &v.value {
                Some(ValueKind::Str(s)) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "{}".to_string());

        // An optional `config` file input arrives inlined as Value.data (raw
        // bytes). It is JSON {name: port} merged over `ports` — this exercises
        // the file-input path end to end.
        let config = req.inputs.get("config").and_then(|v| match &v.value {
            Some(ValueKind::Data(bytes)) => Some(bytes.clone()),
            _ => None,
        });
        let ports = merged_ports(&ports_input, config.as_deref());

        // This plugin provisions nothing real, so its entire state is the
        // requested port mapping. Stash it in `instance_data` so GetFixtureInfo
        // can reproduce the same outputs statelessly.
        Ok(Response::new(CreateResponse {
            instance_data: ports.clone().into_bytes(),
            outputs: ports_outputs(&ports),
            consumed: vec![],
            idle_timeout_seconds: 60,
            max_lifetime_seconds: 0,
        }))
    }

    async fn get_fixture_info(
        &self,
        request: Request<GetFixtureInfoRequest>,
    ) -> Result<Response<GetFixtureInfoResponse>, Status> {
        let req = request.into_inner();
        // Recover the requested port mapping stashed in `instance_data` at create.
        let ports_input = String::from_utf8(req.instance_data).unwrap_or_else(|_| "{}".to_string());
        Ok(Response::new(GetFixtureInfoResponse {
            // Nothing real to start, so it is immediately running.
            status: InstanceStatus::InstanceRunning as i32,
            outputs: ports_outputs(&ports_input),
            messages: vec![],
            consumed: vec![],
        }))
    }

    async fn destroy_fixture(
        &self,
        _request: Request<DestroyRequest>,
    ) -> Result<Response<DestroyResponse>, Status> {
        // Provisions nothing real, so there's nothing to tear down.
        Ok(Response::new(DestroyResponse {}))
    }

    async fn issue_access_credentials(
        &self,
        _request: Request<IssueAccessCredentialsRequest>,
    ) -> Result<Response<IssueAccessCredentialsResponse>, Status> {
        // Self-expiring / no credentials: the coordinator tolerates UNIMPLEMENTED.
        Err(Status::unimplemented("dummy plugin issues no credentials"))
    }

    async fn release_access_credentials(
        &self,
        _request: Request<ReleaseAccessCredentialsRequest>,
    ) -> Result<Response<Empty>, Status> {
        Err(Status::unimplemented("dummy plugin issues no credentials"))
    }
}

/// Merges an optional `config` JSON object ({name: port}) over the `ports`
/// input, returning a canonical JSON object string.
fn merged_ports(ports_input: &str, config: Option<&[u8]>) -> String {
    let mut map: Map<String, JsonValue> = serde_json::from_str(ports_input).unwrap_or_default();
    if let Some(bytes) = config {
        if let Ok(extra) = serde_json::from_slice::<Map<String, JsonValue>>(bytes) {
            for (k, v) in extra {
                map.insert(k, v);
            }
        }
    }
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
}

/// Builds the `systemtest.ports` outputs map, one entry per key in
/// `ports_input` (JSON like `{"main":"6379"}`), per CONTRACT.md "Stub plugin
/// behavior". Every entry is `tunnel: false`, bound to loopback -- this plugin
/// provisions nothing real, it just echoes the requested port mapping back as
/// if it were reachable there.
fn ports_outputs(ports_input: &str) -> BTreeMap<String, Value> {
    let mut outputs = BTreeMap::new();
    outputs.insert(
        "endpoints".to_string(),
        Value {
            value: Some(ValueKind::SchemaBlob(SchemaBlob {
                schema_id: "systemtest.ports".to_string(),
                contents: ports_schema_blob(ports_input),
            })),
        },
    );
    outputs
}

/// Builds the `systemtest.ports` JSON blob. Port values may be JSON strings or
/// numbers.
fn ports_schema_blob(ports_input: &str) -> Vec<u8> {
    let parsed: Map<String, JsonValue> = serde_json::from_str(ports_input).unwrap_or_default();

    // Deterministic order so the output blob is stable across runs.
    let mut names: Vec<&String> = parsed.keys().collect();
    names.sort();

    let ports: Vec<JsonValue> = names
        .iter()
        .map(|name| {
            let port = match &parsed[*name] {
                JsonValue::String(s) => s.parse::<u64>().unwrap_or(0),
                JsonValue::Number(n) => n.as_u64().unwrap_or(0),
                _ => 0,
            };
            json!({
                "name": name,
                "protocol": "tcp",
                "tunnel": false,
                "host": "127.0.0.1",
                "port": port,
            })
        })
        .collect();

    serde_json::to_vec(&json!({ "ports": ports })).unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(parent) = args.socket.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating plugin socket dir {parent:?}"))?;
    }
    let _ = std::fs::remove_file(&args.socket);

    let listener = UnixListener::bind(&args.socket)
        .with_context(|| format!("binding plugin socket at {:?}", args.socket))?;
    let incoming = UnixListenerStream::new(listener);

    Server::builder()
        .add_service(CoordinatorPluginServer::new(StubPlugin))
        .serve_with_incoming(incoming)
        .await
        .context("plugin server exited")?;

    Ok(())
}
