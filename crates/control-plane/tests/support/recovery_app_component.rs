//! One genuine owned source component. Faults affect replies only, never authority/state rows.
use super::*;
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tonic::{Request as RpcRequest, Response, Status};
use webdock_agent_protocol::{
    self as wire, agent_control_server::AgentControl, recovery_control_server::RecoveryControl,
};
#[derive(Default)]
pub(super) struct ControlState {
    agent_pid: AtomicUsize,
    helper_pid: AtomicUsize,
    pub stopped: AtomicBool,
    pub release_lost: AtomicBool,
    pub release_replay_allowed: AtomicBool,
    pub stopped_ack_allowed: AtomicBool,
    pub release_denied: AtomicUsize,
    pub release_replays: AtomicUsize,
    pub capture_denied: AtomicUsize,
    pub hold_normal: AtomicBool,
    pub normal_received: AtomicBool,
    pub normal_ack_allowed: AtomicBool,
    pub bind_probe_requested: AtomicBool,
    pub bind_probe_done: AtomicBool,
    pub bound_claim: StdMutex<Option<wire::recovery::RecoveryClaim>>,
    pub changed: tokio::sync::Notify,
    pub stopped_request: StdMutex<Option<wire::RecoveryCheckpointRequest>>,
    pub released_request: StdMutex<Option<wire::RecoveryCheckpointRequest>>,
}
pub(super) fn enabled() -> bool {
    std::env::var("WEBDOCK_RECOVERY_AUTHORITY_COMPONENT").as_deref() == Ok("1")
        && !super::byte_component::enabled()
        && !super::mail_component::enabled()
}
pub(super) fn safe_phase(value: Option<&str>) -> &'static str {
    const PHASES: &[&str] = &[
        "preflight",
        "app-admit",
        "app-stopped",
        "app-captured",
        "uploaded-verified",
        "app-resume-intent",
        "app-resumed",
        "canonical-fencing",
        "all-writers-stopped",
        "matched-set-verified",
        "same-fence-compensation",
        "checkpoint",
        "fencing",
        "fenced",
        "resume",
        "source-admit",
        "source-drained",
    ];
    value
        .map(|value| {
            PHASES
                .iter()
                .copied()
                .find(|known| *known == value)
                .unwrap_or("other")
        })
        .unwrap_or("absent")
}
fn safe_purpose(value: Option<&str>) -> &'static str {
    match value {
        Some("preflight") => "preflight",
        Some("stop") => "stop",
        Some("capture") => "capture",
        Some("upload") => "upload",
        Some("resume") => "resume",
        Some("release") => "release",
        None => "absent",
        _ => "other",
    }
}
fn rpc_line(
    boundary: &'static str,
    event: &'static str,
    phase: &'static str,
    code: tonic::Code,
) -> String {
    format!(
        "recovery_component recovery_rpc boundary={boundary} event={event} phase={phase} tonicCode={}",
        code as i32
    )
}
fn rpc_marker<T>(boundary: &'static str, phase: &'static str, result: &Result<T, Status>) {
    eprintln!(
        "{}",
        rpc_line(
            boundary,
            "result",
            phase,
            result
                .as_ref()
                .err()
                .map(Status::code)
                .unwrap_or(tonic::Code::Ok)
        )
    );
}
fn safe_enum(value: &Value, allowed: &'static [&'static str]) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    json!(
        value
            .as_str()
            .and_then(|s| allowed.iter().copied().find(|known| *known == s))
            .unwrap_or("other")
    )
}
fn safe_canonical(raw: Value) -> Value {
    let keys = raw["callbackKeys"].as_array().map(|keys| {
        keys.iter()
            .take(16)
            .map(|key| {
                safe_enum(
                    key,
                    &[
                        "checkpoint:preflight",
                        "checkpoint:app-admit",
                        "checkpoint:app-stopped",
                        "checkpoint:app-captured",
                        "checkpoint:uploaded-verified",
                        "checkpoint:app-resume-intent",
                        "checkpoint:app-resumed",
                        "fencing:canonical-fencing",
                        "fenced:all-writers-stopped",
                        "resume:same-fence-compensation",
                    ],
                )
            })
            .collect::<Vec<_>>()
    });
    json!({"operationStatus":safe_enum(&raw["operationStatus"],&["queued","running","succeeded","failed","needs-reconciliation"]),"generation":raw["generation"].as_u64().filter(|n|i32::try_from(*n).is_ok()),"jobPhase":if raw["jobPhase"].is_null(){Value::Null}else{raw["jobPhase"].as_str().map(|phase|json!(safe_phase(Some(phase)))).unwrap_or_else(||json!("other"))},"fenceState":safe_enum(&raw["fenceState"],&["draining","drained","resumed","uncertain","stopped","captured","resuming","released"]),"fenceExists":raw["fenceExists"].as_bool(),"jobBound":raw["jobBound"].as_bool(),"callbacks":raw["callbacks"].as_u64().filter(|n|*n<=1000000),"events":raw["events"].as_u64().filter(|n|*n<=1000000),"callbackKeys":keys,"appStatus":safe_enum(&raw["appStatus"],&["draft","pending","ready","failed","deleted","restoring"])})
}
fn completion_summary(raw: &[u8]) -> Value {
    if raw.len() > 65536 {
        return json!({"status":"unavailable"});
    }
    let Ok(value) = serde_json::from_slice::<Value>(raw) else {
        return json!({"status":"unavailable"});
    };
    json!({"state":safe_enum(&value["state"], &["succeeded","failed","needs-reconciliation"]),"errorCode":safe_enum(&value["errorCode"], &["recovery_authority_or_integrity_denied","recovery_deadline","recovery_execution_uncertain"])})
}
fn reservation_marker_projection(line: &str) -> Option<Value> {
    let fields: Vec<_> = line.split(' ').collect();
    let ["recovery_component", "reservation", actor, stage, result] = fields.as_slice() else {
        return None;
    };
    let actor = actor.strip_prefix("actor=")?;
    let stage = stage.strip_prefix("stage=")?;
    let result = result.strip_prefix("result=")?;
    let valid = match (actor, stage) {
        ("agent", "outer_enter" | "work_first_poll") | ("helper", "enter") => result == "none",
        ("helper", "plan_verified") => result == "Ok",
        ("agent", "rpc_result" | "guarded_result") | ("helper", "result") => [
            "Ok",
            "Invalid",
            "Denied",
            "Expired",
            "Fenced",
            "Conflict",
            "Ownership",
            "Unsupported",
            "Runtime",
            "Journal",
        ]
        .contains(&result),
        _ => false,
    };
    valid.then(|| json!({"actor":actor,"stage":stage,"result":result}))
}
pub(super) fn reservation_markers(path: &Path) -> Value {
    let Ok(directory) = diagnostic_directory(path) else {
        return json!({"status":"unavailable"});
    };
    let mut logs = Vec::new();
    for name in [
        "agent-profile-trusted.stderr",
        "agent-missing-journal.stderr",
        "agent-restarted.stderr",
        "storage-trusted.stderr",
    ] {
        let observation = (|| -> io::Result<Value> {
            let file = diagnostic_file(&directory, name)?;
            let mut raw = Vec::new();
            file.take(DIAGNOSTIC_LIMIT + 1).read_to_end(&mut raw)?;
            if raw.len() as u64 > DIAGNOSTIC_LIMIT {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let text = std::str::from_utf8(&raw).map_err(|_| io::ErrorKind::InvalidData)?;
            let rows: Vec<_> = text
                .lines()
                .filter_map(reservation_marker_projection)
                .take(32)
                .collect();
            Ok(json!({"file":name,"markers":rows}))
        })();
        logs.push(observation.unwrap_or_else(|_| json!({"file":name,"status":"unavailable"})));
    }
    json!(logs)
}
fn pending_outbox_summary(path: &Path) -> Value {
    if let Err(error) = std::fs::symlink_metadata(path) {
        return json!({"status":if error.kind()==io::ErrorKind::NotFound {"missing"}else{"unavailable"}});
    }
    match webdock_agent::outbox::RecoveryOutbox::open(path).and_then(|outbox| outbox.pending()) {
        Ok(None) => json!({"pending":false}),
        Ok(Some(delivery)) => match delivery.callback {
            Some(wire::recovery_delivery::Callback::Checkpoint(checkpoint)) => {
                let phase = if checkpoint.checkpoint_json.len() <= 65536 {
                    serde_json::from_slice::<Value>(&checkpoint.checkpoint_json)
                        .ok()
                        .map(|v| safe_phase(v["phase"].as_str()))
                } else {
                    None
                };
                json!({"pending":true,"kind":"checkpoint","phase":phase.unwrap_or("other")})
            }
            Some(wire::recovery_delivery::Callback::Receipt(_)) => {
                json!({"pending":true,"kind":"receipt"})
            }
            None => json!({"status":"unavailable"}),
        },
        Err(_) => json!({"status":"unavailable"}),
    }
}
#[test]
fn reservation_and_completion_projection_are_strict_and_payload_free() {
    let poison = "PRIVATE-token-https://private.invalid";
    let completion = completion_summary(&serde_json::to_vec(&json!({"state":"needs-reconciliation","errorCode":"recovery_execution_uncertain","proof":poison,"token":poison})).unwrap());
    assert_eq!(
        completion,
        json!({"state":"needs-reconciliation","errorCode":"recovery_execution_uncertain"})
    );
    assert!(!completion.to_string().contains(poison));
    let unknown = completion_summary(
        &serde_json::to_vec(&json!({"state":poison,"errorCode":poison})).unwrap(),
    );
    assert_eq!(unknown, json!({"state":"other","errorCode":"other"}));
    assert_eq!(completion_summary(b"not JSON")["status"], "unavailable");
    for actor in ["agent", "helper"] {
        let stage = if actor == "agent" {
            "rpc_result"
        } else {
            "result"
        };
        for variant in [
            "Ok",
            "Invalid",
            "Unsupported",
            "Runtime",
            "Denied",
            "Expired",
            "Fenced",
            "Conflict",
            "Ownership",
            "Journal",
        ] {
            let line = format!(
                "recovery_component reservation actor={actor} stage={stage} result={variant}"
            );
            assert!(reservation_marker_projection(&line).is_some());
            assert!(reservation_marker_projection(&format!("{line} {poison}")).is_none());
        }
    }
    for line in [
        "recovery_component reservation actor=agent stage=outer_enter result=none",
        "recovery_component reservation actor=helper stage=plan_verified result=Ok",
    ] {
        assert!(reservation_marker_projection(line).is_some());
    }
    for line in [
        "recovery_component reservation actor=helper stage=plan_verified result=Runtime",
        "recovery_component reservation actor=agent stage=rpc_result result=none",
        "recovery_component reservation actor=foreign stage=result result=Ok",
        "recovery_component reservation actor=helper stage=result result=PRIVATE",
    ] {
        assert!(reservation_marker_projection(line).is_none());
    }
}
#[test]
fn pending_outbox_projection_is_read_only_and_omits_claims_and_tokens() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.path().join("recovery-outbox.json");
    assert_eq!(pending_outbox_summary(&path)["status"], "missing");
    assert!(!path.exists());
    let private = b"PRIVATE-unittest-token-url-https://private.invalid";
    let delivery = wire::RecoveryDelivery {
        callback: Some(wire::recovery_delivery::Callback::Checkpoint(
            wire::RecoveryCheckpointRequest {
                identity: None,
                claim_json: private.to_vec(),
                phase: "checkpoint".into(),
                checkpoint_json: serde_json::to_vec(
                    &json!({"phase":"app-admit","private":"PRIVATE-unittest-token"}),
                )
                .unwrap(),
            },
        )),
    };
    let mut outbox = webdock_agent::outbox::RecoveryOutbox::open(&path).unwrap();
    outbox.put(&delivery).unwrap();
    let before = std::fs::read(&path).unwrap();
    let summary = pending_outbox_summary(&path);
    assert_eq!(
        summary,
        json!({"pending":true,"kind":"checkpoint","phase":"app-admit"})
    );
    assert!(!summary.to_string().contains("PRIVATE"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    outbox.acknowledge(&delivery).unwrap();
    let receipt = wire::RecoveryDelivery {
        callback: Some(wire::recovery_delivery::Callback::Receipt(
            wire::RecoveryReceipt {
                identity: None,
                claim_json: private.to_vec(),
                completion_json: private.to_vec(),
            },
        )),
    };
    outbox.put(&receipt).unwrap();
    assert_eq!(
        pending_outbox_summary(&path),
        json!({"pending":true,"kind":"receipt"})
    );
}
#[test]
fn causal_projection_never_emits_arbitrary_rpc_or_database_fields() {
    let secret = "PRIVATE-token-url-https://private.invalid";
    assert_eq!(safe_phase(Some(secret)), "other");
    assert_eq!(safe_phase(Some("app-admit")), "app-admit");
    let line = rpc_line(
        "checkpoint_recovery",
        "result",
        safe_phase(Some(secret)),
        tonic::Code::PermissionDenied,
    );
    assert!(!line.contains(secret));
    assert!(line.contains("tonicCode=7"));
    let value = safe_canonical(
        json!({"operationStatus":secret,"generation":2,"jobPhase":secret,"fenceState":"draining","fenceExists":true,"jobBound":true,"callbacks":3,"events":1,"callbackKeys":["checkpoint:preflight",secret],"appStatus":"ready","claim":secret,"environment":secret}),
    );
    assert!(!value.to_string().contains(secret));
    assert!(value.get("claim").is_none());
    assert_eq!(value["jobPhase"], "other");
    assert_eq!(value["callbackKeys"][0], "checkpoint:preflight");
}
/// Actual function on the genuine product-created draft and authenticated enrollment.
/// No old function replacement or authority/state row writes are used.
pub(super) async fn normal_guard_lock_compatibility(
    hosting: &webdock_hosting::HostingService,
    cluster: &str,
    enrollment: &Value,
    app: &str,
) {
    let agent = hosting
        .authenticate_agent(cluster, enrollment["credential"].as_str().unwrap())
        .await
        .unwrap();
    let first = hosting.begin_authenticated_agent(&agent).await.unwrap();
    let second = hosting.begin_authenticated_agent(&agent).await.unwrap();
    for tx in [&first, &second] {
        let result = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.recovery_app_effect_allowed($1) AS allowed",
                [app.into()],
            ))
            .await;
        assert!(result.is_ok(), "actual simultaneous normal guard failed");
        assert!(
            result
                .unwrap()
                .unwrap()
                .try_get::<bool>("", "allowed")
                .unwrap()
        );
    }
    let writer = hosting.begin_authenticated_agent(&agent).await.unwrap();
    let blocked = writer
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.hosting_app WHERE id=$1 FOR UPDATE NOWAIT",
            [app.into()],
        ))
        .await;
    let code = match &blocked {
        Err(sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(error))) => {
            error.as_database_error().and_then(|error| error.code())
        }
        _ => None,
    };
    assert_eq!(
        code.as_deref(),
        Some("55P03"),
        "shared guards must exclude an app writer"
    );
    writer.rollback().await.unwrap();
    first.rollback().await.unwrap();
    second.rollback().await.unwrap();
    let writer = hosting.begin_authenticated_agent(&agent).await.unwrap();
    let locked = writer
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.hosting_app WHERE id=$1 FOR UPDATE NOWAIT",
            [app.into()],
        ))
        .await;
    assert!(
        matches!(locked, Ok(Some(_))),
        "exclusive app lock unavailable after full guard rollback"
    );
    writer.rollback().await.unwrap();
    eprintln!(
        "recovery_component normal_guard_shared_readers=true exclusive_writer_nowait=true rollback_releases=true"
    );
}
impl ControlState {
    fn flags(&self) -> Value {
        json!({"stopped":self.stopped.load(Ordering::SeqCst),"releaseLost":self.release_lost.load(Ordering::SeqCst),"releaseReplayAllowed":self.release_replay_allowed.load(Ordering::SeqCst),"stoppedAckAllowed":self.stopped_ack_allowed.load(Ordering::SeqCst),"releaseDenied":self.release_denied.load(Ordering::SeqCst),"releaseReplays":self.release_replays.load(Ordering::SeqCst),"captureDenied":self.capture_denied.load(Ordering::SeqCst),"holdNormal":self.hold_normal.load(Ordering::SeqCst),"normalReceived":self.normal_received.load(Ordering::SeqCst),"normalAckAllowed":self.normal_ack_allowed.load(Ordering::SeqCst),"bindProbeRequested":self.bind_probe_requested.load(Ordering::SeqCst),"bindProbeDone":self.bind_probe_done.load(Ordering::SeqCst),"boundClaim":self.bound_claim.lock().ok().map(|claim|claim.is_some())})
    }
    pub async fn until(&self, condition: impl Fn() -> bool) {
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let wake = self.changed.notified();
                tokio::pin!(wake);
                wake.as_mut().enable();
                if condition() {
                    break;
                }
                wake.await;
            }
        })
        .await;
        if result.is_err() {
            eprintln!(
                "recovery_component reply_gate_timeout flags={}",
                self.flags()
            );
        }
        result.expect("bounded real component phase");
    }
    pub fn signal(&self) {
        self.changed.notify_waiters();
    }
}
#[derive(Clone)]
struct Endpoint {
    core: webdock_agent_control::ControlPlane,
    state: Arc<ControlState>,
}
#[tonic::async_trait]
impl RecoveryControl for Endpoint {
    async fn poll_recovery(
        &self,
        r: RpcRequest<wire::PollRequest>,
    ) -> Result<Response<wire::RecoveryDispatch>, Status> {
        eprintln!(
            "{}",
            rpc_line("poll_recovery", "entry", "none", tonic::Code::Ok)
        );
        let result = self.core.poll_recovery(r).await;
        rpc_marker("poll_recovery", "none", &result);
        let response = result?;
        if self.state.bind_probe_requested.load(Ordering::SeqCst)
            && !self.state.bind_probe_done.load(Ordering::SeqCst)
            && !response.get_ref().claim_json.is_empty()
        {
            let claim = serde_json::from_slice(&response.get_ref().claim_json)
                .map_err(|_| Status::internal("Owned actual claim decoding"))?;
            *self.state.bound_claim.lock().unwrap() = Some(claim);
            self.state.signal();
            self.state
                .until(|| self.state.bind_probe_done.load(Ordering::SeqCst))
                .await;
        }
        Ok(response)
    }
    async fn guard_recovery(
        &self,
        r: RpcRequest<wire::RecoveryGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        let claim: Value = serde_json::from_slice(&r.get_ref().claim_json)
            .map_err(|_| Status::invalid_argument("Owned guard JSON"))?;
        let phase = claim["checkpoint"]["appGuard"]["phase"]
            .as_str()
            .unwrap_or("");
        let purpose = safe_purpose(claim["checkpoint"]["appGuard"]["phase"].as_str());
        eprintln!(
            "{}",
            rpc_line("guard_recovery", "entry", purpose, tonic::Code::Ok)
        );
        let result = self.core.guard_recovery(r).await;
        rpc_marker("guard_recovery", purpose, &result);
        if result.is_err() {
            match phase {
                "release" => {
                    self.state.release_denied.fetch_add(1, Ordering::SeqCst);
                }
                "capture" if self.state.stopped_ack_allowed.load(Ordering::SeqCst) => {
                    self.state.capture_denied.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
            self.state.signal();
        }
        result
    }
    async fn checkpoint_recovery(
        &self,
        r: RpcRequest<wire::RecoveryCheckpointRequest>,
    ) -> Result<Response<wire::RecoveryCheckpointReply>, Status> {
        let original = r.get_ref().clone();
        let value: Value = serde_json::from_slice(&original.checkpoint_json)
            .map_err(|_| Status::invalid_argument("Owned checkpoint JSON"))?;
        let phase = safe_phase(value["phase"].as_str().or(Some(original.phase.as_str())));
        eprintln!(
            "{}",
            rpc_line("checkpoint_recovery", "entry", phase, tonic::Code::Ok)
        );
        if value["phase"] == "app-resumed"
            && self.state.release_lost.load(Ordering::SeqCst)
            && !self.state.release_replay_allowed.load(Ordering::SeqCst)
        {
            eprintln!(
                "{}",
                rpc_line(
                    "checkpoint_recovery",
                    "reply",
                    phase,
                    tonic::Code::Unavailable
                )
            );
            return Err(Status::unavailable("Owned withheld resume ACK"));
        }
        let result = self.core.checkpoint_recovery(r).await;
        rpc_marker("checkpoint_recovery", phase, &result);
        let response = result?;
        if value["phase"] == "app-stopped" {
            *self.state.stopped_request.lock().unwrap() = Some(original);
            self.state.stopped.store(true, Ordering::SeqCst);
            self.state.signal();
            self.state
                .until(|| self.state.stopped_ack_allowed.load(Ordering::SeqCst))
                .await;
        } else if value["phase"] == "app-resumed"
            && !self.state.release_lost.swap(true, Ordering::SeqCst)
        {
            *self.state.released_request.lock().unwrap() = Some(original);
            self.state.signal();
            eprintln!(
                "{}",
                rpc_line(
                    "checkpoint_recovery",
                    "reply",
                    phase,
                    tonic::Code::Unavailable
                )
            );
            return Err(Status::unavailable("Owned dropped committed resume ACK"));
        }
        if value["phase"] == "app-resumed" {
            self.state.release_replays.fetch_add(1, Ordering::SeqCst);
            self.state.signal();
        }
        Ok(response)
    }
    async fn complete_recovery(
        &self,
        r: RpcRequest<wire::RecoveryReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        eprintln!(
            "{}",
            rpc_line("complete_recovery", "entry", "none", tonic::Code::Ok)
        );
        eprintln!(
            "recovery_component completion={}",
            completion_summary(&r.get_ref().completion_json)
        );
        let result = self.core.complete_recovery(r).await;
        rpc_marker("complete_recovery", "none", &result);
        result
    }
}
#[tonic::async_trait]
impl AgentControl for Endpoint {
    async fn register(
        &self,
        r: RpcRequest<wire::Hello>,
    ) -> Result<Response<wire::Welcome>, Status> {
        self.core.register(r).await
    }
    async fn heartbeat(
        &self,
        r: RpcRequest<wire::HeartbeatRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        self.core.heartbeat(r).await
    }
    async fn poll(
        &self,
        r: RpcRequest<wire::PollRequest>,
    ) -> Result<Response<wire::Dispatch>, Status> {
        self.core.poll(r).await
    }
    async fn guard(
        &self,
        r: RpcRequest<wire::GuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        self.core.guard(r).await
    }
    async fn complete(&self, r: RpcRequest<wire::Receipt>) -> Result<Response<wire::Ack>, Status> {
        if self.state.hold_normal.load(Ordering::SeqCst)
            && !self.state.normal_ack_allowed.load(Ordering::SeqCst)
        {
            self.state.normal_received.store(true, Ordering::SeqCst);
            self.state.signal();
            return Err(Status::unavailable(
                "Owned withheld precommit workload receipt",
            ));
        }
        self.core.complete(r).await
    }
}
pub(super) async fn serve(
    core: webdock_agent_control::ControlPlane,
    mut socket: webdock_plugin_runtime::UnixSocket,
    state: Arc<ControlState>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let endpoint = Endpoint {
        core: core.clone(),
        state,
    };
    tonic::transport::Server::builder()
        .timeout(Duration::from_secs(30))
        .concurrency_limit_per_connection(32)
        .add_service(core.clone().database_service())
        .add_service(
            wire::recovery_control_server::RecoveryControlServer::new(endpoint.clone())
                .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
                .max_encoding_message_size(wire::MAX_MESSAGE_BYTES),
        )
        .add_service(core.clone().mail_service())
        .add_service(core.clone().mail_configuration_service())
        .add_service(core.clone().workload_service())
        .add_service(core.clone().plugin_runtime_service())
        .add_service(
            wire::agent_control_server::AgentControlServer::new(endpoint)
                .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
                .max_encoding_message_size(wire::MAX_MESSAGE_BYTES),
        )
        .serve_with_incoming_shutdown(socket.incoming().unwrap(), shutdown)
        .await
        .unwrap();
    drop(socket);
}

pub(super) struct Context<'a> {
    pub hosting: &'a webdock_hosting::HostingService,
    pub enrollment: &'a Value,
    pub app: &'a Router,
    pub console: &'a str,
    pub owner: &'a DatabaseConnection,
    pub path: &'a Path,
    pub cluster: &'a str,
    pub source: &'a str,
    pub source_row: &'a Value,
    pub agent_config: &'a mut Value,
    pub agent_config_path: &'a Path,
    pub storage_config: &'a Path,
    pub storage_socket: &'a Path,
    pub control_path: &'a Path,
    pub agent: &'a mut tokio::process::Child,
    pub helper: &'a mut tokio::process::Child,
    pub state: Arc<ControlState>,
}
impl Context<'_> {
    async fn until(&mut self, condition: impl Fn(&ControlState) -> bool) {
        let state = self.state.clone();
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let wake = state.changed.notified();
                tokio::pin!(wake);
                wake.as_mut().enable();
                if condition(&state) {
                    break;
                }
                wake.await;
            }
        })
        .await;
        if result.is_err() {
            let processes = json!({"agent":diagnostic_child(self.agent.try_wait()),"helper":diagnostic_child(self.helper.try_wait())});
            let operation = state
                .bound_claim
                .lock()
                .ok()
                .and_then(|claim| claim.as_ref().map(|claim| claim.spec.operation_id.clone()));
            let query=self.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT jsonb_build_object('operationStatus',o.status,'generation',o.generation,'jobPhase',j.checkpoint->>'phase','jobBound',j.agent_identity IS NOT NULL,'fenceState',f.state,'fenceExists',f.operation_id IS NOT NULL,'callbacks',(SELECT count(*) FROM webdock_auth.hosting_recovery_callback cb WHERE cb.operation_id=j.operation_id),'events',(SELECT count(*) FROM webdock_auth.hosting_recovery_source_event se WHERE se.operation_id=j.operation_id),'callbackKeys',(SELECT coalesce(jsonb_agg(keys.phase_key),'[]'::jsonb) FROM (SELECT phase_key FROM webdock_auth.hosting_recovery_callback cb WHERE cb.operation_id=j.operation_id ORDER BY phase_key LIMIT 16) keys),'appStatus',a.status) AS value FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_recovery_policy p ON p.id=j.policy_id JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id LEFT JOIN webdock_auth.hosting_recovery_source_fence f ON f.operation_id=j.operation_id JOIN webdock_auth.hosting_app a ON a.id=p.source_app_id WHERE p.source_app_id=$1 AND ($2::text IS NULL OR j.operation_id=$2) ORDER BY j.created_at DESC LIMIT 1",
                vec![self.source.into(),operation.into()]));
            let canonical = match tokio::time::timeout(Duration::from_secs(2), query).await {
                Ok(Ok(Some(row))) => row
                    .try_get::<Value>("", "value")
                    .ok()
                    .map(safe_canonical)
                    .unwrap_or_else(|| json!({"status":"unavailable"})),
                _ => json!({"status":"unavailable"}),
            };
            let snapshot = json!({"scope":"owned_timeout_observations_only_no_readiness_or_drain","flags":state.flags(),"processes":processes,"canonical":canonical,"outbox":pending_outbox_summary(&self.path.join("agent-state/recovery-outbox.json")),"reservationMarkers":reservation_markers(self.path)});
            eprintln!("recovery_component component_wait_timeout snapshot={snapshot}");
            let target = Path::new("/diagnostics").join(format!(
                "component-timeout-{}.json",
                uuid::Uuid::new_v4().simple()
            ));
            if write_diagnostic_json(&target, &snapshot).is_err() {
                eprintln!("recovery_component component_wait_snapshot_retention=unavailable");
            }
        }
        result.expect("bounded real component phase");
    }
}
fn begin_policy(cluster: &str, source: &str, key: &str, capacity: u64) -> Value {
    json!({"action":"recovery.policies.grant","clusterID":cluster,"profile":"ntfy-sqlite-offline-v1","sourceAppID":source,"profileId":"ntfy-source","capacityBytes":capacity,"intervalSeconds":60,"idempotencyKey":key})
}
fn source_start(source: &str, revision: u64) -> Value {
    json!({"action":"apps.start","appID":source,"revision":revision,"subscriptionRevision":1,"idempotencyKey":"authority-fenced-start"})
}
fn source_update(source: &str, revision: u64, key: &str) -> Value {
    let mut update = fixture_deploy_command(source, FIXTURE_NTFY_IMAGE);
    update["action"] = json!("apps.update");
    update["idempotencyKey"] = json!(key);
    update["revision"] = json!(revision);
    update
}
fn database_code(error: &sea_orm::DbErr) -> Option<String> {
    match error {
        sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(error)) => error
            .as_database_error()
            .and_then(|e| e.code())
            .map(|s| s.into_owned()),
        _ => None,
    }
}
async fn binding_job_digest(c: &Context<'_>, claim: &wire::recovery::RecoveryClaim) -> Value {
    row(c.owner,"SELECT to_jsonb(encode(sha256(convert_to(to_jsonb(j)::text,'UTF8')),'hex')) AS value FROM webdock_auth.hosting_recovery_job j WHERE operation_id=$1",vec![claim.spec.operation_id.clone().into()]).await
}
async fn actual_binding_probes(c: &Context<'_>, claim: &wire::recovery::RecoveryClaim) {
    let agent = c
        .hosting
        .authenticate_agent(c.cluster, c.enrollment["credential"].as_str().unwrap())
        .await
        .unwrap();
    let identity = &c.enrollment["identity"];
    let installation = identity["installation"].as_str().unwrap();
    let agent_id = identity["agentID"].as_str().unwrap();
    let before = binding_job_digest(c, claim).await;
    let acl=row(c.owner,"SELECT jsonb_build_object('tableSelect',has_table_privilege('webdock_worker_runtime','webdock_auth.agent_transport_enrollment','SELECT'),'columnUpdate',has_any_column_privilege('webdock_worker_runtime','webdock_auth.agent_transport_enrollment','UPDATE'),'workerExecute',has_function_privilege('webdock_worker_runtime','webdock_rls.recovery_bind_transport(text,integer,text,uuid,integer,text,text)','EXECUTE'),'authExecute',has_function_privilege('webdock_auth_runtime','webdock_rls.recovery_bind_transport(text,integer,text,uuid,integer,text,text)','EXECUTE')) AS value",vec![]).await;
    assert_eq!(
        acl,
        json!({"tableSelect":false,"columnUpdate":false,"workerExecute":true,"authExecute":false})
    );
    for (old, new) in [
        (
            "recovery_admit_source_fence(text,integer,text,uuid,jsonb)",
            "recovery_admit_source_fence(text,integer,text,uuid,jsonb,text,text)",
        ),
        (
            "recovery_mark_source_drained(text,integer,text,uuid,jsonb)",
            "recovery_mark_source_drained(text,integer,text,uuid,jsonb,text)",
        ),
        (
            "recovery_source_resume(text,integer,text,uuid)",
            "recovery_source_resume(text,integer,text,uuid,text,text)",
        ),
        (
            "recovery_app_source_admit(text,integer,text,uuid,integer,jsonb)",
            "recovery_app_source_admit(text,integer,text,uuid,integer,jsonb,text,text)",
        ),
        (
            "recovery_app_source_drained(text,integer,text,uuid,integer,jsonb)",
            "recovery_app_source_drained(text,integer,text,uuid,integer,jsonb,text)",
        ),
        (
            "recovery_app_source_resume(text,integer,text,uuid,integer,jsonb)",
            "recovery_app_source_resume(text,integer,text,uuid,integer,jsonb,text,text)",
        ),
    ] {
        let result=row(c.owner,"SELECT jsonb_build_object('oldAbsent',to_regprocedure('webdock_rls.'||$1) IS NULL,'workerExecute',has_function_privilege('webdock_worker_runtime','webdock_rls.'||$2,'EXECUTE'),'authExecute',has_function_privilege('webdock_auth_runtime','webdock_rls.'||$2,'EXECUTE')) AS value",vec![old.into(),new.into()]).await;
        assert_eq!(
            result,
            json!({"oldAbsent":true,"workerExecute":true,"authExecute":false})
        );
    }
    assert_eq!(row(c.owner,"SELECT to_jsonb(column_default IS NULL AND is_nullable='NO') AS value FROM information_schema.columns WHERE table_schema='webdock_auth' AND table_name='hosting_recovery_source_event' AND column_name='id'",vec![]).await,true);
    let tx = c.hosting.begin_authenticated_agent(&agent).await.unwrap();
    let denied=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT credential_hash FROM webdock_auth.agent_transport_enrollment WHERE installation=$1 AND agent_id=$2 AND cluster_id=$3 AND generation=$4 AND NOT revoked FOR SHARE",
        vec![installation.into(),agent_id.into(),c.cluster.into(),agent.generation().into()])).await;
    assert!(
        denied.as_ref().err().and_then(database_code).as_deref() == Some("42501"),
        "raw worker transport read unexpectedly authorized"
    );
    tx.rollback().await.unwrap();
    c.hosting
        .bind_recovery_transport(&agent, claim, installation, agent_id)
        .await
        .unwrap();
    assert!(
        binding_job_digest(c, claim).await == before,
        "exact binding replay changed job custody"
    );
    let arguments: Vec<sea_orm::Value> = vec![
        claim.spec.operation_id.clone().into(),
        (claim.lease.generation as i32).into(),
        format!("{:x}", Sha256::digest(claim.lease.token.as_bytes())).into(),
        claim.spec.epoch.clone().into(),
        agent.generation().into(),
        installation.into(),
        agent_id.into(),
    ];
    for index in 0..7 {
        let mut changed = arguments.clone();
        match index {
            0 => changed[6] = "foreign-agent".into(),
            1 => changed[5] = "foreign-installation".into(),
            2 => changed[4] = (agent.generation() + 1).into(),
            3 => changed[1] = ((claim.lease.generation + 1) as i32).into(),
            4 => changed[2] = "0".repeat(64).into(),
            5 => changed[0] = "999999999999999999".into(),
            _ => changed[3] = uuid::Uuid::new_v4().to_string().into(),
        }
        let tx = c.hosting.begin_authenticated_agent(&agent).await.unwrap();
        let result = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.recovery_bind_transport($1,$2,$3,$4::uuid,$5,$6,$7) AS allowed",
                changed,
            ))
            .await;
        let rejected = match result {
            Ok(Some(row)) => row
                .try_get::<bool>("", "allowed")
                .is_ok_and(|allowed| !allowed),
            Err(error) => database_code(&error).as_deref() == Some("42501"),
            _ => false,
        };
        tx.rollback().await.unwrap();
        assert!(
            rejected,
            "binding mutation negative did not deny at expected authority boundary"
        );
        assert!(
            binding_job_digest(c, claim).await == before,
            "denied binding changed job"
        );
    }
    let result = c
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.recovery_bind_transport($1,$2,$3,$4::uuid,$5,$6,$7) AS allowed",
            arguments,
        ))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !result.try_get::<bool>("", "allowed").unwrap(),
        "unscoped owner context must not bind a worker job"
    );
    let mut expired = serde_json::to_value(claim).unwrap();
    expired["lease"]["expiresAt"] = json!("1970-01-01T00:00:00Z");
    let expired: wire::recovery::RecoveryClaim = serde_json::from_value(expired).unwrap();
    assert!(
        c.hosting
            .bind_recovery_transport(&agent, &expired, installation, agent_id)
            .await
            .is_err()
    );
    let mut foreign = claim.clone();
    foreign.spec.cluster_id = "999999999999999999".into();
    assert!(
        c.hosting
            .bind_recovery_transport(&agent, &foreign, installation, agent_id)
            .await
            .is_err()
    );
    assert!(
        binding_job_digest(c, claim).await == before,
        "context/claim denial changed job"
    );
    eprintln!(
        "recovery_component transport_binding_exact_replay=true raw_worker_read_42501=true acl_unchanged=true foreign_stale_token_context_denied=true expired_claim_denied=true job_unchanged=true"
    );
}
async fn source_history_counts(c: &Context<'_>, operation: &str) -> Value {
    row(c.owner,"SELECT jsonb_build_object('events',(SELECT count(*) FROM webdock_auth.hosting_recovery_source_event WHERE operation_id=$1),'audits',(SELECT count(*) FROM webdock_auth.hosting_audit WHERE target_id=$2 AND action IN('recovery.app-source','recovery.app-compensation'))) AS value",vec![operation.into(),c.source.into()]).await
}
async fn actual_native_history_probes(
    c: &Context<'_>,
    claim: &wire::recovery::RecoveryClaim,
    proof: &Value,
) {
    let agent = c
        .hosting
        .authenticate_agent(c.cluster, c.enrollment["credential"].as_str().unwrap())
        .await
        .unwrap();
    let before = source_history_counts(c, &claim.spec.operation_id).await;
    let cached = c
        .hosting
        .apply_recovery_checkpoint(&agent, claim, "checkpoint", proof.clone())
        .await
        .unwrap();
    let replay = c
        .hosting
        .apply_recovery_checkpoint(&agent, claim, "checkpoint", proof.clone())
        .await
        .unwrap();
    assert!(
        cached == replay,
        "exact checkpoint replay changed its retained receipt"
    );
    assert!(
        source_history_counts(c, &claim.spec.operation_id).await == before,
        "exact checkpoint replay grew canonical history"
    );
    let ids=row(c.owner,"SELECT coalesce(jsonb_agg(id ORDER BY id),'[]'::jsonb) AS value FROM (SELECT id FROM webdock_auth.hosting_recovery_source_event WHERE operation_id=$1 UNION ALL SELECT id FROM webdock_auth.hosting_audit WHERE target_id=$2 AND action IN('recovery.app-source','recovery.app-compensation')) history",vec![claim.spec.operation_id.clone().into(),c.source.into()]).await;
    let ids = ids.as_array().unwrap();
    assert!(ids.len() >= 3);
    let mut unique = std::collections::BTreeSet::new();
    for id in ids {
        let id = id.as_str().unwrap();
        assert!(webdock_core::id::RecordId::parse(id).is_ok());
        let parts = webdock_ids::Layout::PLATFORM
            .decode(id.parse().unwrap())
            .unwrap();
        assert!(
            webdock_ids::Identity::platform(parts.identity.machine(), parts.identity.generator())
                .is_ok(),
            "source history used a reserved legacy ID identity"
        );
        assert!(unique.insert(id), "history IDs must be globally distinct");
    }
    assert_eq!(
        row(
            c.owner,
            "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.snowflake_state",
            vec![]
        )
        .await,
        0,
        "native fixture must not seed the legacy AUTH allocator"
    );
    let event=row(c.owner,"SELECT to_jsonb(id) AS value FROM webdock_auth.hosting_recovery_source_event WHERE operation_id=$1 ORDER BY created_at,id LIMIT 1",vec![claim.spec.operation_id.clone().into()]).await;
    let audit=row(c.owner,"SELECT to_jsonb(id) AS value FROM webdock_auth.hosting_audit WHERE target_id=$1 AND action='recovery.app-source' ORDER BY created_at,id LIMIT 1",vec![c.source.into()]).await;
    let intent = json!({"phase":"app-resume-intent","runtimeUID":proof["runtimeUID"],"fenceFingerprint":proof["fenceFingerprint"]});
    for (event_id, audit_id) in [
        (event.as_str().unwrap(), claim.spec.operation_id.as_str()),
        (claim.spec.operation_id.as_str(), audit.as_str().unwrap()),
    ] {
        // Deliberately reuse actual server IDs only in this invalid, rolled-back SQL call.
        let tx = c.hosting.begin_authenticated_agent(&agent).await.unwrap();
        let result = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.recovery_app_source_resume($1,$2,$3,$4::uuid,$5,$6,$7,$8)",
                vec![
                    claim.spec.operation_id.clone().into(),
                    (claim.lease.generation as i32).into(),
                    format!("{:x}", Sha256::digest(claim.lease.token.as_bytes())).into(),
                    claim.spec.epoch.clone().into(),
                    agent.generation().into(),
                    intent.clone().into(),
                    event_id.into(),
                    audit_id.into(),
                ],
            ))
            .await;
        let collision = result.as_ref().err().and_then(database_code).as_deref() == Some("23505");
        tx.rollback().await.unwrap();
        assert!(
            collision,
            "duplicate history ID must abort the whole transition"
        );
        assert!(
            source_history_counts(c, &claim.spec.operation_id).await == before,
            "collision committed partial history"
        );
        assert_eq!(
            fence_state(c, &claim.spec.operation_id).await["state"],
            "stopped"
        );
    }
    eprintln!(
        "recovery_component source_history_native_ids=true legacy_auth_allocator_unseeded=true exact_checkpoint_replay_no_growth=true event_audit_collision_rollback=true old_writer_entrypoints_retired=true"
    );
}
#[test]
fn actual_authority_hosting_commands_match_product_contracts() {
    for command in [
        source_start("123", 1),
        source_update("123", 1, "authority-fenced-update"),
        source_update("123", 1, "authority-normal-unknown"),
    ] {
        webdock_hosting::contracts::parse_command(command).unwrap();
    }
}
fn checkpoint(
    identity: &wire::AgentIdentity,
    claim: &wire::recovery::RecoveryClaim,
    value: Value,
) -> wire::RecoveryCheckpointRequest {
    wire::RecoveryCheckpointRequest {
        identity: Some(identity.clone()),
        claim_json: serde_json::to_vec(claim).unwrap(),
        phase: "checkpoint".into(),
        checkpoint_json: serde_json::to_vec(&value).unwrap(),
    }
}
fn guard(
    identity: &wire::AgentIdentity,
    claim: &wire::recovery::RecoveryClaim,
    phase: &str,
    fingerprint: &str,
) -> wire::RecoveryGuardRequest {
    let mut guarded = claim.clone();
    guarded.checkpoint = Some(json!({"appGuard":{"phase":phase,"fenceFingerprint":fingerprint}}));
    wire::RecoveryGuardRequest {
        identity: Some(identity.clone()),
        claim_json: serde_json::to_vec(&guarded).unwrap(),
    }
}
fn rejected<T>(reply: Result<Response<T>, Status>, stage: &str) {
    let error = reply
        .err()
        .unwrap_or_else(|| panic!("expected real authority denial at {stage}"));
    let expected = if matches!(stage, "nullable-provider-outcome" | "stale-capture-purpose") {
        tonic::Code::FailedPrecondition
    } else {
        tonic::Code::Unavailable
    };
    assert_eq!(
        error.code(),
        expected,
        "unexpected static authority status at {stage}"
    );
}
fn fenced(reply: Reply) {
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(
        reply.body["error"]["message"],
        "Application recovery source is fenced. Reconciliation is required."
    );
}
async fn fence_state(c: &Context<'_>, operation: &str) -> Value {
    row(c.owner,"SELECT jsonb_build_object('state',f.state,'events',(SELECT count(*) FROM webdock_auth.hosting_recovery_source_event e WHERE e.operation_id=f.operation_id),'callbacks',(SELECT count(*) FROM webdock_auth.hosting_recovery_callback cb WHERE cb.operation_id=f.operation_id)) AS value FROM webdock_auth.hosting_recovery_source_fence f WHERE f.operation_id=$1",vec![operation.into()]).await
}
pub(super) fn private_input(path: &Path, max: u64) -> Vec<u8> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    let file = File::from(fd);
    let m = file.metadata().unwrap();
    assert!(
        m.is_file()
            && m.uid() == 0
            && m.mode() & 0o777 == 0o600
            && m.nlink() == 1
            && m.len() <= max
    );
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() as u64 <= max);
    bytes
}
fn child_is_running(
    child: &mut tokio::process::Child,
    path: &Path,
    log: &str,
    stage: &'static str,
) -> bool {
    let status = diagnostic_child(child.try_wait());
    if status["state"] == "running" {
        return true;
    }
    let directory = diagnostic_directory(path);
    let entry = diagnostic_entry(
        directory.as_ref().map_err(|e| io::Error::from(e.kind())),
        log,
        true,
    );
    eprintln!(
        "recovery_component child_exit stage={stage} log={log} status={status} categories={}",
        entry["categories"]
    );
    false
}
#[derive(Clone, Copy)]
pub(super) enum RecoveryHeartbeat<'a> {
    App { trusted: bool },
    Mail(&'a Value),
}
impl RecoveryHeartbeat<'_> {
    fn matches(self, native: &Value) -> bool {
        match self {
            Self::App { .. } => native["appEffectFenceVersion"] == 1,
            Self::Mail(expected) => {
                expected["id"] == "mail-source"
                    && expected["profile"] == "mail-installation-v1"
                    && native["version"] == 2
                    && native["profiles"] == json!([expected])
                    && native["targets"] == json!([])
            }
        }
    }
}
#[test]
fn mail_heartbeat_binds_its_real_profile_without_claiming_an_app_fence() {
    let expected = json!({"id":"mail-source","profile":"mail-installation-v1","sha256":"a".repeat(64),"keyCustodyID":"operator-unit","productionQualified":false,"storageIdentity":{"poolID":"owned-pool","revision":"b".repeat(64)}});
    let native = json!({"version":2,"profiles":[expected.clone()],"targets":[]});
    assert!(!RecoveryHeartbeat::App { trusted: true }.matches(&native));
    assert!(RecoveryHeartbeat::Mail(&expected).matches(&native));
    for field in ["sha256", "keyCustodyID", "storageIdentity", "profile"] {
        let mut wrong = native.clone();
        wrong["profiles"][0][field] = json!("foreign");
        assert!(!RecoveryHeartbeat::Mail(&expected).matches(&wrong));
    }
}

