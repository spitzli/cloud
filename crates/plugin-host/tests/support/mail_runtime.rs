//! Real Mail process, real TLS PostgreSQL; Host and JMAP are counted local stand-ins.
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tonic::{Request, Response, Status};
use webdock_plugin_host::provisioning::{
    storage::{MetadataProvisioner, StoragePlan},
    tls::{RuntimeIdentity, issue_runtime_identity},
};
use webdock_plugin_runtime::{PinnedMutualTls, authenticated_peer, wire};

#[allow(
    dead_code,
    reason = "Reusable PostgreSQL fixture also exposes helpers for other suites"
)]
#[path = "provisioning_fixture.rs"]
mod postgres;
#[allow(
    dead_code,
    reason = "Reusable child/file helpers also contain the PostgreSQL DenyHost fixture"
)]
#[path = "runtime_fixture.rs"]
mod process;
pub use process::private;
pub fn mail_child(child: std::process::Child) -> process::Child {
    process::Child(child)
}

pub struct Server(pub tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Default)]
pub struct Calls {
    pub active: AtomicU64,
    pub peers: AtomicUsize,
    pub payloads: AtomicUsize,
    pub invocations: AtomicUsize,
    pub workloads: Mutex<Vec<(String, Value)>>,
    pub jmap: Mutex<Vec<(String, String)>>,
}
#[derive(Clone)]
pub struct Host {
    pub calls: Arc<Calls>,
    pub runtime_pin: String,
    pub unix: bool,
}
impl Host {
    fn peer<T>(&self, request: &Request<T>) -> Result<(), Status> {
        let valid = match authenticated_peer(request)? {
            webdock_plugin_runtime::PeerIdentity::Unix { uid } => {
                self.unix && uid == rustix::process::geteuid().as_raw()
            }
            webdock_plugin_runtime::PeerIdentity::MutualTls { certificates } => {
                !self.unix
                    && certificates
                        .first()
                        .is_some_and(|leaf| hex::encode(Sha256::digest(leaf)) == self.runtime_pin)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(Status::permission_denied("runtime peer"))
        }
    }
    fn active(&self, plugin: &str, generation: u64) -> Result<(), Status> {
        if plugin == "mail"
            && generation > 0
            && self.calls.active.load(Ordering::SeqCst) == generation
        {
            Ok(())
        } else {
            Err(Status::permission_denied("inactive runtime"))
        }
    }
    fn claims(&self, bytes: &[u8]) -> Result<(), Status> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| Status::invalid_argument("claims"))?;
        self.active(
            value["pluginID"].as_str().unwrap_or(""),
            value["generation"].as_u64().unwrap_or(0),
        )
    }
}
#[tonic::async_trait]
impl wire::plugin_host_server::PluginHost for Host {
    async fn authorize_peer(
        &self,
        request: Request<wire::HostPeerRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        self.peer(&request)?;
        self.calls.peers.fetch_add(1, Ordering::SeqCst);
        let r = request.into_inner();
        self.active(&r.plugin_id, r.generation)?;
        Ok(Response::new(wire::HostAuthorityResponse { active: true }))
    }
    async fn authorize_invocation(
        &self,
        request: Request<wire::HostInvocationRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        self.peer(&request)?;
        self.claims(&request.into_inner().claims_json)?;
        self.calls.invocations.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(wire::HostAuthorityResponse { active: true }))
    }
    async fn authorize_request(
        &self,
        request: Request<wire::HostPayloadRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        self.peer(&request)?;
        let r = request.into_inner();
        self.claims(&r.claims_json)?;
        self.calls.payloads.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(wire::HostAuthorityResponse { active: true }))
    }
    async fn call(
        &self,
        _: Request<wire::HostCallRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn workloads(
        &self,
        request: Request<wire::HostWorkloadRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        self.peer(&request)?;
        let r = request.into_inner();
        self.active(&r.plugin_id, r.generation)?;
        let input: Value = serde_json::from_slice(&r.input_json)
            .map_err(|_| Status::invalid_argument("workload"))?;
        self.calls
            .workloads
            .lock()
            .unwrap()
            .push((r.method.clone(), input));
        let value = match r.method.as_str() {
            "apply" => json!({"revision":1}),
            "observe" => json!({"appliedRevision":1,"observed":{"ready":true}}),
            _ => return Err(Status::permission_denied("remove forbidden")),
        };
        Ok(Response::new(wire::HostCallResponse {
            output_json: serde_json::to_vec(&value).unwrap(),
        }))
    }
}

