use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use webdock_control_plane::{plugin_config::RuntimeConnections, plugins::PluginEndpoints};
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::{
    ActionHandler, CorePeer, HostClient, JobHandler, RemoteAuthority, Runtime, UnixSocket, wire,
};

struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct Handler {
    host: HostClient,
    started: Arc<tokio::sync::Notify>,
    dropped: Arc<AtomicBool>,
}
struct PendingDrop(Arc<AtomicBool>);
impl Drop for PendingDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct CapacityHandler {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Notify,
}
#[tonic::async_trait]
impl ActionHandler for CapacityHandler {
    async fn invoke(
        &self,
        _: &InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        Ok(json!(null))
    }
}
#[tonic::async_trait]
impl JobHandler for CapacityHandler {
    async fn run(
        &self,
        _: &InvocationClaims,
        _: &str,
        input: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        if input["wait"] == true {
            self.entered.add_permits(1);
            self.release.notified().await;
        }
        Ok(json!({"ran":true}))
    }
}

pub async fn verify_saturation() {
    // Capacity, rather than a one-connection authorization pool, is the limit under test here.
    let f = fixture::JobFixture::start_with_connections(16).await;
    let host_path = f.directory.path().join("host.sock");
    let mut socket = UnixSocket::bind(&host_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = f.host.clone().rpc();
    let _host = Task(tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    }));
    let host = HostClient::connect_unix(&host_path).await.unwrap();
    let handler = Arc::new(CapacityHandler {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Notify::new(),
    });
    let path = f.directory.path().join("runtime.sock");
    let socket = UnixSocket::bind(&path).unwrap();
    let runtime = Runtime::new(
        Manifest::parse(&serde_json::to_vec(&f.manifest).unwrap()).unwrap(),
        1,
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]).verifying_key(),
        Arc::new(
            RemoteAuthority::new(
                host,
                "job-fixture".into(),
                1,
                vec![CorePeer::Unix {
                    uid: rustix::process::geteuid().as_raw(),
                }],
            )
            .unwrap(),
        ),
        handler.clone(),
    )
    .unwrap()
    .with_jobs(handler.clone());
    let _runtime = Task(tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap();
    }));
    let client = webdock_plugin_runtime::connect_unix(&path).await.unwrap();
    let blocker = f
        .host
        .jobs_enqueue(
            &f.handle("start", "capacity-parent").await,
            json!({"name":"work","payload":{"wait":true},"idempotencyKey":"capacity-blocker"}),
        )
        .await
        .unwrap();
    let blocker_lease = f.host.claim_job(None).await.unwrap().unwrap();
    assert_eq!(blocker_lease.id(), blocker["id"].as_str().unwrap());
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let mut client = client.clone();
        let token = blocker_lease.invocation().token.clone();
        calls.spawn(async move {
            client
                .run_job(wire::RunJobRequest {
                    invocation_token: token,
                    name: "work".into(),
                    input_json: br#"{"wait":true}"#.to_vec(),
                    configuration_json: b"{}".to_vec(),
                })
                .await
        });
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        handler.entered.acquire_many(32),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    let target = f.enqueue("capacity-retry").await;
    let connections = f.directory.path().join("connections.json");
    std::fs::write(&connections,json!({"runtimes":{"job-fixture":{"generation":1,"transport":{"kind":"unix","path":path}}}}).to_string()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&connections, std::fs::Permissions::from_mode(0o600)).unwrap();
    let endpoints =
        PluginEndpoints::new(f.host.clone(), RuntimeConnections::new(connections), None);
    assert!(endpoints.advance_job().await.unwrap());
    let read = f.handle("inspect", "capacity-read").await;
    let pending = f.host.jobs_get(&read, target.clone()).await.unwrap();
    assert_eq!(
        pending["state"], "waiting",
        "saturation must schedule a retry, not permanently fail"
    );
    assert_eq!(pending["attempts"], 1);
    handler.release.notify_waiters();
    while let Some(call) = calls.join_next().await {
        call.unwrap().unwrap();
    }
    f.host
        .complete_job(&blocker_lease, Ok(json!(null)))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(endpoints.advance_job().await.unwrap());
    let completed = f.host.jobs_get(&read, target).await.unwrap();
    assert_eq!(completed["state"], "completed");
    assert_eq!(completed["attempts"], 2);
    assert_eq!(completed["result"], json!({"ran":true}));
}
#[tonic::async_trait]
impl ActionHandler for Handler {
    async fn invoke(
        &self,
        claims: &InvocationClaims,
        token: &str,
        input: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        self.host
            .call(
                token,
                "jobs",
                match claims.action.as_str() {
                    "start" => "enqueue",
                    "cancel" => "cancel",
                    _ => "get",
                },
                input,
            )
            .await
    }
}
#[tonic::async_trait]
impl JobHandler for Handler {
    async fn run(
        &self,
        claims: &InvocationClaims,
        token: &str,
        input: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        if input["wait"] == true {
            let _guard = PendingDrop(self.dropped.clone());
            self.started.notify_one();
            std::future::pending::<()>().await;
        }
        self.host.call(token,"data","write",json!({"key":"rpc-effect","value":{"durable":true},"expectedRevision":null,"idempotencyKey":"effect-once"})).await?;
        if claims.job.as_ref().unwrap().attempt == 1 {
            return Err(PluginError::new(
                ErrorCode::Unavailable,
                "PRIVATE provider failure",
            ));
        }
        Ok(json!({"done":true}))
    }
}
pub async fn verify() {
    let f = fixture::JobFixture::start().await;
    let host_path = f.directory.path().join("host.sock");
    let mut host_socket = UnixSocket::bind(&host_path).unwrap();
    let incoming = host_socket.incoming().unwrap();
    let service = f.host.clone().rpc();
    let _host = Task(tokio::spawn(async move {
        let _socket = host_socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    }));
    let host = HostClient::connect_unix(&host_path).await.unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let handler = Arc::new(Handler {
        host: host.clone(),
        started: started.clone(),
        dropped: dropped.clone(),
    });
    let runtime_path = f.directory.path().join("runtime.sock");
    let socket = UnixSocket::bind(&runtime_path).unwrap();
    let runtime = Runtime::new(
        Manifest::parse(&serde_json::to_vec(&f.manifest).unwrap()).unwrap(),
        1,
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]).verifying_key(),
        Arc::new(
            RemoteAuthority::new(
                host.clone(),
                "job-fixture".into(),
                1,
                vec![CorePeer::Unix {
                    uid: rustix::process::geteuid().as_raw(),
                }],
            )
            .unwrap(),
        ),
        handler.clone(),
    )
    .unwrap()
    .with_jobs(handler);
    let _runtime = Task(tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap();
    }));
    let mut client = webdock_plugin_runtime::connect_unix(&runtime_path)
        .await
        .unwrap();
    let connections = f.directory.path().join("connections.json");
    std::fs::write(&connections,json!({"runtimes":{"job-fixture":{"generation":1,"transport":{"kind":"unix","path":runtime_path}}}}).to_string()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&connections, std::fs::Permissions::from_mode(0o600)).unwrap();
    let parent = f.admit("start", "rpc-parent").await;
    // The runtime's action invokes the real authenticated Host RPC, not a host test adapter.
    let reply = client
        .invoke(wire::InvokeRequest {
            invocation_token: parent.token.clone(),
            action: "start".into(),
            input_json: serde_json::to_vec(
                &json!({"name":"work","payload":{},"idempotencyKey":"rpc-enqueue"}),
            )
            .unwrap(),
            configuration_json: b"{}".to_vec(),
        })
        .await;
    // The initial admission pins input; issue the correct pinned parent below instead.
    assert!(reply.is_err());
    f.host.revoke_issued(&parent.token).await.unwrap();
    let parent=f.host.admit_with_authority(&f.actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"job-fixture","projectID":"124","action":"start","input":{"name":"work","payload":{},"idempotencyKey":"rpc-enqueue"},"idempotencyKey":"rpc-parent-pinned"})).unwrap(),true,&webdock_browser_session::StoredAuthority::LocalSession{}).await.unwrap();
    let reply = client
        .invoke(wire::InvokeRequest {
            invocation_token: parent.token.clone(),
            action: "start".into(),
            input_json: serde_json::to_vec(&parent.input).unwrap(),
            configuration_json: b"{}".to_vec(),
        })
        .await
        .unwrap()
        .into_inner();
    let job: Value = serde_json::from_slice(&reply.output_json).unwrap();
    f.host.revoke_issued(&parent.token).await.unwrap();
    let endpoints = PluginEndpoints::new(
        f.host.clone(),
        RuntimeConnections::new(connections.clone()),
        None,
    );
    assert!(endpoints.advance_job().await.unwrap());
    let read = f.handle("inspect", "rpc-inspect").await;
    assert_eq!(
        f.host.jobs_get(&read, job.clone()).await.unwrap()["state"],
        "waiting"
    );
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let restarted = Arc::new(PluginEndpoints::new(
        f.host.clone(),
        RuntimeConnections::new(connections),
        None,
    ));
    assert!(restarted.advance_job().await.unwrap());
    let state = f.host.jobs_get(&read, job).await.unwrap();
    assert_eq!(state["state"], "completed");
    assert_eq!(state["attempts"], 2);
    assert_eq!(state["result"], json!({"done":true}));
    assert_eq!(scalar(&f.owner,"SELECT to_jsonb(revision) AS value FROM webdock_auth.plugin_document WHERE key='rpc-effect'").await,json!(1));
    assert_eq!(
        scalar(
            &f.owner,
            "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.plugin_document_receipt"
        )
        .await,
        json!(1)
    );
    let pending = f
        .host
        .jobs_enqueue(
            &f.handle("start", "rpc-wait-parent").await,
            json!({"name":"work","payload":{"wait":true},"idempotencyKey":"rpc-wait"}),
        )
        .await
        .unwrap();
    let active = restarted.clone();
    let dispatch = tokio::spawn(async move { active.advance_job().await });
    tokio::time::timeout(std::time::Duration::from_secs(10), started.notified())
        .await
        .unwrap();
    let cancel = f.admit("cancel", "rpc-cancel-action").await;
    host.call(&cancel.token, "jobs", "cancel", pending.clone())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), dispatch)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    );
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(
        f.host.jobs_get(&read, pending).await.unwrap()["state"],
        "cancelled"
    );
    assert!(!restarted.advance_job().await.unwrap());
}
