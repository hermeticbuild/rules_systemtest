//! Stub `CoordinatorPlugin` server for the rules_systemtest scaffold.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::{Map, Value as JsonValue, json};
use tokio::net::UnixListener;
use tokio_stream::{Stream, wrappers::UnixListenerStream};
use tonic::{Request, Response, Status, transport::Server};

use systemtest_proto::systemtest::v1::{
    CreateProgress, CreateProgressStatus, CreateRequest, DescribeResponse, DescribeResponses,
    DestroyRequest, DestroyResponse, Empty, FixtureResult, HealthCheckRequest, HealthCheckResponse,
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

    type CreateFixtureStream =
        Pin<Box<dyn Stream<Item = Result<CreateProgress, Status>> + Send + 'static>>;

    async fn create_fixture(
        &self,
        request: Request<CreateRequest>,
    ) -> Result<Response<Self::CreateFixtureStream>, Status> {
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
        let contents = ports_schema_blob(&ports_input);

        let mut outputs = BTreeMap::new();
        outputs.insert(
            "endpoints".to_string(),
            Value {
                value: Some(ValueKind::SchemaBlob(SchemaBlob {
                    schema_id: "systemtest.ports".to_string(),
                    contents,
                })),
            },
        );

        let progress = CreateProgress {
            status: CreateProgressStatus::CreateProgressSucceeded as i32,
            element_infos: vec![],
            result: Some(FixtureResult {
                outputs,
                idle_timeout_seconds: 60,
                instance_data: b"stub".to_vec(),
                consumed: vec![],
            }),
        };
        let stream: Self::CreateFixtureStream = Box::pin(tokio_stream::once(Ok(progress)));
        Ok(Response::new(stream))
    }

    async fn destroy_fixture(
        &self,
        _request: Request<DestroyRequest>,
    ) -> Result<Response<DestroyResponse>, Status> {
        // Provisions nothing real, so there's nothing to tear down.
        Ok(Response::new(DestroyResponse {}))
    }

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            healthy: true,
            message: String::new(),
        }))
    }
}

/// Builds the `systemtest.ports` JSON, one entry per key in `ports_input`
/// (JSON like `{"main":"6379"}`), per CONTRACT.md "Stub plugin behavior".
/// Every entry is `tunnel: false`, bound to loopback -- this plugin
/// provisions nothing real, it just echoes the requested port mapping back
/// as if it were reachable there. Port values may be JSON strings or numbers.
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