pub struct Fixture {
    pub pg: postgres::Fixture,
    pub directory: tempfile::TempDir,
    pub config: Value,
    pub sql: tokio_postgres::Client,
    pub calls: Arc<Calls>,
    pub ca: String,
    pub ca_key: String,
    pub core: RuntimeIdentity,
    pub runtime: RuntimeIdentity,
    pub port: u16,
    pub binary: PathBuf,
    pub servers: Vec<Server>,
}
impl Fixture {
    pub async fn new() -> Self {
        let binary = PathBuf::from(
            std::env::var_os("WEBDOCK_MAIL_RUNTIME_BINARY")
                .expect("explicit owned Mail executable required"),
        );
        assert!(
            binary.is_absolute() && binary.is_file(),
            "owned Mail executable must be an absolute file"
        );
        let pg = postgres::Fixture::start().await;
        let directory = tempfile::tempdir().unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let p = directory.path();
        let provisioner = MetadataProvisioner::connect(&pg.admin_url, Some(pg.ca.as_bytes()))
            .await
            .unwrap();
        let plan = StoragePlan::new(
            "mail-process",
            "mail",
            "a".repeat(64),
            "localhost".into(),
            pg.port,
        )
        .unwrap();
        let storage = provisioner.provision(&plan).await.unwrap();
        let sql = webdock_plugin_kit::postgres::connect(&storage.url, Some(pg.ca.as_bytes()))
            .await
            .unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        let ca = params.self_signed(&key).unwrap().pem();
        let ca_key = key.serialize_pem();
        let core = issue_runtime_identity(&ca, &ca_key, "localhost").unwrap();
        let runtime = issue_runtime_identity(&ca, &ca_key, "localhost").unwrap();
        for (name, value) in [
            ("runtime.crt", runtime.certificate_pem.as_str()),
            ("runtime.key", runtime.private_key_pem.as_str()),
            ("core-ca.crt", ca.as_str()),
            ("storage-ca.crt", pg.ca.as_str()),
        ] {
            private(&p.join(name), value);
        }
        private(
            &p.join("manifest.json"),
            include_bytes!("../../../../plugins/mail/manifest.json"),
        );
        let calls = Arc::new(Calls::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let core_port = listener.local_addr().unwrap().port();
        let stream = futures_util::stream::unfold(listener, |l| async move {
            let io = l.accept().await.map(|(s, _)| s);
            Some((io, l))
        });
        let host = Host {
            calls: calls.clone(),
            runtime_pin: runtime.certificate_sha256.clone(),
            unix: false,
        };
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let server = tonic::transport::Server::builder()
            .tls_config(
                tonic::transport::ServerTlsConfig::new()
                    .identity(tonic::transport::Identity::from_pem(
                        &core.certificate_pem,
                        &core.private_key_pem,
                    ))
                    .client_ca_root(tonic::transport::Certificate::from_pem(&ca)),
            )
            .unwrap()
            .add_service(wire::plugin_host_server::PluginHostServer::new(host));
        let servers = vec![Server(tokio::spawn(async move {
            server.serve_with_incoming(stream).await.unwrap();
        }))];
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let config = json!({"manifest":p.join("manifest.json"),"issuerPublicKeyHex":hex::encode(SigningKey::from_bytes(&[73;32]).verifying_key().as_bytes()),"generation":1,
            "listenTls":{"address":format!("127.0.0.1:{port}"),"certificate":p.join("runtime.crt"),"privateKey":p.join("runtime.key"),"authority":p.join("core-ca.crt")},
            "coreTls":{"origin":format!("https://localhost:{core_port}"),"domain":"localhost","certificate":p.join("runtime.crt"),"privateKey":p.join("runtime.key"),"authority":p.join("core-ca.crt"),"serverCertificateSha256":core.certificate_sha256},
            "corePeers":[{"kind":"mutualTls","certificateSha256":core.certificate_sha256}],"storageUrl":storage.url,"storageCa":p.join("storage-ca.crt"),"secretKeyHex":"7a".repeat(32),"platformTransport":{"kind":"smtp","host":"127.0.0.1","port":9},"mailboxes":null});
        Self {
            pg,
            directory,
            config,
            sql,
            calls,
            ca,
            ca_key,
            core,
            runtime,
            port,
            binary,
            servers,
        }
    }
    pub fn start(&self, config: &Value) -> process::Child {
        let path = self.directory.path().join("runtime.json");
        private(&path, serde_json::to_vec(config).unwrap());
        process::Child(
            std::process::Command::new(&self.binary)
                .arg(path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }
    pub fn tls(&self) -> PinnedMutualTls {
        PinnedMutualTls {
            certificate: self.core.certificate_pem.as_bytes().to_vec(),
            private_key: self.core.private_key_pem.as_bytes().to_vec(),
            authority: self.ca.as_bytes().to_vec(),
            server_certificate_sha256: self.runtime.certificate_sha256.clone(),
        }
    }
    pub async fn client(
        &self,
        child: &mut process::Child,
    ) -> wire::plugin_runtime_client::PluginRuntimeClient<tonic::transport::Channel> {
        for _ in 0..100 {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "Mail exited before listener readiness; diagnostics withheld"
            );
            if let Ok(mut c) = webdock_plugin_runtime::connect_pinned_mtls(
                format!("https://localhost:{}", self.port),
                "localhost".into(),
                self.tls(),
            )
            .await
                && c.health(wire::HealthRequest {}).await.is_ok()
            {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("Mail listener readiness deadline");
    }
    pub async fn unix(&mut self) -> Value {
        let mut socket =
            webdock_plugin_runtime::UnixSocket::bind(self.directory.path().join("core.sock"))
                .unwrap();
        let incoming = socket.incoming().unwrap();
        let host = Host {
            calls: self.calls.clone(),
            runtime_pin: String::new(),
            unix: true,
        };
        self.servers.push(Server(tokio::spawn(async move {
            let _socket = socket;
            tonic::transport::Server::builder()
                .add_service(wire::plugin_host_server::PluginHostServer::new(host))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        })));
        let mut config = self.config.clone();
        let map = config.as_object_mut().unwrap();
        map.remove("listenTls");
        map.remove("coreTls");
        map.remove("storageCa");
        config["listen"] = json!(self.directory.path().join("mail.sock"));
        config["core"] = json!(self.directory.path().join("core.sock"));
        config["corePeers"] = json!([{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]);
        config
    }
    pub async fn snapshot(&self) -> String {
        self.sql.query_one("SELECT jsonb_build_object('engines',(SELECT jsonb_agg(to_jsonb(e) ORDER BY customer_id) FROM customer_engines e),'mailboxes',(SELECT jsonb_agg(to_jsonb(m) ORDER BY id) FROM mailboxes m),'domains',(SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM domains d),'relays',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM relays r))::text",&[]).await.unwrap().get(0)
    }
}

pub fn invoke(generation: u64, action: &str, input: Value) -> wire::InvokeRequest {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({"v":1,"id":uuid::Uuid::new_v4().to_string(),"pluginID":"mail","generation":generation,"action":action,"projectID":"11","customerID":"101","iat":now,"exp":now+60});
    let body = URL_SAFE_NO_PAD.encode(claims.to_string());
    let key = SigningKey::from_bytes(&[73; 32]);
    wire::InvokeRequest {
        invocation_token: format!(
            "{body}.{}",
            URL_SAFE_NO_PAD.encode(key.sign(body.as_bytes()).to_bytes())
        ),
        action: action.into(),
        input_json: serde_json::to_vec(&input).unwrap(),
        configuration_json: b"{}".to_vec(),
    }
}

pub async fn wait_until(mut ready: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if ready().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("owned Mail observation deadline");
}

pub async fn jmap(calls: Arc<Calls>) -> (String, Server) {
    use axum::{
        Json,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
    };
    async fn handle(
        State(calls): State<Arc<Calls>>,
        headers: HeaderMap,
        Json(input): Json<Value>,
    ) -> impl IntoResponse {
        let host = headers.get("host").unwrap().to_str().unwrap().to_owned();
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|v| STANDARD.decode(v).ok())
            .unwrap_or_default();
        let customer = host.split('.').next().unwrap().trim_start_matches('c');
        let call = &input["methodCalls"][0];
        let method = call[0].as_str().unwrap();
        calls
            .jmap
            .lock()
            .unwrap()
            .push((host.clone(), method.into()));
        if auth.starts_with(b"admin:") {
            return (StatusCode::UNAUTHORIZED, Json(json!({})));
        }
        assert!(
            auth == format!("webdock-admin@mail.c{customer}.webdock.internal:fixture-admin")
                .as_bytes(),
            "retained engine credential was not recovered"
        );
        let value = match method {
            "x:Domain/get" => {
                json!({"list":[{"id":"retained-domain","name":format!("c{customer}.example.test") }]})
            }
            "x:Account/get" => {
                json!({"list":[{"id":"retained-admin","name":"webdock-admin"},{"id":"retained-account","name":"hello","domainId":"retained-domain"}]})
            }
            "x:Account/set" => {
                assert!(
                    call[1].get("create").is_none(),
                    "retained accounts must be reused"
                );
                for (account, update) in call[1]["update"].as_object().unwrap() {
                    let expected = if account == "retained-admin" {
                        "fixture-admin"
                    } else {
                        "fixture-mailbox-password"
                    };
                    assert!(
                        update["credentials"]["0"]["secret"] == expected,
                        "retained account credential changed"
                    );
                }
                json!({"updated":{}})
            }
            "x:NetworkListener/get" => {
                json!({"list":[{"id":"http","name":"http"},{"id":"imaps","name":"imaps"},{"id":"submissions","name":"submissions"}]})
            }
            "x:Certificate/get" => json!({"list":[{"id":"retained-certificate"}]}),
            "x:SystemSettings/set" => json!({"updated":{}}),
            _ => panic!("unexpected JMAP fixture method"),
        };
        (
            StatusCode::OK,
            Json(json!({"methodResponses":[[method,value,"w"]]})),
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route("/jmap", axum::routing::post(handle))
        .with_state(calls);
    (
        origin,
        Server(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        })),
    )
}

pub fn mailboxes(origin: &str) -> Value {
    json!({"image":format!("fixture.invalid/stalwart@sha256:{}","a".repeat(64)),"domain":"mail.example.test","imapPort":1993,"submissionPort":2465,"admin":{"origin":origin,"domain":"mail-admin.example.test","entryPoints":["web"]}})
}

pub async fn seed(sql: &tokio_postgres::Client) {
    let sealer = webdock_plugin_kit::seal::Sealer::new(&"7a".repeat(32)).unwrap();
    for (customer, state, enabled, mailbox_state) in [
        ("101", "ready", true, "failed"),
        ("102", "failed", true, "creating"),
        ("103", "suspended", false, "creating"),
    ] {
        let engine = uuid::Uuid::new_v4();
        let mailbox = uuid::Uuid::new_v4();
        let domain = uuid::Uuid::new_v4();
        sql.execute("INSERT INTO customer_engines(customer_id,id,admin_sealed,state,desired_enabled) VALUES($1,$2::text::uuid,$3,$4,$5)",&[&customer,&engine.to_string(),&sealer.seal(&engine,"fixture-admin"),&state,&enabled]).await.unwrap();
        sql.execute("INSERT INTO domains(id,project_id,domain,token,verified_at) VALUES($1::text::uuid,'11',$2,'retained-token',now())",&[&domain.to_string(),&format!("c{customer}.example.test")]).await.unwrap();
        sql.execute("INSERT INTO mailboxes(id,project_id,domain_id,local_part,display_name,password_sealed,account_id,quota_mb,state,customer_id) VALUES($1::text::uuid,'11',$2::text::uuid,'hello','Retained',$3,'retained-account',1024,$4,$5)",&[&mailbox.to_string(),&domain.to_string(),&sealer.seal(&mailbox,"fixture-mailbox-password"),&mailbox_state,&customer]).await.unwrap();
    }
}

pub async fn shutdown(child: &mut process::Child) {
    assert!(
        std::process::Command::new("kill")
            .args(["-INT", &child.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait_until(async || child.0.try_wait().unwrap().is_some()).await;
    assert!(
        child.0.try_wait().unwrap().unwrap().success(),
        "Mail graceful shutdown failed"
    );
}

pub fn config_path(directory: &Path) -> PathBuf {
    directory.join("runtime.json")
}

pub fn callback(
    listener: tokio::net::TcpListener,
    host: Host,
    core: &RuntimeIdentity,
    ca: &str,
) -> Server {
    let stream = futures_util::stream::unfold(listener, |l| async move {
        let io = l.accept().await.map(|(s, _)| s);
        Some((io, l))
    });
    let server = tonic::transport::Server::builder()
        .tls_config(
            tonic::transport::ServerTlsConfig::new()
                .identity(tonic::transport::Identity::from_pem(
                    &core.certificate_pem,
                    &core.private_key_pem,
                ))
                .client_ca_root(tonic::transport::Certificate::from_pem(ca)),
        )
        .unwrap()
        .add_service(wire::plugin_host_server::PluginHostServer::new(host));
    Server(tokio::spawn(async move {
        server.serve_with_incoming(stream).await.unwrap();
    }))
}
