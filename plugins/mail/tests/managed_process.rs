//! Actual executable/TLS/private-PG process proof with an explicitly controlled Host/MailControl
//! stand-in. This is NOT real Core authority acceptance (root Task4 owns that proof).
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::Signer;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use tonic::{Request, Response, Status};
use tower::ServiceExt;
use webdock_mail_protocol::{
    LeaseClaims,
    native_control::{Control, ControlError, ControlServer},
    outbox::SendLease,
    wire as mail,
};
use webdock_plugin_runtime::{PeerIdentity, PinnedMutualTls, wire};

struct Row {
    job: Option<SendLease>,
    state: String,
    attempted: bool,
}
struct State {
    row: Mutex<Row>,
    active: AtomicBool,
    jobs: AtomicUsize,
    pause_attempt: AtomicBool,
    attempt_started: AtomicBool,
    claims: AtomicUsize,
    begins: AtomicUsize,
    auths: AtomicUsize,
    drop_final: AtomicBool,
    data: AtomicUsize,
    action_domain: String,
    descriptor: String,
    credential: String,
    issuer: ed25519_dalek::SigningKey,
}
#[derive(Clone)]
struct Host(Arc<State>);
impl std::ops::Deref for Host {
    type Target = State;
    fn deref(&self) -> &State {
        &self.0
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
#[tonic::async_trait]
impl wire::plugin_host_server::PluginHost for Host {
    async fn authorize_peer(
        &self,
        _: Request<wire::HostPeerRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Ok(Response::new(wire::HostAuthorityResponse {
            active: self.active.load(Ordering::SeqCst),
        }))
    }
    async fn authorize_invocation(
        &self,
        _: Request<wire::HostInvocationRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Ok(Response::new(wire::HostAuthorityResponse {
            active: self.active.load(Ordering::SeqCst),
        }))
    }
    async fn authorize_request(
        &self,
        _: Request<wire::HostPayloadRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Ok(Response::new(wire::HostAuthorityResponse {
            active: self.active.load(Ordering::SeqCst),
        }))
    }
    async fn call(
        &self,
        _: Request<wire::HostCallRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::permission_denied("unused test capability"))
    }
    async fn workloads(
        &self,
        _: Request<wire::HostWorkloadRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        panic!("outgoing-only must not allocate workloads")
    }
}
#[async_trait::async_trait]
impl Control for State {
    async fn call(
        &self,
        peer: &PeerIdentity,
        generation: u64,
        v: Value,
    ) -> Result<Value, ControlError> {
        if generation != 2
            || !matches!(peer,PeerIdentity::MutualTls{certificates} if certificates.first().is_some_and(|c|webdock_mail_protocol::sha256(c)==self.credential))
        {
            return Err(ControlError { status: 403 });
        }
        if !self.active.load(Ordering::SeqCst) {
            return Err(ControlError { status: 403 });
        }
        let mut row = self.row.lock().unwrap();
        Ok(match v["action"].as_str().unwrap() {
            "send.authenticate" => {
                self.auths.fetch_add(1, Ordering::SeqCst);
                if v["token"] != format!("wd_mail_{}", "x".repeat(43)) {
                    return Err(ControlError { status: 401 });
                }
                json!({"customerID":"7101"})
            }
            "send.admit" => self.admit(&mut row,&v["submission"],"key:71".into(),json!({"executor":"managed-mail-v1","payloadStoreID":"mail:platform:v1","payloadStoreVersion":1,"executorGeneration":2})),
            "send.admitAction" => {
                let a=&v["submission"];
                let token=a["invocationToken"].as_str().unwrap();
                let claims=webdock_plugin_protocol::verify_invocation(token,&self.issuer.verifying_key(),"mail",2,now()).unwrap();
                assert_eq!(claims.action,"mail.send");assert!(claims.job.is_none());
                let compiled=webdock_mail_message::compile_action(a["input"].clone(),&self.action_domain).unwrap();
                let proposed=json!({"payloadRef":a["payloadRef"],"payloadBytes":compiled.payload.len(),"requestHash":webdock_mail_protocol::sha256(&compiled.payload),"envelope":compiled.envelope});
                let source=json!({"executor":"managed-mail-v1","payloadStoreID":"mail:platform:v1","payloadStoreVersion":1,"executorGeneration":2,"projectID":claims.project_id,"resourceID":a["input"]["resource_id"],"domain":self.action_domain,"privatePolicySha256":a["privatePolicySha256"]});
                self.admit(&mut row,&proposed,format!("action:{}",webdock_mail_protocol::sha256(token.as_bytes())),source)
            }
            "send.commitPayload" => {
                row.state = "queued".into();
                json!({"queued":true})
            }
            "send.claim" => {
                self.claims.fetch_add(1, Ordering::SeqCst);
                // Test stand-in models Core handoff expiry; real Core expiry is Task4, not this proof.
                if row.state == "handoff" && row.attempted {
                    row.state = "unknown".into();
                }
                if row.state == "queued" {
                    row.state = "leased".into();
                    serde_json::to_value(row.job.as_ref().unwrap()).unwrap()
                } else {
                    Value::Null
                }
            }
            "send.reviewPreparation" => {
                row.state = "preparation_review".into();
                json!(true)
            }
            "send.begin" => {
                self.begins.fetch_add(1, Ordering::SeqCst);
                let j = row.job.as_ref().unwrap();
                let p = &v["preparation"];
                let c = LeaseClaims {
                    v: 1,
                    purpose: "mail-send".into(),
                    audience: "webdock.mail.runtime.v1".into(),
                    id: uuid::Uuid::new_v4().to_string(),
                    plugin_id: "mail".into(),
                    generation: 2,
                    customer_id: j.customer_id.clone(),
                    job_id: j.id.clone(),
                    source_id: j.source_id.clone(),
                    transport_revision: 1,
                    grant_revision: 1,
                    configuration_revision: 0,
                    descriptor_sha256: self.descriptor.clone(),
                    binding_sha256: "d".repeat(64),
                    worker_id: "mail".into(),
                    worker_generation: 2,
                    worker_credential_sha256: self.credential.clone(),
                    metadata_sha256: p["metadataSha256"].as_str().unwrap().into(),
                    mime_sha256: p["mimeSha256"].as_str().unwrap().into(),
                    mime_bytes: p["mimeBytes"].as_u64().unwrap(),
                    iat: now(),
                    exp: now() + 60,
                };
                row.state = "handoff".into();
                json!({"kind":"direct","leaseToken":c.sign(&self.issuer).unwrap(),"pluginID":"mail","generation":2,"descriptorSha256":self.descriptor})
            }
            "send.finish" => {
                row.state = v["result"]["state"].as_str().unwrap().into();
                json!(true)
            }
            "send.status" => json!({"id":row.job.as_ref().unwrap().id,"state":row.state}),
            "send.cleanup" => json!([]),
            _ => return Err(ControlError::invalid()),
        })
    }
}
impl State {
    fn admit(&self, row: &mut Row, s: &Value, source_id: String, source_context: Value) -> Value {
        let id = (7711 + self.jobs.fetch_add(1, Ordering::SeqCst)).to_string();
        let token = uuid::Uuid::new_v4().to_string();
        row.job = Some(SendLease {
            id: id.clone(),
            customer_id: "7101".into(),
            source_id,
            payload_ref: s["payloadRef"].as_str().unwrap().into(),
            payload_bytes: s["payloadBytes"].as_i64().unwrap() as i32,
            request_hash: s["requestHash"].as_str().unwrap().into(),
            payload_format: webdock_mail_protocol::PayloadFormat::Request,
            source_context,
            route: json!({"kind":"direct","pluginID":"mail","generation":2,"transportRevision":1,"grantRevision":1,"configurationRevision":0,"descriptorSha256":self.descriptor,"bindingSha256":"d".repeat(64)}),
            envelope: s["envelope"].clone(),
            lease_token: token.clone(),
        });
        row.state = "staging".into();
        row.attempted = false;
        json!({"id":id,"state":"staging","created":true,"customerID":"7101","payloadRef":s["payloadRef"],"uploadToken":token})
    }
}
#[tonic::async_trait]
impl mail::mail_authority_server::MailAuthority for Host {
    async fn guard(
        &self,
        _: Request<mail::LeaseProof>,
    ) -> Result<Response<mail::GuardReply>, Status> {
        Err(Status::permission_denied("not needed by direct executor"))
    }
    async fn begin_attempt(
        &self,
        request: Request<mail::LeaseProof>,
    ) -> Result<Response<mail::AttemptReply>, Status> {
        let c = webdock_mail_protocol::verify_lease(
            &request.into_inner().lease_token,
            &self.issuer.verifying_key(),
            "mail",
            2,
            now(),
        )
        .map_err(|_| Status::permission_denied("fixture lease denied"))?;
        let configuration = {
            let mut row = self.row.lock().unwrap();
            if row.attempted || row.state != "handoff" || c.job_id != row.job.as_ref().unwrap().id {
                return Err(Status::permission_denied("fixture one use denied"));
            }
            row.attempted = true;
            let job = row.job.as_ref().unwrap();
            let mut transport = json!({"selection":"native","customerID":"7101","accountID":null,"mode":"managed","region":"eu","transportRevision":1,"bindingRevision":1,"byokRevision":null});
            if job.source_id.starts_with("action:") {
                transport["selection"] = json!("project");
                transport["projectID"] = job.source_context["projectID"].clone();
                transport["privatePolicySha256"] =
                    job.source_context["privatePolicySha256"].clone();
            }
            serde_json::to_vec(
                &json!({"kind":"managed","sourceContext":job.source_context,"transport":transport}),
            )
            .unwrap()
        };
        self.attempt_started.store(true, Ordering::SeqCst);
        if self.pause_attempt.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(Response::new(mail::AttemptReply {
            admitted: true,
            private_configuration_json: configuration,
        }))
    }
}
fn ca() -> (rcgen::Certificate, rcgen::Issuer<'static, rcgen::KeyPair>) {
    let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    p.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let k = rcgen::KeyPair::generate().unwrap();
    (p.self_signed(&k).unwrap(), rcgen::Issuer::new(p, k))
}
fn certificate(
    issuer: &rcgen::Issuer<'static, rcgen::KeyPair>,
) -> (rcgen::Certificate, rcgen::KeyPair) {
    let mut p = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
    p.extended_key_usages = vec![
        rcgen::ExtendedKeyUsagePurpose::ServerAuth,
        rcgen::ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let k = rcgen::KeyPair::generate().unwrap();
    (p.signed_by(&k, issuer).unwrap(), k)
}
fn write(dir: &std::path::Path, name: &str, bytes: impl AsRef<[u8]>) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path.to_str().unwrap().into()
}
async fn channel(
    origin: String,
    cert: &rcgen::Certificate,
    key: &rcgen::KeyPair,
    root: &rcgen::Certificate,
    pin: &rcgen::Certificate,
) -> tonic::transport::Channel {
    webdock_plugin_runtime::pinned_mtls_channel(
        origin,
        "localhost".into(),
        PinnedMutualTls {
            certificate: cert.pem().into_bytes(),
            private_key: key.serialize_pem().into_bytes(),
            authority: root.pem().into_bytes(),
            server_certificate_sha256: webdock_mail_protocol::sha256(pin.der()),
        },
    )
    .await
    .unwrap()
}
async fn http(
    channel: tonic::transport::Channel,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header(
            "authorization",
            format!("Bearer wd_mail_{}", "x".repeat(43)),
        )
        .header("content-type", "application/json")
        .body(tonic::body::Body::new(axum::body::Body::from(
            body.to_string(),
        )))
        .unwrap();
    let response = channel.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}
async fn invoke(
    client: &mut wire::plugin_runtime_client::PluginRuntimeClient<tonic::transport::Channel>,
    state: &State,
    action: &str,
    input: Value,
) -> Value {
    let claims = json!({"v":1,"id":uuid::Uuid::new_v4().to_string(),"pluginID":"mail","generation":2,"action":action,"customerID":"7101","projectID":"7201","iat":now(),"exp":now()+60});
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let token = format!(
        "{body}.{}",
        URL_SAFE_NO_PAD.encode(state.issuer.sign(body.as_bytes()).to_bytes())
    );
    let result = client
        .invoke(wire::InvokeRequest {
            invocation_token: token,
            action: action.into(),
            input_json: serde_json::to_vec(&input).unwrap(),
            configuration_json: b"{}".to_vec(),
        })
        .await
        .expect("owned actual Invoke must reach durable handler/control")
        .into_inner();
    serde_json::from_slice(&result.output_json).unwrap()
}
async fn until(check: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("owned process condition deadline");
}
async fn terminate(child: &mut tokio::process::Child) {
    let id = child.id().unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", &id.to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(4), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}
async fn smtp_session(socket: tokio::net::TcpStream, s: Arc<State>) {
    let (read, mut write) = socket.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"220 owned catcher\r\n").await.unwrap();
    let mut data = false;
    while let Ok(Some(line)) = lines.next_line().await {
        let reply = if data {
            if line != "." {
                continue;
            }
            s.data.fetch_add(1, Ordering::SeqCst);
            if s.drop_final.load(Ordering::SeqCst) {
                return;
            }
            data = false;
            b"250 queued\r\n".as_slice()
        } else if line.starts_with("EHLO") {
            b"250-catcher\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n".as_slice()
        } else if line == "DATA" {
            data = true;
            b"354 go\r\n".as_slice()
        } else {
            b"250 ok\r\n".as_slice()
        };
        if write.write_all(reply).await.is_err() {
            return;
        }
    }
}
#[tokio::test]
#[ignore = "requires fresh task3-owned TLS PostgreSQL"]
async fn actual_mail_process_shared_endpoint_candidate_gateway_pin_receipts_and_sigterm() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let (root, issuer) = ca();
    let (runtime_cert, runtime_key) = certificate(&issuer);
    let (core_cert, core_key) = certificate(&issuer);
    let (gateway_cert, gateway_key) = certificate(&issuer);
    let (wrong_cert, wrong_key) = certificate(&issuer);
    let descriptor = webdock_mail_protocol::sha256(include_bytes!("../manifest.json"));
    let state = Arc::new(State {
        row: Mutex::new(Row {
            job: None,
            state: String::new(),
            attempted: false,
        }),
        active: AtomicBool::new(false),
        jobs: AtomicUsize::new(0),
        pause_attempt: AtomicBool::new(false),
        attempt_started: AtomicBool::new(false),
        claims: AtomicUsize::new(0),
        begins: AtomicUsize::new(0),
        auths: AtomicUsize::new(0),
        drop_final: AtomicBool::new(false),
        data: AtomicUsize::new(0),
        action_domain: format!("owned-process-{}.test", uuid::Uuid::new_v4().simple()),
        descriptor: descriptor.clone(),
        credential: webdock_mail_protocol::sha256(runtime_cert.der()),
        issuer: ed25519_dalek::SigningKey::from_bytes(&[31; 32]),
    });
    let smtp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let smtp_port = smtp.local_addr().unwrap().port();
    let captured = state.clone();
    let catcher = tokio::spawn(async move {
        loop {
            let (socket, _) = smtp.accept().await.unwrap();
            tokio::spawn(smtp_session(socket, captured.clone()));
        }
    });
    let core = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let core_port = core.local_addr().unwrap().port();
    let core_tls = tonic::transport::ServerTlsConfig::new()
        .identity(tonic::transport::Identity::from_pem(
            core_cert.pem(),
            core_key.serialize_pem(),
        ))
        .client_ca_root(tonic::transport::Certificate::from_pem(root.pem()));
    let services = tonic::service::Routes::new(wire::plugin_host_server::PluginHostServer::new(
        Host(state.clone()),
    ))
    .add_service(ControlServer::service(state.clone()))
    .add_service(mail::mail_authority_server::MailAuthorityServer::new(Host(
        state.clone(),
    )));
    let core_task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(core_tls)
            .unwrap()
            .add_routes(services)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(core))
            .await
            .unwrap();
    });
    let reserved = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let manifest = write(
        dir.path(),
        "manifest.json",
        include_bytes!("../manifest.json"),
    );
    let runtime_pem = write(dir.path(), "runtime.crt", runtime_cert.pem());
    let runtime_key_path = write(dir.path(), "runtime.key", runtime_key.serialize_pem());
    let ca_path = write(dir.path(), "ca.crt", root.pem());
    let config = json!({"manifest":manifest,"issuerPublicKeyHex":hex::encode(state.issuer.verifying_key().as_bytes()),"generation":2,"listenTls":{"address":format!("127.0.0.1:{port}"),"certificate":runtime_pem,"privateKey":runtime_key_path,"authority":ca_path},"coreTls":{"origin":format!("https://localhost:{core_port}"),"domain":"localhost","certificate":runtime_pem,"privateKey":runtime_key_path,"authority":ca_path,"serverCertificateSha256":webdock_mail_protocol::sha256(core_cert.der())},"corePeers":[{"kind":"mutualTls","certificateSha256":webdock_mail_protocol::sha256(core_cert.der())}],"storageUrl":std::env::var("WEBDOCK_MAIL_TASK3_TEST_URL").unwrap(),"storageCa":std::env::var("WEBDOCK_MAIL_TASK3_TEST_CA").unwrap(),"secretKeyHex":"24".repeat(32),"trustUnverifiedDomains":true,"platformTransport":{"kind":"smtp","host":"127.0.0.1","port":smtp_port},"delivery":{"nativeLeaseIssuerPublicKeyHex":hex::encode(state.issuer.verifying_key().as_bytes()),"providerBinding":{"pluginID":"mail","generation":2,"descriptorSha256":descriptor,"workerID":"mail","workerGeneration":2,"workerCredentialSha256":state.credential},"gatewayPeers":[{"kind":"mutualTls","certificateSha256":webdock_mail_protocol::sha256(gateway_cert.der())}],"nativeAccounts":[{"customerID":"7101","accountID":null,"mode":"managed","transportRevision":1,"bindingRevision":1,"byokRevision":null,"transport":{"kind":"smtp","host":"127.0.0.1","port":smtp_port}}]}});
    // Normal installer profile must bootstrap without a post-admission credential override.
    let mut config = config;
    config["delivery"]
        .as_object_mut()
        .unwrap()
        .remove("providerBinding");
    let path = write(
        dir.path(),
        "configuration.json",
        serde_json::to_vec(&config).unwrap(),
    );
    let spawn = || {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_webdock-mail"))
            .arg(&path)
            .kill_on_drop(true)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    };
    let mut process = spawn();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            assert!(
                process.try_wait().unwrap().is_none(),
                "owned Mail process exited"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let origin = format!("https://localhost:{port}");
    let core_channel = channel(origin.clone(), &core_cert, &core_key, &root, &runtime_cert).await;
    let mut rpc = wire::plugin_runtime_client::PluginRuntimeClient::new(core_channel);
    assert_eq!(
        rpc.health(wire::HealthRequest {})
            .await
            .unwrap()
            .into_inner()
            .plugin_id,
        "mail"
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(state.claims.load(Ordering::SeqCst), 0);
    assert_eq!(state.data.load(Ordering::SeqCst), 0);
    let wrong = channel(
        origin.clone(),
        &wrong_cert,
        &wrong_key,
        &root,
        &runtime_cert,
    )
    .await;
    assert_eq!(http(wrong, "POST", "/v1/emails", json!({})).await.0, 403);
    assert_eq!(state.auths.load(Ordering::SeqCst), 0);
    let gateway = channel(
        origin.clone(),
        &gateway_cert,
        &gateway_key,
        &root,
        &runtime_cert,
    )
    .await;
    assert_eq!(
        http(gateway.clone(), "POST", "/v1/emails", json!({}))
            .await
            .0,
        403
    );
    assert_eq!(state.data.load(Ordering::SeqCst), 0);
    state.active.store(true, Ordering::SeqCst);
    let body = json!({"from":"sender@example.test","to":"recipient@example.test","subject":"owned fixture","text":"bounded local only"});
    let (status, receipt) = http(gateway.clone(), "POST", "/v1/emails", body.clone()).await;
    assert_eq!(status, 202);
    assert_eq!(receipt, json!({"id":"7711","state":"queued"}));
    until(|| state.row.lock().unwrap().state == "accepted").await;
    assert_eq!(state.data.load(Ordering::SeqCst), 1);
    assert_eq!(state.begins.load(Ordering::SeqCst), 1);
    assert_eq!(
        http(gateway.clone(), "GET", "/v1/emails/7711", Value::Null)
            .await
            .1,
        json!({"id":"7711","state":"accepted"})
    );
    state.drop_final.store(true, Ordering::SeqCst);
    assert_eq!(
        http(gateway.clone(), "POST", "/v1/emails", body).await.0,
        202
    );
    until(|| state.row.lock().unwrap().state == "unknown").await;
    assert_eq!(state.data.load(Ordering::SeqCst), 2);
    let reference = state
        .row
        .lock()
        .unwrap()
        .job
        .as_ref()
        .unwrap()
        .payload_ref
        .clone();
    let store = Arc::new(
        webdock_mail::store::Store::connect_tls(
            &std::env::var("WEBDOCK_MAIL_TASK3_TEST_URL").unwrap(),
            &std::fs::read(std::env::var("WEBDOCK_MAIL_TASK3_TEST_CA").unwrap()).unwrap(),
        )
        .await
        .unwrap(),
    );
    let payload =
        webdock_mail::payload::PgPrivatePayloadStore::new(store.clone(), &"24".repeat(32))
            .await
            .unwrap();
    assert!(
        webdock_mail_runtime::PrivatePayloadStore::get(&payload, "7101", &reference)
            .await
            .is_ok()
    );
    assert!(store.engines().await.unwrap().is_empty());
    terminate(&mut process).await;
    drop(gateway);
    drop(rpc);
    let mut process = spawn();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(process.try_wait().unwrap().is_none());
    assert_eq!(state.data.load(Ordering::SeqCst), 2);
    assert_eq!(state.row.lock().unwrap().state, "unknown");
    terminate(&mut process).await;
    // Force an owned process exit after Core handoff/one-use attempt but before authorization ACK.
    state.pause_attempt.store(true, Ordering::SeqCst);
    state.attempt_started.store(false, Ordering::SeqCst);
    let mut process = spawn();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let gateway = channel(
        origin.clone(),
        &gateway_cert,
        &gateway_key,
        &root,
        &runtime_cert,
    )
    .await;
    assert_eq!(http(gateway,"POST","/v1/emails",json!({"from":"sender@example.test","to":"recipient@example.test","subject":"forced owned attempt","text":"body"})).await.0,202);
    until(|| state.attempt_started.load(Ordering::SeqCst)).await;
    assert_eq!(state.row.lock().unwrap().state, "handoff");
    let forced_ref = state
        .row
        .lock()
        .unwrap()
        .job
        .as_ref()
        .unwrap()
        .payload_ref
        .clone();
    assert!(
        std::process::Command::new("kill")
            .args(["-KILL", &process.id().unwrap().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(!process.wait().await.unwrap().success());
    assert!(
        webdock_mail_runtime::PrivatePayloadStore::get(&payload, "7101", &forced_ref)
            .await
            .is_ok()
    );
    let mut process = spawn();
    until(|| state.row.lock().unwrap().state == "unknown").await;
    assert_eq!(state.data.load(Ordering::SeqCst), 2);
    terminate(&mut process).await;
    state.pause_attempt.store(false, Ordering::SeqCst);
    let mut process = spawn();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let core_channel = channel(origin, &core_cert, &core_key, &root, &runtime_cert).await;
    let mut rpc = wire::plugin_runtime_client::PluginRuntimeClient::new(core_channel)
        .max_encoding_message_size(224 * 1024);
    let created = invoke(
        &mut rpc,
        &state,
        "domains.create",
        json!({"domain":state.action_domain}),
    )
    .await;
    let resource = created["resource"]["id"].clone();
    for chars in [20000, 30000] {
        let before = state.data.load(Ordering::SeqCst);
        let queued=invoke(&mut rpc,&state,"mail.send",json!({"resource_id":resource,"from":"hello","from_name":"😀".repeat(80),"to":"recipient@example.test","subject":"owned Unicode Invoke","text":"😀".repeat(chars)})).await;
        assert_eq!(queued["state"], "queued");
        until(|| {
            state.data.load(Ordering::SeqCst) == before + 1
                && state.row.lock().unwrap().state == "unknown"
        })
        .await;
    }
    assert_eq!(state.jobs.load(Ordering::SeqCst), 5);
    terminate(&mut process).await;
    core_task.abort();
    catcher.abort();
}
