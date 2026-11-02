use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use webdock_plugin_protocol::{InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::{
    ActionHandler, InvocationAuthority, JobHandler, PeerIdentity, Runtime, UnixSocket,
    connect_unix, wire,
};

struct Authority(AtomicBool);
#[tonic::async_trait]
impl InvocationAuthority for Authority {
    async fn authorize_peer(&self, _: &PeerIdentity) -> Result<(), PluginError> {
        Ok(())
    }
    async fn authorize_invocation(
        &self,
        _: &PeerIdentity,
        _: &InvocationClaims,
    ) -> Result<(), PluginError> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(PluginError::denied())
        }
    }
    async fn authorize_request(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
        _: &[u8],
        _: &[u8],
    ) -> Result<(), PluginError> {
        self.authorize_invocation(peer, claims).await
    }
}
struct Handler;
#[tonic::async_trait]
impl ActionHandler for Handler {
    async fn invoke(
        &self,
        _: &InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        Ok(json!({"action":true}))
    }
}
#[tonic::async_trait]
impl JobHandler for Handler {
    async fn run(
        &self,
        claims: &InvocationClaims,
        _: &str,
        input: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        if let Some(seconds) = input["seconds"].as_u64() {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
        }
        if input["oversize"] == true {
            return Ok(json!("x".repeat(32768)));
        }
        if input["message"].as_str().is_some_and(|s| s.len() > 32000) {
            return Ok(json!("x".repeat(32766)));
        }
        Ok(json!({"job":claims.job,"message":input["message"]}))
    }
}
fn manifest() -> Manifest {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    value["jobs"] = json!([{"name":"notes.read","capabilities":["plugin.data.read"],"input":{"type":"object","additionalProperties":false,"properties":{"message":{"type":"string"},"seconds":{"type":"integer"},"oversize":{"type":"boolean"}}}}]);
    Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
}
fn request(job: bool, input: Value) -> wire::RunJobRequest {
    request_with_lifetime(job, input, 60)
}
fn request_with_lifetime(job: bool, input: Value, lifetime: u64) -> wire::RunJobRequest {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut claims = json!({"v":1,"id":"123e4567-e89b-42d3-a456-426614174000","pluginID":"project-notes","generation":1,"action":"notes.read","projectID":"123","customerID":"101","iat":now,"exp":now+lifetime});
    if job {
        claims["job"] = json!({"id":format!("job-{}","a".repeat(64)),"attempt":2});
    }
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let token = format!(
        "{body}.{}",
        URL_SAFE_NO_PAD.encode(
            SigningKey::from_bytes(&[7; 32])
                .sign(body.as_bytes())
                .to_bytes()
        )
    );
    wire::RunJobRequest {
        invocation_token: token,
        name: "notes.read".into(),
        input_json: serde_json::to_vec(&input).unwrap(),
        configuration_json: b"{}".to_vec(),
    }
}
struct Server {
    _directory: tempfile::TempDir,
    client: wire::plugin_runtime_client::PluginRuntimeClient<tonic::transport::Channel>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(
    handler: Option<Arc<dyn JobHandler>>,
    authority: Arc<dyn InvocationAuthority>,
) -> Server {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("plugin.sock");
    let socket = UnixSocket::bind(&path).unwrap();
    let mut runtime = Runtime::new(
        manifest(),
        1,
        SigningKey::from_bytes(&[7; 32]).verifying_key(),
        authority,
        Arc::new(Handler),
    )
    .unwrap();
    if let Some(handler) = handler {
        runtime = runtime.with_jobs(handler);
    }
    let task = tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap();
    });
    let client = connect_unix(&path).await.unwrap();
    Server {
        _directory: directory,
        client,
        task,
    }
}
fn authority() -> Arc<Authority> {
    Arc::new(Authority(AtomicBool::new(true)))
}

