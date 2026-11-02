use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use webdock_plugin_protocol::{InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::{
    ActionHandler, InvocationAuthority, PeerIdentity, Runtime, UnixSocket, connect_unix, wire,
};

struct Echo;
#[tonic::async_trait]
impl ActionHandler for Echo {
    async fn invoke(
        &self,
        _: &InvocationClaims,
        _: &str,
        input: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        if input["wait"] == true {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Ok(input)
    }
}
struct Grant(AtomicBool);
#[tonic::async_trait]
impl InvocationAuthority for Grant {
    async fn authorize_request(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
        _input: &[u8],
        _configuration: &[u8],
    ) -> Result<(), PluginError> {
        self.authorize_invocation(peer, claims).await
    }

    async fn authorize_peer(&self, peer: &PeerIdentity) -> Result<(), PluginError> {
        if matches!(peer, PeerIdentity::Unix { uid } if *uid == rustix::process::geteuid().as_raw())
        {
            Ok(())
        } else {
            Err(PluginError::denied())
        }
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
}
fn manifest() -> Manifest {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    value["jobs"] = json!([]);
    Manifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
}
fn request(generation: u64, input: Value) -> wire::InvokeRequest {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({"v":1,"id":"123e4567-e89b-42d3-a456-426614174000","pluginID":"project-notes","generation":generation,"action":"notes.read","projectID":"123","customerID":"101","iat":now,"exp":now+60});
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let token = format!(
        "{body}.{}",
        URL_SAFE_NO_PAD.encode(
            SigningKey::from_bytes(&[7; 32])
                .sign(body.as_bytes())
                .to_bytes()
        )
    );
    wire::InvokeRequest {
        invocation_token: token,
        action: "notes.read".into(),
        input_json: serde_json::to_vec(&input).unwrap(),
        configuration_json: b"{}".to_vec(),
    }
}
#[tokio::test]
async fn unix_calls_bind_generation_and_recheck_revocation_during_work() {
    let directory = tempfile::Builder::new()
        .permissions({
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o700)
        })
        .tempdir()
        .unwrap();
    let path = directory.path().join("plugin.sock");
    let socket = UnixSocket::bind(&path).unwrap();
    let authority = Arc::new(Grant(AtomicBool::new(true)));
    let runtime = Runtime::new(
        manifest(),
        1,
        SigningKey::from_bytes(&[7; 32]).verifying_key(),
        authority.clone(),
        Arc::new(Echo),
    )
    .unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(runtime.serve_unix(socket, async {
        let _ = stop_rx.await;
    }));
    let mut client = connect_unix(&path).await.unwrap();
    let health = client
        .health(wire::HealthRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(health.plugin_id, "project-notes");
    let response = client
        .invoke(request(1, json!({"message":"hello"})))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        serde_json::from_slice::<Value>(&response.output_json).unwrap(),
        json!({"message":"hello"})
    );
    assert_eq!(
        client
            .invoke(request(2, json!({})))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    let pending =
        tokio::spawn(async move { client.invoke(request(1, json!({"wait":true}))).await });
    tokio::time::sleep(Duration::from_millis(80)).await;
    authority.0.store(false, Ordering::SeqCst);
    assert_eq!(
        pending.await.unwrap().unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}
#[tokio::test]
async fn missing_authority_denies_even_authenticated_local_peer() {
    struct NoAuthority;
    #[tonic::async_trait]
    impl InvocationAuthority for NoAuthority {}
    let directory = tempfile::Builder::new()
        .permissions({
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o700)
        })
        .tempdir()
        .unwrap();
    let path = directory.path().join("plugin.sock");
    let socket = UnixSocket::bind(&path).unwrap();
    let runtime = Runtime::new(
        manifest(),
        1,
        SigningKey::from_bytes(&[7; 32]).verifying_key(),
        Arc::new(NoAuthority),
        Arc::new(Echo),
    )
    .unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(runtime.serve_unix(socket, async {
        let _ = rx.await;
    }));
    let mut client = connect_unix(&path).await.unwrap();
    assert_eq!(
        client
            .health(wire::HealthRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}
#[test]
fn socket_refuses_shared_directory_and_existing_paths() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions({
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o700)
        })
        .tempdir()
        .unwrap();
    let path = directory.path().join("plugin.sock");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(UnixSocket::bind(&path).is_err());
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(&path, "retained").unwrap();
    assert!(UnixSocket::bind(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "retained");
}

struct CertificateGrant {
    leaf: Vec<u8>,
    allowed: AtomicBool,
}
#[tonic::async_trait]
impl InvocationAuthority for CertificateGrant {
    async fn authorize_request(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
        _input: &[u8],
        _configuration: &[u8],
    ) -> Result<(), PluginError> {
        self.authorize_invocation(peer, claims).await
    }

    async fn authorize_peer(&self, peer: &PeerIdentity) -> Result<(), PluginError> {
        if self.allowed.load(Ordering::SeqCst)
            && matches!(peer,PeerIdentity::MutualTls{certificates} if certificates.first()==Some(&self.leaf))
        {
            Ok(())
        } else {
            Err(PluginError::denied())
        }
    }
    async fn authorize_invocation(
        &self,
        peer: &PeerIdentity,
        _: &InvocationClaims,
    ) -> Result<(), PluginError> {
        self.authorize_peer(peer).await
    }
}
fn ca() -> (rcgen::Certificate, rcgen::Issuer<'static, rcgen::KeyPair>) {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let key = rcgen::KeyPair::generate().unwrap();
    let certificate = params.self_signed(&key).unwrap();
    (certificate, rcgen::Issuer::new(params, key))
}
fn certificate(
    issuer: &rcgen::Issuer<'static, rcgen::KeyPair>,
    server: bool,
    expired: bool,
) -> (rcgen::Certificate, rcgen::KeyPair) {
    let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
    params.extended_key_usages = vec![if server {
        rcgen::ExtendedKeyUsagePurpose::ServerAuth
    } else {
        rcgen::ExtendedKeyUsagePurpose::ClientAuth
    }];
    if expired {
        params.not_before = rcgen::date_time_ymd(2000, 1, 1);
        params.not_after = rcgen::date_time_ymd(2001, 1, 1);
    }
    let key = rcgen::KeyPair::generate().unwrap();
    (params.signed_by(&key, issuer).unwrap(), key)
}
#[tokio::test]
async fn mtls_rejects_untrusted_expired_and_revoked_clients() {
    use tonic::transport::{Certificate, ClientTlsConfig, Endpoint, Identity};
    use webdock_plugin_runtime::{MutualTls, connect_mtls};
    let (root, issuer) = ca();
    let (server_cert, server_key) = certificate(&issuer, true, false);
    let (client_cert, client_key) = certificate(&issuer, false, false);
    let authority = Arc::new(CertificateGrant {
        leaf: client_cert.der().to_vec(),
        allowed: AtomicBool::new(true),
    });
    let runtime = Runtime::new(
        manifest(),
        1,
        SigningKey::from_bytes(&[7; 32]).verifying_key(),
        authority.clone(),
        Arc::new(Echo),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tls = MutualTls {
        identity: Identity::from_pem(server_cert.pem(), server_key.serialize_pem()),
        peer_ca: Certificate::from_pem(root.pem()),
    };
    let server = tokio::spawn(runtime.serve_mtls(listener, tls, async {
        let _ = rx.await;
    }));
    let mut client = connect_mtls(
        origin.clone(),
        "localhost".into(),
        MutualTls {
            identity: Identity::from_pem(client_cert.pem(), client_key.serialize_pem()),
            peer_ca: Certificate::from_pem(root.pem()),
        },
    )
    .await
    .unwrap();
    assert!(
        client
            .invoke(request(1, json!({"message":"TLS"})))
            .await
            .is_ok()
    );
    use sha2::{Digest, Sha256};
    let pinned = || webdock_plugin_runtime::PinnedMutualTls {
        certificate: client_cert.pem().into_bytes(),
        private_key: client_key.serialize_pem().into_bytes(),
        authority: root.pem().into_bytes(),
        server_certificate_sha256: hex::encode(Sha256::digest(server_cert.der())),
    };
    let mut enrolled =
        webdock_plugin_runtime::connect_pinned_mtls(origin.clone(), "localhost".into(), pinned())
            .await
            .unwrap();
    assert!(
        enrolled
            .invoke(request(1, json!({"message":"pinned TLS"})))
            .await
            .is_ok()
    );
    let mut wrong_pin = pinned();
    wrong_pin.server_certificate_sha256 = "0".repeat(64);
    assert!(
        webdock_plugin_runtime::connect_pinned_mtls(origin.clone(), "localhost".into(), wrong_pin)
            .await
            .is_err()
    );
    assert!(
        webdock_plugin_runtime::connect_pinned_mtls(
            origin.clone(),
            "wrong.invalid".into(),
            pinned()
        )
        .await
        .is_err()
    );
    let (_, untrusted_issuer) = ca();
    for (certificate, key) in [
        certificate(&untrusted_issuer, false, false),
        certificate(&issuer, false, true),
    ] {
        if let Ok(mut denied) = connect_mtls(
            origin.clone(),
            "localhost".into(),
            MutualTls {
                identity: Identity::from_pem(certificate.pem(), key.serialize_pem()),
                peer_ca: Certificate::from_pem(root.pem()),
            },
        )
        .await
        {
            assert!(denied.health(wire::HealthRequest {}).await.is_err());
        }
    }
    // An authenticated server connection without a client certificate must also fail.
    if let Ok(channel) = Endpoint::from_shared(origin)
        .unwrap()
        .tls_config(
            ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(root.pem()))
                .domain_name("localhost"),
        )
        .unwrap()
        .connect()
        .await
    {
        let mut denied = wire::plugin_runtime_client::PluginRuntimeClient::new(channel);
        assert!(denied.health(wire::HealthRequest {}).await.is_err());
    }
    authority.allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        client
            .health(wire::HealthRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn malformed_scope_and_oversized_input_never_reach_the_handler() {
    struct Never;
    #[tonic::async_trait]
    impl ActionHandler for Never {
        async fn invoke(
            &self,
            _: &InvocationClaims,
            _: &str,
            _: Value,
            _: Value,
        ) -> Result<Value, PluginError> {
            panic!("invalid input executed");
        }
    }
    let directory = tempfile::Builder::new()
        .permissions({
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o700)
        })
        .tempdir()
        .unwrap();
    let path = directory.path().join("plugin.sock");
    let socket = UnixSocket::bind(&path).unwrap();
    let runtime = Runtime::new(
        manifest(),
        1,
        SigningKey::from_bytes(&[7; 32]).verifying_key(),
        Arc::new(Grant(AtomicBool::new(true))),
        Arc::new(Never),
    )
    .unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(runtime.serve_unix(socket, async {
        let _ = rx.await;
    }));
    let mut client = connect_unix(&path).await.unwrap();
    let mut wrong_action = request(1, json!({}));
    wrong_action.action = "notes.write".into();
    assert_eq!(
        client.invoke(wrong_action).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    let oversized = request(1, json!("x".repeat(32769)));
    assert_eq!(
        client.invoke(oversized).await.unwrap_err().code(),
        tonic::Code::ResourceExhausted
    );
    let mut configuration = request(1, json!({}));
    configuration.configuration_json = br#"{"unexpected":"secret"}"#.to_vec();
    assert_eq!(
        client.invoke(configuration).await.unwrap_err().code(),
        tonic::Code::InvalidArgument
    );
    let mut unsigned = request(1, json!({}));
    unsigned.invocation_token = "broken".into();
    assert_eq!(
        client.invoke(unsigned).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    let watch = wire::WatchOperationRequest {
        invocation_token: request(1, json!({})).invocation_token,
        operation_id: "123".into(),
        after_cursor: String::new(),
    };
    assert_eq!(
        client.watch_operation(watch).await.unwrap_err().code(),
        tonic::Code::Unimplemented
    );
    tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn caller_deadline_drops_inflight_action() {
    struct DropMarker(Arc<AtomicBool>);
    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    struct Slow(Arc<AtomicBool>);
    #[tonic::async_trait]
    impl ActionHandler for Slow {
        async fn invoke(
            &self,
            _: &InvocationClaims,
            _: &str,
            _: Value,
            _: Value,
        ) -> Result<Value, PluginError> {
            let _marker = DropMarker(self.0.clone());
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
    let directory = tempfile::Builder::new()
        .permissions({
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o700)
        })
        .tempdir()
        .unwrap();
    let path = directory.path().join("plugin.sock");
    let socket = UnixSocket::bind(&path).unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let runtime = Runtime::new(
        manifest(),
        1,
        SigningKey::from_bytes(&[7; 32]).verifying_key(),
        Arc::new(Grant(AtomicBool::new(true))),
        Arc::new(Slow(dropped.clone())),
    )
    .unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(runtime.serve_unix(socket, async {
        let _ = rx.await;
    }));
    let mut client = connect_unix(&path).await.unwrap();
    let mut request = tonic::Request::new(request(1, json!({})));
    request.set_timeout(Duration::from_millis(100));
    assert!(client.invoke(request).await.is_err());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}
