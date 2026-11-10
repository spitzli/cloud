use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use spitzli_plugin_http::{HttpBinding, HttpRule};
use spitzli_plugin_protocol::{InvocationClaims, Manifest};
use spitzli_plugin_runtime::{UnixSocket, wire};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tonic::{Request, Response, Status};

const PROJECT: &str = "1428d399-8861-40a5-a85f-6ed4c669bae0";
const SERVER: &str = "2428d399-8861-40a5-a85f-6ed4c669bae0";
type Admissions = Arc<Mutex<BTreeMap<String, (InvocationClaims, Vec<u8>)>>>;

#[derive(Clone)]
struct Host {
    active: Arc<AtomicBool>,
    admissions: Admissions,
    binding: HttpBinding,
}
impl Host {
    fn claims(&self, bytes: &[u8]) -> Result<InvocationClaims, Status> {
        let claims: InvocationClaims =
            serde_json::from_slice(bytes).map_err(|_| Status::permission_denied("claims"))?;
        if !self.active.load(Ordering::SeqCst)
            || self
                .admissions
                .lock()
                .unwrap()
                .get(&claims.id)
                .is_none_or(|entry| entry.0 != claims)
        {
            return Err(Status::permission_denied("inactive"));
        }
        Ok(claims)
    }
}
#[tonic::async_trait]
impl wire::plugin_host_server::PluginHost for Host {
    async fn authorize_peer(
        &self,
        request: Request<wire::HostPeerRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        let request = request.into_inner();
        Ok(Response::new(wire::HostAuthorityResponse {
            active: self.active.load(Ordering::SeqCst)
                && request.plugin_id == "lumaserv-compute"
                && request.generation == 1,
        }))
    }
    async fn authorize_invocation(
        &self,
        request: Request<wire::HostInvocationRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        self.claims(&request.into_inner().claims_json)?;
        Ok(Response::new(wire::HostAuthorityResponse { active: true }))
    }
    async fn authorize_request(
        &self,
        request: Request<wire::HostPayloadRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        let request = request.into_inner();
        let claims = self.claims(&request.claims_json)?;
        if self.admissions.lock().unwrap()[&claims.id].1 != request.input_json
            || request.configuration_json != b"{}"
        {
            return Err(Status::permission_denied("payload"));
        }
        Ok(Response::new(wire::HostAuthorityResponse { active: true }))
    }
    async fn workloads(
        &self,
        _: Request<wire::HostWorkloadRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::unimplemented("not used by this plugin"))
    }
    async fn call(
        &self,
        request: Request<wire::HostCallRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        let request = request.into_inner();
        let claims = spitzli_plugin_protocol::verify_invocation(
            &request.invocation_token,
            &SigningKey::from_bytes(&[7; 32]).verifying_key(),
            "lumaserv-compute",
            1,
            now(),
        )
        .map_err(|_| Status::permission_denied("token"))?;
        self.claims(&serde_json::to_vec(&claims).unwrap())?;
        let input: Value = serde_json::from_slice(&request.input_json).unwrap();
        if request.service != "http"
            || request.method != "request"
            || input["binding"] != "compute"
            || input["method"] != "GET"
            || input.as_object().unwrap().len() != 3
        {
            return Err(Status::permission_denied("HTTP grant"));
        }
        let target = spitzli_plugin_http::bind_target(
            "GET",
            input["path"].as_str().unwrap(),
            self.binding.clone(),
        )
        .map_err(|_| Status::permission_denied("HTTP target"))?;
        let result = spitzli_plugin_http::send_json(target, None)
            .await
            .map_err(|_| Status::unavailable("HTTP failure"))?;
        self.claims(&serde_json::to_vec(&claims).unwrap())?;
        let body: Value =
            serde_json::from_slice(&result.body).map_err(|_| Status::unavailable("HTTP JSON"))?;
        Ok(Response::new(wire::HostCallResponse {
            output_json: serde_json::to_vec(&json!({"status":result.status,"body":body})).unwrap(),
        }))
    }
}
fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}
fn invocation(
    admissions: &Admissions,
    action: &str,
    project: &str,
    input: Value,
) -> wire::InvokeRequest {
    let claims = InvocationClaims {
        job: None,
        v: 1,
        id: uuid::Uuid::new_v4().to_string(),
        plugin_id: "lumaserv-compute".into(),
        generation: 1,
        action: action.into(),
        project_id: project.into(),
        customer_id: "101".into(),
        iat: now(),
        exp: now() + 60,
    };
    let input_json = serde_json::to_vec(&input).unwrap();
    admissions
        .lock()
        .unwrap()
        .insert(claims.id.clone(), (claims.clone(), input_json.clone()));
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let signature = SigningKey::from_bytes(&[7; 32]).sign(body.as_bytes());
    wire::InvokeRequest {
        invocation_token: format!("{body}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes())),
        action: action.into(),
        input_json,
        configuration_json: b"{}".to_vec(),
    }
}
struct Tasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}
async fn provider(
    mode: Arc<AtomicU8>,
    reached: Arc<tokio::sync::Notify>,
) -> (HttpBinding, tokio::task::JoinHandle<()>) {
    let generated = rcgen::generate_simple_self_signed(vec!["provider.invalid".into()]).unwrap();
    let tls = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![generated.cert.der().clone()],
        tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(
            generated.signing_key.serialize_der(),
        )
        .into(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let mut request = vec![0; 8192];
            let length = stream.read(&mut request).await.unwrap();
            let request = std::str::from_utf8(&request[..length]).unwrap();
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer controlled-fixture-secret")
            );
            let path = request.split_ascii_whitespace().nth(1).unwrap();
            let mode = mode.load(Ordering::SeqCst);
            if mode == 4 {
                reached.notify_one();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let row = json!({"id":SERVER,"project_id":if mode == 1 {"foreign-project"} else {PROJECT},"name":"Fixture server","state":if mode == 3 {"UNRECOGNIZED"} else {"ONLINE"},"root_password":"must-not-leave-provider","labels":{"secret":"redact"}});
            let data = if path.ends_with("/status") {
                json!({"online":true,"password":"redact"})
            } else if path.starts_with("/compute/servers?") {
                assert!(path.contains(&format!("filter[project_id]={PROJECT}")));
                assert!(path.contains("page=1&page_size=50"));
                json!([row])
            } else {
                assert_eq!(path, format!("/compute/servers/{SERVER}"));
                row
            };
            let body = json!({"success": mode != 2,"data":data,"messages":{"errors":["private provider diagnostics"]}}).to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    let rules = vec![
        HttpRule {
            method: "GET".into(),
            path: "/compute/servers".into(),
            query: vec![
                "filter[project_id]".into(),
                "page".into(),
                "page_size".into(),
            ],
        },
        HttpRule {
            method: "GET".into(),
            path: format!("/compute/servers/{SERVER}"),
            query: vec![],
        },
        HttpRule {
            method: "GET".into(),
            path: format!("/compute/servers/{SERVER}/status"),
            query: vec![],
        },
    ];
    (
        HttpBinding {
            origin: format!("https://provider.invalid:{port}"),
            addresses: vec!["127.0.0.1".parse().unwrap()],
            headers: BTreeMap::from([(
                "Authorization".into(),
                "Bearer controlled-fixture-secret".into(),
            )]),
            rules,
            ca: Some(generated.cert.pem()),
            website_binding_id: None,
        },
        task,
    )
}
fn write(path: &std::path::Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
#[tokio::test]
async fn standalone_private_rpc_scoped_https_redaction_and_revocation() {
    let manifest = Manifest::parse(include_bytes!("manifest.json")).unwrap();
    spitzli_lumaserv_compute::validate_manifest(&manifest).unwrap();
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mode = Arc::new(AtomicU8::new(0));
    let reached = Arc::new(tokio::sync::Notify::new());
    let (binding, provider_task) = provider(mode.clone(), reached.clone()).await;
    let active = Arc::new(AtomicBool::new(true));
    let admissions = Arc::new(Mutex::new(BTreeMap::new()));
    let host = Host {
        active: active.clone(),
        admissions: admissions.clone(),
        binding,
    };
    let host_path = directory.path().join("host.sock");
    let plugin_path = directory.path().join("plugin.sock");
    let manifest_path = directory.path().join("manifest.json");
    let config_path = directory.path().join("config.json");
    write(&manifest_path, manifest.as_bytes());
    let config = json!({
        "manifest":manifest_path,"issuerPublicKeyHex":hex::encode(SigningKey::from_bytes(&[7;32]).verifying_key().as_bytes()),
        "generation":1,"projects":{"123":PROJECT},"listen":{"kind":"unix","path":plugin_path},
        "core":{"kind":"unix","path":host_path},"corePeers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]
    });
    write(&config_path, config.to_string().as_bytes());
    let mut socket = UnixSocket::bind(host_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let host_task = tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(wire::plugin_host_server::PluginHostServer::new(host))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let _tasks = Tasks(vec![provider_task, host_task]);
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_spitzli-lumaserv-compute"))
        .arg(&config_path)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut client = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(child.try_wait().unwrap().is_none(), "plugin exited");
            if let Ok(mut client) = spitzli_plugin_runtime::connect_unix(&plugin_path).await
                && client.health(wire::HealthRequest {}).await.is_ok()
            {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let response = client
        .invoke(invocation(&admissions, "servers.list", "123", json!({})))
        .await
        .unwrap()
        .into_inner();
    let output: Value = serde_json::from_slice(&response.output_json).unwrap();
    assert_eq!(
        output["servers"],
        json!([{"id":SERVER,"name":"Fixture server","state":"ONLINE"}])
    );
    let response = client
        .invoke(invocation(
            &admissions,
            "servers.get",
            "123",
            json!({"server":SERVER}),
        ))
        .await
        .unwrap()
        .into_inner();
    let output: Value = serde_json::from_slice(&response.output_json).unwrap();
    assert_eq!(
        output,
        json!({"server":{"id":SERVER,"name":"Fixture server","state":"ONLINE"},"online":true})
    );
    let panel = client
        .invoke(invocation(&admissions, "servers.panel", "123", json!({})))
        .await
        .unwrap()
        .into_inner();
    let panel: Value = serde_json::from_slice(&panel.output_json).unwrap();
    // This full output is also parsed by the Console consumer contract test.
    let console_panel: Value =
        serde_json::from_str(include_str!("fixtures/servers-panel.json")).unwrap();
    assert_eq!(panel, console_panel);
    drop(client);
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!plugin_path.exists());
    child = tokio::process::Command::new(env!("CARGO_BIN_EXE_spitzli-lumaserv-compute"))
        .arg(&config_path)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut client = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            assert!(child.try_wait().unwrap().is_none());
            if let Ok(mut client) = spitzli_plugin_runtime::connect_unix(&plugin_path).await
                && client.health(wire::HealthRequest {}).await.is_ok()
            {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        client
            .invoke(invocation(&admissions, "servers.list", "123", json!({})))
            .await
            .is_ok()
    );
    for input in [
        json!({"page":0}),
        json!({"page":10001}),
        json!({"projectID":"456"}),
        json!({"binding":"other"}),
    ] {
        assert_eq!(
            client
                .invoke(invocation(&admissions, "servers.list", "123", input))
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
    }
    assert_eq!(
        client
            .invoke(invocation(&admissions, "servers.list", "456", json!({})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    mode.store(1, Ordering::SeqCst);
    for (action, input) in [
        ("servers.list", json!({})),
        ("servers.get", json!({"server":SERVER})),
    ] {
        assert_eq!(
            client
                .invoke(invocation(&admissions, action, "123", input))
                .await
                .unwrap_err()
                .code(),
            tonic::Code::PermissionDenied
        );
    }
    for state in [2, 3] {
        mode.store(state, Ordering::SeqCst);
        let error = client
            .invoke(invocation(&admissions, "servers.list", "123", json!({})))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unavailable);
        assert!(!error.message().contains("private provider"));
    }
    mode.store(0, Ordering::SeqCst);
    let mut altered = invocation(&admissions, "servers.list", "123", json!({}));
    altered.input_json = br#"{"page":2}"#.to_vec();
    assert_eq!(
        client.invoke(altered).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    mode.store(4, Ordering::SeqCst);
    let mut pending_client = client.clone();
    let pending_request = invocation(&admissions, "servers.list", "123", json!({}));
    let pending = tokio::spawn(async move { pending_client.invoke(pending_request).await });
    tokio::time::timeout(Duration::from_secs(3), reached.notified())
        .await
        .unwrap();
    active.store(false, Ordering::SeqCst);
    assert_eq!(
        pending.await.unwrap().unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client
            .invoke(invocation(&admissions, "servers.list", "123", json!({})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    assert!(client.health(wire::HealthRequest {}).await.is_ok());
    // Static admission probes remain possible after disable; they do not authorize work.
    assert!(client.describe(wire::DescribeRequest {}).await.is_ok());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        !tokio::process::Command::new(env!("CARGO_BIN_EXE_spitzli-lumaserv-compute"))
            .arg(&config_path)
            .output()
            .await
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn descriptor_never_advertises_unimplemented_provider_mutations() {
    let original: Value = serde_json::from_slice(include_bytes!("manifest.json")).unwrap();
    for mutate in ["write", "http", "binding", "configuration"] {
        let mut value = original.clone();
        match mutate {
            "write" => value["actions"][0]["intent"] = json!("write"),
            "http" => value["capabilities"] = json!(["plugin.http.read", "plugin.http.request"]),
            "binding" => value["httpBindings"] = json!(["compute", "arbitrary"]),
            "configuration" => {
                value["configuration"]["properties"]["destination"] = json!({"type":"string"})
            }
            _ => unreachable!(),
        }
        assert!(
            Manifest::parse(&serde_json::to_vec(&value).unwrap())
                .and_then(|manifest| spitzli_lumaserv_compute::validate_manifest(&manifest))
                .is_err()
        );
    }
}