struct OccupiedSlots {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Notify,
}
#[tonic::async_trait]
impl JobHandler for OccupiedSlots {
    async fn run(
        &self,
        _: &InvocationClaims,
        _: &str,
        input: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        if input["seconds"] == 1 {
            self.entered.add_permits(1);
            self.release.notified().await;
        }
        if input["oversize"] == true {
            return Ok(json!("x".repeat(32768)));
        }
        Ok(json!({"ran":true}))
    }
}
#[tokio::test]
async fn job_slot_saturation_is_retryable_and_wire_bounds_stay_terminal() {
    let handler = Arc::new(OccupiedSlots {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Notify::new(),
    });
    let mut server = server(Some(handler.clone()), authority()).await;
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let mut client = server.client.clone();
        calls.spawn(async move { client.run_job(request(true, json!({"seconds":1}))).await });
    }
    tokio::time::timeout(Duration::from_secs(10), handler.entered.acquire_many(32))
        .await
        .unwrap()
        .unwrap()
        .forget();
    // A distinct connection reaches the global semaphore rather than queuing behind the
    // existing connection's independent transport concurrency limit.
    let mut saturated = connect_unix(server._directory.path().join("plugin.sock"))
        .await
        .unwrap();
    let unavailable = saturated
        .run_job(request(true, json!({})))
        .await
        .unwrap_err();
    assert_eq!(unavailable.code(), tonic::Code::Unavailable);
    handler.release.notify_waiters();
    while let Some(call) = calls.join_next().await {
        call.unwrap().unwrap();
    }
    server
        .client
        .run_job(request(true, json!({})))
        .await
        .unwrap();
    assert_eq!(
        server
            .client
            .run_job(request(true, json!({"message":"x".repeat(32768)})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::ResourceExhausted
    );
    assert_eq!(
        server
            .client
            .run_job(request(true, json!({"oversize":true})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::ResourceExhausted
    );
}

struct CrossExpiry {
    payload_phase: usize,
    payload_checks: AtomicUsize,
    invocation_checks_after_expiry: AtomicUsize,
}
impl CrossExpiry {
    fn at(payload_phase: usize) -> Arc<Self> {
        Arc::new(Self {
            payload_phase,
            payload_checks: AtomicUsize::new(0),
            invocation_checks_after_expiry: AtomicUsize::new(0),
        })
    }
}
#[tonic::async_trait]
impl InvocationAuthority for CrossExpiry {
    async fn authorize_peer(&self, _: &PeerIdentity) -> Result<(), PluginError> {
        Ok(())
    }
    async fn authorize_invocation(
        &self,
        _: &PeerIdentity,
        claims: &InvocationClaims,
    ) -> Result<(), PluginError> {
        if SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            >= claims.exp as u64
        {
            self.invocation_checks_after_expiry
                .fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn authorize_request(
        &self,
        _: &PeerIdentity,
        claims: &InvocationClaims,
        _: &[u8],
        _: &[u8],
    ) -> Result<(), PluginError> {
        if self.payload_checks.fetch_add(1, Ordering::SeqCst) + 1 == self.payload_phase {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let remaining = Duration::from_secs(claims.exp as u64).saturating_sub(now);
            tokio::time::sleep(remaining + Duration::from_millis(20)).await;
        }
        Ok(())
    }
}

struct RecordHandlerEntry(Arc<AtomicBool>);
impl JobHandler for RecordHandlerEntry {
    // Observe entry before the returned future is polled: the interval's first tick could
    // otherwise reject expiry before the handler branch and mask this regression.
    fn run<'a, 'b, 'c, 'future>(
        &'a self,
        _: &'b InvocationClaims,
        _: &'c str,
        _: Value,
        _: Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, PluginError>> + Send + 'future>,
    >
    where
        'a: 'future,
        'b: 'future,
        'c: 'future,
        Self: 'future,
    {
        self.0.store(true, Ordering::SeqCst);
        Box::pin(async { Ok(json!({"result":"public"})) })
    }
}

#[tokio::test]
async fn token_expiry_during_payload_authorization_prevents_handler_entry() {
    let authority = CrossExpiry::at(1);
    let entered = Arc::new(AtomicBool::new(false));
    let mut server = server(
        Some(Arc::new(RecordHandlerEntry(entered.clone()))),
        authority.clone(),
    )
    .await;
    let result = server
        .client
        .run_job(request_with_lifetime(true, json!({}), 2))
        .await;
    assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
    assert!(
        !entered.load(Ordering::SeqCst),
        "expired invocation entered its handler"
    );
    assert_eq!(
        authority
            .invocation_checks_after_expiry
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn token_expiry_during_payload_authorization_withholds_final_result() {
    let authority = CrossExpiry::at(2);
    let entered = Arc::new(AtomicBool::new(false));
    let mut server = server(
        Some(Arc::new(RecordHandlerEntry(entered.clone()))),
        authority.clone(),
    )
    .await;
    let result = server
        .client
        .run_job(request_with_lifetime(true, json!({}), 2))
        .await;
    assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
    assert!(entered.load(Ordering::SeqCst));
    assert_eq!(authority.payload_checks.load(Ordering::SeqCst), 2);
    assert_eq!(
        authority
            .invocation_checks_after_expiry
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn job_kind_is_signed_and_never_dispatched_as_an_action() {
    let mut server = server(Some(Arc::new(Handler)), authority()).await;
    let job = request(true, json!({"message":"hello"}));
    let action = wire::InvokeRequest {
        invocation_token: job.invocation_token.clone(),
        action: job.name.clone(),
        input_json: job.input_json.clone(),
        configuration_json: job.configuration_json.clone(),
    };
    assert_eq!(
        server.client.invoke(action).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        server
            .client
            .run_job(request(false, json!({})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    let result = server.client.run_job(job).await.unwrap().into_inner();
    assert_eq!(
        serde_json::from_slice::<Value>(&result.output_json).unwrap(),
        json!({"job":{"id":format!("job-{}","a".repeat(64)),"attempt":2},"message":"hello"})
    );
}

#[tokio::test]
async fn jobs_require_opt_in_matching_names_and_declared_input() {
    let mut absent = server(None, authority()).await;
    assert_eq!(
        absent
            .client
            .run_job(request(true, json!({})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unimplemented
    );
    let mut server = server(Some(Arc::new(Handler)), authority()).await;
    let mut wrong = request(true, json!({}));
    wrong.name = "notes.write".into();
    assert_eq!(
        server.client.run_job(wrong).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    let mut unknown = request(true, json!({}));
    let (body, _) = unknown.invocation_token.split_once('.').unwrap();
    let mut claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).unwrap()).unwrap();
    claims["action"] = json!("notes.write");
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    unknown.invocation_token = format!(
        "{body}.{}",
        URL_SAFE_NO_PAD.encode(
            SigningKey::from_bytes(&[7; 32])
                .sign(body.as_bytes())
                .to_bytes()
        )
    );
    unknown.name = "notes.write".into();
    assert_eq!(
        server.client.run_job(unknown).await.unwrap_err().code(),
        tonic::Code::NotFound
    );
    assert_eq!(
        server
            .client
            .run_job(request(true, json!({"unexpected":true})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    let mut invalid = request(true, json!({}));
    invalid.configuration_json = br#"{"unexpected":true}"#.to_vec();
    assert_eq!(
        server.client.run_job(invalid).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
}

#[tokio::test]
async fn job_input_and_result_have_independent_32k_limits() {
    let mut server = server(Some(Arc::new(Handler)), authority()).await;
    let boundary = request(true, json!({"message":"x".repeat(32754)}));
    assert_eq!(boundary.input_json.len(), 32768);
    assert_eq!(
        server
            .client
            .run_job(boundary)
            .await
            .unwrap()
            .into_inner()
            .output_json
            .len(),
        32768
    );
    assert_eq!(
        server
            .client
            .run_job(request(true, json!({"message":"x".repeat(32768)})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::ResourceExhausted
    );
    assert_eq!(
        server
            .client
            .run_job(request(true, json!({"oversize":true})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::ResourceExhausted
    );
}

struct RevokeOnCompletion(Arc<Authority>);
#[tonic::async_trait]
impl JobHandler for RevokeOnCompletion {
    async fn run(
        &self,
        _: &InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        self.0.0.store(false, Ordering::SeqCst);
        Ok(json!({"withheld":true}))
    }
}
#[tokio::test]
async fn job_result_is_withheld_after_authority_revocation_at_completion() {
    let grant = authority();
    let mut server = server(Some(Arc::new(RevokeOnCompletion(grant.clone()))), grant).await;
    assert_eq!(
        server
            .client
            .run_job(request(true, json!({})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
}

struct DropMarker(Arc<AtomicBool>);
impl Drop for DropMarker {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct Pending(Arc<AtomicBool>);
#[tonic::async_trait]
impl JobHandler for Pending {
    async fn run(
        &self,
        _: &InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        let _marker = DropMarker(self.0.clone());
        std::future::pending().await
    }
}
#[tokio::test]
async fn cancelled_job_authority_drops_the_pending_handler() {
    let grant = authority();
    let dropped = Arc::new(AtomicBool::new(false));
    let server = server(Some(Arc::new(Pending(dropped.clone()))), grant.clone()).await;
    let mut client = server.client.clone();
    let call = tokio::spawn(async move { client.run_job(request(true, json!({}))).await });
    tokio::time::sleep(Duration::from_millis(80)).await;
    grant.0.store(false, Ordering::SeqCst);
    assert_eq!(
        call.await.unwrap().unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn caller_cancellation_drops_the_pending_job() {
    let dropped = Arc::new(AtomicBool::new(false));
    let mut server = server(Some(Arc::new(Pending(dropped.clone()))), authority()).await;
    let mut call = tonic::Request::new(request(true, json!({})));
    call.set_timeout(Duration::from_millis(100));
    assert!(server.client.run_job(call).await.is_err());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn jobs_can_exceed_the_old_35_second_transport_limit() {
    let mut server = server(Some(Arc::new(Handler)), authority()).await;
    assert!(
        server
            .client
            .run_job(request(true, json!({"seconds":36})))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn job_deadline_drops_the_handler_after_45_seconds() {
    let dropped = Arc::new(AtomicBool::new(false));
    let mut server = server(
        Some(Arc::new(Pending(dropped.clone()))),
        Arc::new(SlowAtDeadline(tokio::time::Instant::now())),
    )
    .await;
    let start = tokio::time::Instant::now();
    assert_eq!(
        server
            .client
            .run_job(request(true, json!({})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::DeadlineExceeded
    );
    assert!(start.elapsed() >= Duration::from_secs(45));
    assert!(start.elapsed() < Duration::from_millis(46500));
    assert!(dropped.load(Ordering::SeqCst));
}

struct SlowAtDeadline(tokio::time::Instant);
#[tonic::async_trait]
impl InvocationAuthority for SlowAtDeadline {
    async fn authorize_peer(&self, _: &PeerIdentity) -> Result<(), PluginError> {
        Ok(())
    }
    async fn authorize_invocation(
        &self,
        _: &PeerIdentity,
        _: &InvocationClaims,
    ) -> Result<(), PluginError> {
        if self.0.elapsed() >= Duration::from_secs(44) {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Ok(())
    }
    async fn authorize_request(
        &self,
        _: &PeerIdentity,
        _: &InvocationClaims,
        _: &[u8],
        _: &[u8],
    ) -> Result<(), PluginError> {
        Ok(())
    }
}
