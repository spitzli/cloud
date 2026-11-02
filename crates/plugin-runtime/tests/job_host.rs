use serde_json::{Value, json};
use tonic::{Request, Response, Status};
use webdock_plugin_protocol::ErrorCode;
use webdock_plugin_runtime::{HostClient, UnixSocket, wire};

#[derive(Clone)]
struct Host;
#[tonic::async_trait]
impl wire::plugin_host_server::PluginHost for Host {
    async fn authorize_peer(
        &self,
        _: Request<wire::HostPeerRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
    async fn authorize_invocation(
        &self,
        _: Request<wire::HostInvocationRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
    async fn authorize_request(
        &self,
        _: Request<wire::HostPayloadRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
    async fn workloads(
        &self,
        _: Request<wire::HostWorkloadRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
    async fn call(
        &self,
        request: Request<wire::HostCallRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        let request = request.into_inner();
        if request.invocation_token != "attempt-token" || request.service != "jobs" {
            return Err(Status::permission_denied("wrong authority"));
        }
        let input: Value = serde_json::from_slice(&request.input_json).unwrap();
        let output = json!({"method":request.method,"input":input});
        Ok(Response::new(wire::HostCallResponse {
            output_json: serde_json::to_vec(&output).unwrap(),
        }))
    }
}
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn native_host_client_carries_only_the_three_bounded_job_methods() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("host.sock");
    let mut socket = UnixSocket::bind(&path).unwrap();
    let incoming = socket.incoming().unwrap();
    let _task = Task(tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(wire::plugin_host_server::PluginHostServer::new(Host))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    }));
    let client = HostClient::connect_unix(&path).await.unwrap();
    for method in ["enqueue", "get", "cancel"] {
        let input = if method == "enqueue" {
            json!({"name":"notes.compact","payload":"x".repeat(32760),"idempotencyKey":"fixture-job-1"})
        } else {
            json!({"id":format!("job-{}","a".repeat(64))})
        };
        let result = client
            .call("attempt-token", "jobs", method, input.clone())
            .await
            .unwrap();
        assert_eq!(result, json!({"method":method,"input":input}));
        assert_eq!(
            client
                .call("attempt-token", "jobs", method, json!("x".repeat(36864)))
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidInput
        );
    }
    assert_eq!(
        client
            .call("attempt-token", "jobs", "schedule", json!({}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::CapabilityUnavailable
    );
}