pub(super) async fn observation_after(
    owner: &impl ConnectionTrait,
    cluster: &str,
    sequence: i64,
    agent: &mut tokio::process::Child,
    helper: &mut tokio::process::Child,
    path: &Path,
    expectation: RecoveryHeartbeat<'_>,
) -> Value {
    let trusted = !matches!(expectation, RecoveryHeartbeat::App { trusted: false });
    let mut last = Value::Null;
    let result=tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            assert!(
                child_is_running(
                    agent,
                    path,
                    if trusted {
                        "agent-profile-trusted.stderr"
                    } else {
                        "agent-profile.stderr"
                    },
                    if trusted {
                        "trusted_profile_heartbeat"
                    } else {
                        "first_profile_heartbeat"
                    }
                ),
                "owned Agent exited before configured heartbeat"
            );
            assert!(
                child_is_running(
                    helper,
                    path,
                    if trusted {
                        "storage-trusted.stderr"
                    } else {
                        "storage.stderr"
                    },
                    if trusted {
                        "trusted_profile_heartbeat"
                    } else {
                        "first_profile_heartbeat"
                    }
                ),
                "owned helper exited before configured heartbeat"
            );
            let a = row(
                owner,
                "SELECT to_jsonb(a) AS value FROM webdock_auth.hosting_agent a WHERE cluster_id=$1",
                vec![cluster.into()],
            )
            .await;
            let native=&a["observation"]["capabilities"]["nativeRecovery"];
            last=json!({"sequence":a["last_sequence"],"nativeRecoveryVersion":native["version"],"profileCount":native["profiles"].as_array().map(Vec::len),"expectedProfileMatched":expectation.matches(native),"appEffectFenceVersion":native["appEffectFenceVersion"].as_u64()});
            if a["last_sequence"].as_i64().is_some_and(|s| s > sequence)
                && expectation.matches(native)
            {
                break a;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    if matches!(expectation, RecoveryHeartbeat::Mail(_)) {
        let _ = write_diagnostic_json(
            Path::new("/diagnostics/mail-recovery-agent-heartbeat.json"),
            &last,
        );
    }
    result.expect("real configured daemon recovery heartbeat")
}
pub(super) fn bind_trusted_socket(
    socket_directory: &Path,
    agent: &mut Value,
    recovery: &mut Value,
) -> std::path::PathBuf {
    let socket = socket_directory.join("storage.sock");
    agent["runtime"]["storageSocket"] = json!(socket);
    recovery["storageSocket"] = json!(socket);
    socket
}
fn socket_directory_receipt(path: &Path) -> Value {
    let mut safe = std::fs::canonicalize(path).is_ok_and(|canonical| canonical == path);
    let mut rows = Vec::new();
    for (index, ancestor) in path.ancestors().enumerate() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(m) => {
                let mode = m.mode() & 0o777;
                safe &= m.is_dir()
                    && !m.file_type().is_symlink()
                    && m.uid() == 0
                    && if index == 0 {
                        mode == 0o700 || m.gid() == 65532 && mode == 0o710
                    } else {
                        mode & 0o022 == 0
                    };
                rows.push(json!({"path":ancestor,"uid":m.uid(),"gid":m.gid(),"mode":format!("{mode:04o}"),"type":if m.file_type().is_symlink(){"symlink"}else if m.is_dir(){"directory"}else{"other"}}));
            }
            Err(error) => {
                safe = false;
                rows.push(json!({"path":ancestor,"status":"unavailable","errorKind":format!("{:?}",error.kind())}));
            }
        }
    }
    json!({"scope":"owned_fixture_socket_ancestor_metadata_only","safe":safe,"ancestors":rows})
}
pub(super) fn record_socket_directory(path: &Path, phase: &'static str) {
    let receipt = socket_directory_receipt(path);
    let target = Path::new("/diagnostics").join(format!(
        "trusted-socket-{phase}-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    write_diagnostic_json(&target, &receipt).expect("private socket ancestor metadata retention");
    assert_eq!(
        receipt["safe"], true,
        "owned socket directory violates helper ancestor custody"
    );
}
#[test]
fn writable_tmp_cannot_be_a_trusted_socket_parent() {
    let receipt = socket_directory_receipt(Path::new("/tmp"));
    assert_eq!(receipt["safe"], false);
    assert_eq!(receipt["ancestors"][0]["path"], "/tmp");
    assert!(receipt.get("body").is_none());
}
#[test]
fn trusted_helper_socket_binding_updates_both_consumers_without_changing_other_inputs() {
    let root = Path::new("/owned/recovery-peer-unit");
    let mut agent = json!({"runtime":{"kind":"docker","storageSocket":root.join("storage.sock")},"unchanged":17});
    let mut recovery =
        json!({"storageSocket":root.join("storage.sock"),"profiles":[{"id":"fixture"}]});
    let socket = bind_trusted_socket(root, &mut agent, &mut recovery);
    assert_eq!(socket.parent(), Some(root));
    assert_ne!(socket.parent(), Some(Path::new("/tmp")));
    assert_eq!(agent["runtime"]["storageSocket"], json!(socket));
    assert_eq!(recovery["storageSocket"], json!(socket));
    assert_eq!(agent["runtime"]["kind"], "docker");
    assert_eq!(agent["unchanged"], 17);
    assert_eq!(recovery["profiles"], json!([{"id":"fixture"}]));
}
#[tokio::test]
async fn actual_owned_child_exit_is_detected_before_readiness_polling() {
    let path = tempfile::tempdir().unwrap();
    std::fs::set_permissions(path.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let log = path.path().join("storage-trusted.stderr");
    std::fs::write(&log, b"Runtime resource ownership mismatch\n").unwrap();
    std::fs::set_permissions(log, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut child = tokio::process::Command::new("/bin/sh")
        .args(["-c", "exit 17"])
        .spawn()
        .unwrap();
    assert_eq!(child.wait().await.unwrap().code(), Some(17));
    assert!(!child_is_running(
        &mut child,
        path.path(),
        "storage-trusted.stderr",
        "trusted_helper_readiness"
    ));
    assert_eq!(diagnostic_child(child.try_wait())["code"], 17);
}
pub(super) async fn wait_trusted_helper(
    helper: &mut tokio::process::Child,
    path: &Path,
    socket: &Path,
    expected: &webdock_agent::storage::recovery_pool::RecoveryPoolIdentity,
) {
    let client = webdock_agent::storage::StorageClient::new(socket.to_path_buf());
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            assert!(
                child_is_running(
                    helper,
                    path,
                    "storage-trusted.stderr",
                    "trusted_helper_readiness"
                ),
                "owned trusted helper exited before readiness"
            );
            let probe = tokio::time::timeout(Duration::from_millis(500), async {
                match client.information().await {
                    Ok(config) => client
                        .recovery_pool_identity()
                        .await
                        .map(|identity| (config, identity)),
                    Err(error) => Err(error),
                }
            })
            .await;
            if let Ok(Ok((config, identity))) = probe {
                assert!(
                    config == expected.configuration && identity == *expected,
                    "trusted helper changed original pool identity/configuration"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("real trusted helper typed readiness");
    eprintln!(
        "recovery_component trusted_helper_typed_readiness=true original_pool_identity_unchanged=true"
    );
}
pub(super) fn cli_validate(path: &Path) {
    let result = Command::new("/usr/local/bin/webdock-agent")
        .arg("--config")
        .arg(path)
        .arg("validate")
        .output()
        .unwrap();
    eprintln!(
        "recovery_component agent_cli_validate exitCode={} stdoutBytes={} stderrBytes={} stderrSHA256={:x}",
        result.status.code().unwrap_or(-1),
        result.stdout.len(),
        result.stderr.len(),
        Sha256::digest(&result.stderr)
    );
    assert!(
        result.status.success(),
        "real Agent CLI validation rejected private operator recovery input"
    );
}
fn owned_pid_status(pid: usize) -> Value {
    if pid == 0 {
        return json!({"state":"stopped_owned_child"});
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            json!({"state":match stat.rsplit_once(')').and_then(|(_,s)|s.trim().chars().next()) {Some('Z')=>"zombie",Some(_)=>"present",None=>"unreadable_status"}})
        }
        Err(error) => {
            json!({"state":"absent_or_unavailable","errorKind":format!("{:?}",error.kind())})
        }
    }
}
fn profile(c: &Context<'_>, runtime: Value, operator: &Value) -> Value {
    let custody = c.path.join("operator-custody");
    json!({"version":1,"installation":"owned-recovery","dockerSocket":"/run/owned-engine.sock","storageSocket":c.storage_socket,
        "stagingDirectory":c.path.join("recovery-staging"),"custodyDirectory":custody,"replayDirectory":c.path.join("recovery-replay"),
        "coreSigners":c.path.join("core-signers"),"operatorSigners":c.path.join("operator-signers"),"targets":[],
        "profiles":[{"id":"ntfy-source","profile":"ntfy-sqlite-offline-v1","keyCustodyID":operator["keyCustodyID"],"destination":operator["destination"],
            "writeCredentials":"/owned/operator-recovery/writer.json","restoreCredentials":"/owned/operator-recovery/reader.json","ageRecipient":operator["ageRecipient"],"ageIdentity":"/owned/operator-recovery/identity.agekey","independentFiles":{},
            "source":{"kind":"ntfy","runtime":runtime,"runtimeID":c.source_row["observed_uid"]}}]})
}
pub(super) struct InstalledProfile {
    pub runtime: Value,
    pub config: Value,
    pub actor: String,
}
pub(super) async fn install_profile(c: &mut Context<'_>) -> InstalledProfile {
    // Only actual product dispatch/readback supplies the source identity and RuntimeSpec.
    let runtime=row(c.owner,"SELECT command->'spec' AS value FROM webdock_auth.agent_runtime_dispatch WHERE operation_id=$1 AND receipt_sha256 IS NOT NULL ORDER BY generation DESC LIMIT 1",vec![c.source_row["operation_id"].as_str().unwrap().into()]).await;
    assert_eq!(runtime["resourceId"], c.source);
    assert_eq!(runtime["projectId"], c.source_row["project_id"]);
    let operator: Value = serde_json::from_slice(&private_input(
        Path::new("/owned/operator-recovery/inputs.json"),
        16384,
    ))
    .unwrap();
    assert_eq!(operator["version"], 1);
    for name in [
        "recovery-staging",
        "operator-custody",
        "recovery-replay",
        "recovery-verification",
    ] {
        directory(&c.path.join(name));
    }
    let public = std::fs::read_to_string(c.path.join("recovery-signing.pub")).unwrap();
    let operator_public = String::from_utf8(private_input(
        Path::new("/owned/operator-recovery/operator.pub"),
        16384,
    ))
    .unwrap();
    let actor=row(c.owner,"SELECT to_jsonb(u.id) AS value FROM webdock_auth.\"user\" u WHERE email='operator@example.test' AND role='operator'",vec![]).await;
    for (name, text) in [
        (
            "core-signers",
            format!("webdock-core-recovery-reservation {}\n", public.trim()),
        ),
        (
            "operator-signers",
            format!("{} {}\n", actor.as_str().unwrap(), operator_public.trim()),
        ),
    ] {
        std::fs::write(c.path.join(name), text).unwrap();
        std::fs::set_permissions(c.path.join(name), std::fs::Permissions::from_mode(0o600))
            .unwrap();
    }
    stop(c.agent).await;
    c.state.agent_pid.store(0, Ordering::SeqCst);
    let recovery = c.path.join("recovery.json");
    let mut recovery_config = profile(c, runtime.clone(), &operator);
    private(&recovery, &recovery_config);
    c.agent_config["recoveryRuntime"] = json!(recovery);
    private(c.agent_config_path, c.agent_config);
    cli_validate(c.agent_config_path);
    let before=row(c.owner,"SELECT to_jsonb(last_sequence) AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![c.cluster.into()]).await.as_i64().unwrap();
    *c.agent = spawn(
        "/usr/local/bin/webdock-agent",
        &[
            "--config".as_ref(),
            c.agent_config_path.as_os_str(),
            "run".as_ref(),
        ],
        c.path,
        "agent-profile",
    );
    c.state
        .agent_pid
        .store(c.agent.id().unwrap() as usize, Ordering::SeqCst);
    let observed = observation_after(
        c.owner,
        c.cluster,
        before,
        c.agent,
        c.helper,
        c.path,
        RecoveryHeartbeat::App { trusted: false },
    )
    .await;
    let native = &observed["observation"]["capabilities"]["nativeRecovery"];
    assert_eq!(native["version"], 2);
    assert_eq!(native["profiles"][0]["id"], "ntfy-source");
    assert_eq!(
        native["profiles"][0]["keyCustodyID"],
        operator["keyCustodyID"]
    );
    eprintln!("recovery_component first_profile_cli_heartbeat=true");
    let expected_pool: webdock_agent::storage::recovery_pool::RecoveryPoolIdentity =
        serde_json::from_value(native["profiles"][0]["storageIdentity"].clone()).unwrap();
    // Install the existing dedicated helper trust from actual advertised profile/pool custody.
    stop(c.agent).await;
    c.state.agent_pid.store(0, Ordering::SeqCst);
    stop(c.helper).await;
    c.state.helper_pid.store(0, Ordering::SeqCst);
    let staging = c.path.join("trusted-agent-staging");
    directory(&staging);
    std::os::unix::fs::chown(&staging, Some(65532), Some(65532)).unwrap();
    let trust = c.path.join("recovery-trust.json");
    let peer = Path::new("/owned").join(format!("recovery-peer-{}", uuid::Uuid::new_v4().simple()));
    directory(&peer);
    record_socket_directory(&peer, "before");
    private(
        &trust,
        &json!({"version":1,"installationID":"owned-recovery","coreSigners":c.path.join("core-signers"),"operatorSigners":c.path.join("operator-signers"),"postgresTlsCertificate":null,"postgresTlsKey":null,
        "verificationDirectory":c.path.join("recovery-verification"),"agentStagingDirectory":staging,"socketDirectory":peer,
        "targets":[{"id":"ntfy-source","sha256":native["profiles"][0]["sha256"],"nonce":null,"action":"backup","profile":"ntfy-sqlite-offline-v1","capacityBytes":67108864,"volumes":{},"applicationTarget":null}]}),
    );
    let trusted_socket = bind_trusted_socket(&peer, c.agent_config, &mut recovery_config);
    assert_eq!(trusted_socket.parent(), Some(peer.as_path()));
    private(&recovery, &recovery_config);
    private(c.agent_config_path, c.agent_config);
    *c.helper = spawn(
        "/usr/local/bin/webdock-storage",
        &[
            "--config".as_ref(),
            c.storage_config.as_os_str(),
            "--recovery-trust".as_ref(),
            trust.as_os_str(),
            "serve".as_ref(),
            "--socket".as_ref(),
            trusted_socket.as_os_str(),
        ],
        c.path,
        "storage-trusted",
    );
    c.state
        .helper_pid
        .store(c.helper.id().unwrap() as usize, Ordering::SeqCst);
    wait_trusted_helper(c.helper, c.path, &trusted_socket, &expected_pool).await;
    record_socket_directory(&peer, "after");
    cli_validate(c.agent_config_path);
    *c.agent = spawn(
        "/usr/local/bin/webdock-agent",
        &[
            "--config".as_ref(),
            c.agent_config_path.as_os_str(),
            "run".as_ref(),
        ],
        c.path,
        "agent-profile-trusted",
    );
    c.state
        .agent_pid
        .store(c.agent.id().unwrap() as usize, Ordering::SeqCst);
    observation_after(
        c.owner,
        c.cluster,
        observed["last_sequence"].as_i64().unwrap(),
        c.agent,
        c.helper,
        c.path,
        RecoveryHeartbeat::App { trusted: true },
    )
    .await;
    eprintln!(
        "recovery_component authority_profile_cli_heartbeat=true provider_network_qualified=false"
    );

    InstalledProfile {
        runtime,
        config: recovery_config,
        actor: actor.as_str().unwrap().to_owned(),
    }
}
pub(super) async fn check(mut c: Context<'_>) {
    assert!(enabled());
    c.state
        .agent_pid
        .store(c.agent.id().unwrap() as usize, Ordering::SeqCst);
    c.state
        .helper_pid
        .store(c.helper.id().unwrap() as usize, Ordering::SeqCst);
    let watched = c.state.clone();
    let mut diagnostic = HeartbeatCapture {
        source: c.path.to_path_buf(),
        output: Path::new("/diagnostics").join(format!(
            "native-authority-{}",
            uuid::Uuid::new_v4().simple()
        )),
        observe: move || json!({"agent":owned_pid_status(watched.agent_pid.load(Ordering::SeqCst)),"helper":owned_pid_status(watched.helper_pid.load(Ordering::SeqCst)),"controlResult":"not_sampled","heartbeatConfirmed":true,"phase":"app_authority"}),
        armed: true,
    };

    install_profile(&mut c).await;

    c.state.bind_probe_requested.store(true, Ordering::SeqCst);
    let grant = begin_policy(c.cluster, c.source, "authority-policy", 67108864);
    let (a, b) = tokio::join!(
        post(c.app, c.console, grant.clone()),
        post(c.app, c.console, grant.clone())
    );
    let a = okay(a, "authority-policy-grant");
    assert_eq!(a, okay(b, "authority-policy-replay"));
    let policy = a["policyID"].as_str().unwrap();
    let backup = json!({"action":"recovery.backup","policyID":policy,"revision":1,"idempotencyKey":"authority-backup"});
    let accepted = okay(
        post(c.app, c.console, backup.clone()).await,
        "authority-backup-admission",
    );
    assert_eq!(
        accepted,
        okay(
            post(c.app, c.console, backup).await,
            "authority-backup-replay"
        )
    );
    c.until(|state| state.bound_claim.lock().unwrap().is_some())
        .await;
    let bound_claim = c.state.bound_claim.lock().unwrap().clone().unwrap();
    assert!(
        bound_claim.spec.operation_id == accepted["operationID"],
        "actual binding must belong to the admitted operation"
    );
    actual_binding_probes(&c, &bound_claim).await;
    c.state.bind_probe_done.store(true, Ordering::SeqCst);
    c.state.signal();
    c.until(|state| state.stopped.load(Ordering::SeqCst)).await;
    let stopped = c.state.stopped_request.lock().unwrap().clone().unwrap();
    let claim: wire::recovery::RecoveryClaim = serde_json::from_slice(&stopped.claim_json).unwrap();
    let proof: Value = serde_json::from_slice(&stopped.checkpoint_json).unwrap();
    let fp = proof["fenceFingerprint"].as_str().unwrap();
    let identity = stopped.identity.as_ref().unwrap();
    assert_eq!(claim.spec.operation_id, accepted["operationID"]);
    assert_eq!(claim.spec.source["appID"], c.source);
    let channel = webdock_plugin_runtime::unix_channel(c.control_path)
        .await
        .unwrap();
    let mut client = wire::recovery_control_client::RecoveryControlClient::new(channel);
    // Genuine canonical stopped state, unchanged after held-lock and wrong-purpose denials.
    let before = fence_state(&c, &claim.spec.operation_id).await;
    assert_eq!(before["state"], "stopped");
    actual_native_history_probes(&c, &claim, &proof).await;
    let normal_agent = c
        .hosting
        .authenticate_agent(c.cluster, c.enrollment["credential"].as_str().unwrap())
        .await
        .unwrap();
    let normal_guard = c
        .hosting
        .begin_authenticated_agent(&normal_agent)
        .await
        .unwrap();
    let allowed = normal_guard
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.recovery_app_effect_allowed($1) AS allowed",
            [c.source.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<bool>("", "allowed")
        .unwrap();
    assert!(
        !allowed,
        "an actual admitted source fence must deny normal effects"
    );
    normal_guard.rollback().await.unwrap();
    use sea_orm::TransactionTrait;
    let epoch_lock = c.owner.begin().await.unwrap();
    epoch_lock
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT epoch FROM webdock_auth.native_recovery_epoch WHERE singleton FOR UPDATE",
        ))
        .await
        .unwrap();
    rejected(
        client
            .guard_recovery(guard(identity, &claim, "capture", fp))
            .await,
        "epoch-nowait",
    );
    epoch_lock.rollback().await.unwrap();
    let mut wrong = claim.clone();
    wrong.spec.epoch = uuid::Uuid::new_v4().to_string();
    rejected(
        client
            .guard_recovery(guard(identity, &wrong, "capture", fp))
            .await,
        "wrong-epoch",
    );
    assert_eq!(fence_state(&c, &claim.spec.operation_id).await, before);
    fenced(
        post(
            c.app,
            c.console,
            source_start(c.source, c.source_row["revision"].as_u64().unwrap()),
        )
        .await,
    );
    let update = source_update(
        c.source,
        c.source_row["revision"].as_u64().unwrap(),
        "authority-fenced-update",
    );
    fenced(post(c.app, c.console, update).await);
    client.checkpoint_recovery(checkpoint(identity,&claim,json!({"phase":"app-resume-intent","runtimeUID":proof["runtimeUID"],"fenceFingerprint":fp}))).await.unwrap();
    let before = fence_state(&c, &claim.spec.operation_id).await;
    assert_eq!(before["state"], "resuming");
    for outcome in [None, Some(Value::Null), Some(json!("unknown"))] {
        let mut bad =
            json!({"phase":"app-resumed","runtimeUID":proof["runtimeUID"],"fenceFingerprint":fp});
        if let Some(outcome) = outcome {
            bad["providerOutcome"] = outcome;
        }
        rejected(
            client
                .checkpoint_recovery(checkpoint(identity, &claim, bad))
                .await,
            "nullable-provider-outcome",
        );
        assert_eq!(fence_state(&c, &claim.spec.operation_id).await, before);
    }
    rejected(
        client
            .guard_recovery(guard(identity, &claim, "capture", fp))
            .await,
        "stale-capture-purpose",
    );
    c.state.stopped_ack_allowed.store(true, Ordering::SeqCst);
    c.state.signal();
    c.until(|state| state.release_lost.load(Ordering::SeqCst))
        .await;
    assert!(c.state.capture_denied.load(Ordering::SeqCst) > 0);
    let local = c
        .path
        .join("agent-state/app-effects")
        .join(format!("{}.json", c.source));
    let outbox = c.path.join("agent-state/recovery-outbox.json");
    let closed_bytes = private_input(&local, 1024 * 1024);
    let local_state: Value = serde_json::from_slice(&closed_bytes).unwrap();
    let journal_meta = std::fs::symlink_metadata(&local).unwrap();
    assert!(!local_state["fence"].is_null());
    assert_eq!(local_state["resume_completed"], true);
    let before_outbox = private_input(&outbox, 1024 * 1024);
    assert!(!serde_json::from_slice::<Value>(&before_outbox).unwrap()["pending"].is_null());
    stop(c.agent).await;
    c.state.agent_pid.store(0, Ordering::SeqCst);
    // Negative local-journal loss: move only this owned file while its daemon is stopped.
    let saved = local.with_extension("missing-test-original");
    assert!(!saved.exists());
    std::fs::rename(&local, &saved).unwrap();
    let seq=row(c.owner,"SELECT to_jsonb(last_sequence) AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![c.cluster.into()]).await.as_i64().unwrap();
    c.state.release_replay_allowed.store(true, Ordering::SeqCst);
    cli_validate(c.agent_config_path);
    *c.agent = spawn(
        "/usr/local/bin/webdock-agent",
        &[
            "--config".as_ref(),
            c.agent_config_path.as_os_str(),
            "run".as_ref(),
        ],
        c.path,
        "agent-missing-journal",
    );
    c.state
        .agent_pid
        .store(c.agent.id().unwrap() as usize, Ordering::SeqCst);
    observation_after(
        c.owner,
        c.cluster,
        seq,
        c.agent,
        c.helper,
        c.path,
        RecoveryHeartbeat::App { trusted: true },
    )
    .await;
    c.until(|state| state.release_replays.load(Ordering::SeqCst) >= 2)
        .await;
    assert!(!local.exists());
    assert_eq!(private_input(&outbox, 1024 * 1024), before_outbox);
    stop(c.agent).await;
    c.state.agent_pid.store(0, Ordering::SeqCst);
    // Restore exactly the retained negative-fixture file, never fabricate/repair its contents.
    assert_eq!(private_input(&saved, 1024 * 1024), closed_bytes);
    std::fs::rename(&saved, &local).unwrap();
    let restored_meta = std::fs::symlink_metadata(&local).unwrap();
    assert_eq!(
        (
            journal_meta.uid(),
            journal_meta.gid(),
            journal_meta.mode(),
            journal_meta.dev(),
            journal_meta.ino()
        ),
        (
            restored_meta.uid(),
            restored_meta.gid(),
            restored_meta.mode(),
            restored_meta.dev(),
            restored_meta.ino()
        )
    );
    write_diagnostic_json(&Path::new("/diagnostics").join(format!("authority-journal-negative-{}.json",uuid::Uuid::new_v4().simple())),&json!({"scope":"exact_owned_negative_only_no_repair","path":local,"uid":journal_meta.uid(),"gid":journal_meta.gid(),"mode":format!("{:04o}",journal_meta.mode()&0o7777),"bytes":closed_bytes.len(),"sha256":format!("{:x}",Sha256::digest(&closed_bytes)),"sameFilesystemRename":true,"daemonStoppedBeforeRenameAndRestore":true,"outboxUnchanged":true})).unwrap();
    let epoch_lock = c.owner.begin().await.unwrap();
    epoch_lock
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT epoch FROM webdock_auth.native_recovery_epoch WHERE singleton FOR UPDATE",
        ))
        .await
        .unwrap();
    let seq=row(&epoch_lock,"SELECT to_jsonb(last_sequence) AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![c.cluster.into()]).await.as_i64().unwrap();
    c.state.release_replay_allowed.store(true, Ordering::SeqCst);
    cli_validate(c.agent_config_path);
    *c.agent = spawn(
        "/usr/local/bin/webdock-agent",
        &[
            "--config".as_ref(),
            c.agent_config_path.as_os_str(),
            "run".as_ref(),
        ],
        c.path,
        "agent-restarted",
    );
    c.state
        .agent_pid
        .store(c.agent.id().unwrap() as usize, Ordering::SeqCst);
    observation_after(
        &epoch_lock,
        c.cluster,
        seq,
        c.agent,
        c.helper,
        c.path,
        RecoveryHeartbeat::App { trusted: true },
    )
    .await;
    let denied_before = c.state.release_denied.load(Ordering::SeqCst);
    c.until(|state| state.release_denied.load(Ordering::SeqCst) > denied_before)
        .await;
    assert!(
        !serde_json::from_slice::<Value>(&private_input(&local, 1024 * 1024)).unwrap()["fence"]
            .is_null()
    );
    assert_eq!(private_input(&outbox, 1024 * 1024), before_outbox);
    epoch_lock.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let local: Value = serde_json::from_slice(&private_input(&local, 1024 * 1024)).unwrap();
            let pending: Value =
                serde_json::from_slice(&private_input(&outbox, 1024 * 1024)).unwrap();
            if local["fence"].is_null() && pending["pending"].is_null() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("actual reconnect releases local gate only after exact ACK + fresh release guard");
    let closed = fence_state(&c, &claim.spec.operation_id).await;
    assert_eq!(closed["state"], "released");
    assert_eq!(row(c.owner,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.hosting_recovery_set WHERE operation_id=$1",vec![claim.spec.operation_id.clone().into()]).await,0);
    eprintln!(
        "recovery_component app_fence_sql_nullable_epoch_purpose_lostack_restart=true snapshot_or_set_claim=false"
    );
    okay(post(c.app,c.console,json!({"action":"recovery.policies.revoke","policyID":policy,"revision":1,"idempotencyKey":"authority-revoke"})).await,"authority-policy-revoke");
    rejected(
        client
            .guard_recovery(guard(identity, &claim, "release", fp))
            .await,
        "revoked-release-guard",
    );
    let policy = okay(
        post(
            c.app,
            c.console,
            begin_policy(c.cluster, c.source, "authority-unknown-policy", 67108864),
        )
        .await,
        "policy-before-normal-unknown",
    );
    c.state.hold_normal.store(true, Ordering::SeqCst);
    let update = source_update(
        c.source,
        c.source_row["revision"].as_u64().unwrap(),
        "authority-normal-unknown",
    );
    let normal = okay(
        post(c.app, c.console, update).await,
        "normal-update-before-fence",
    );
    c.until(|state| state.normal_received.load(Ordering::SeqCst))
        .await;
    let unfinished=row(c.owner,"SELECT jsonb_build_object('status',o.status,'appStatus',a.status) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_app a ON a.id=o.app_id WHERE o.id=$1",vec![normal["id"].as_str().unwrap().into()]).await;
    assert_eq!(unfinished["status"], "running");
    assert_eq!(unfinished["appStatus"], "pending");
    let reservations = row(
        c.owner,
        "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.hosting_recovery_reservation",
        vec![],
    )
    .await;
    let denied=post(c.app,c.console,json!({"action":"recovery.backup","policyID":policy["policyID"],"revision":1,"idempotencyKey":"unknown-normal-backup-denied"})).await;
    assert!(expected_draft_policy_denial(denied.status, &denied.body));
    assert_eq!(
        row(
            c.owner,
            "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.hosting_recovery_reservation",
            vec![]
        )
        .await,
        reservations
    );
    c.state.normal_ack_allowed.store(true, Ordering::SeqCst);
    c.state.signal();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let status = row(
                c.owner,
                "SELECT to_jsonb(status) AS value FROM webdock_auth.hosting_operation WHERE id=$1",
                vec![normal["id"].as_str().unwrap().into()],
            )
            .await;
            if status == "succeeded" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("actual normal workload receipt completes after controlled precommit loss");
    assert_eq!(
        row(
            c.owner,
            "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.hosting_recovery_set",
            vec![]
        )
        .await,
        0
    );
    for stage in std::fs::read_dir(c.path.join("trusted-agent-staging")).unwrap() {
        let stage = stage.unwrap();
        assert!(!stage.file_type().unwrap().is_symlink());
        assert!(!stage.path().join("data/application").exists());
        assert!(!stage.path().join("data/backup-work").exists());
    }
    eprintln!(
        "recovery_component canonical_unfinished_normal_operation_admission_denied=true no_capture_directories=true no_completed_sets=true"
    );
    diagnostic.disarm();
}
