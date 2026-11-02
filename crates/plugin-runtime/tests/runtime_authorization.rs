use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tonic::{Request, Response, Status};
use webdock_plugin_protocol::ErrorCode;
use webdock_plugin_runtime::{
    CorePeer, HostClient, InvocationAuthority, PeerIdentity, RemoteAuthority, UnixSocket, wire,
};

#[derive(Clone, Default)]
struct Host {
    mode: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
    client_certificate: Option<Vec<u8>>,
}
#[tonic::async_trait]
impl wire::plugin_host_server::PluginHost for Host {
    async fn authorize_peer(
        &self,
        request: Request<wire::HostPeerRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        if let Some(expected) = &self.client_certificate {
            let tls =
                request
                    .extensions()
                    .get::<tonic::transport::server::TlsConnectInfo<
                        tonic::transport::server::TcpConnectInfo,
                    >>()
                    .expect("mutual TLS callback");
            assert_eq!(tls.peer_certs().unwrap()[0].as_ref(), expected);
        } else {
            assert!(
                request
                    .extensions()
                    .get::<tonic::transport::server::UdsConnectInfo>()
                    .is_some()
            );
        }
        let request = request.into_inner();
        assert_eq!(
            (request.plugin_id.as_str(), request.generation),
            ("mail", 7)
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.mode.load(Ordering::SeqCst) {
            0 => Ok(Response::new(wire::HostAuthorityResponse { active: false })),
            1 => Ok(Response::new(wire::HostAuthorityResponse { active: true })),
            2 => Err(Status::unavailable("private fixture diagnostics")),
            3 => Err(Status::permission_denied("private fixture diagnostics")),
            _ => std::future::pending().await,
        }
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
    async fn call(
        &self,
        _: Request<wire::HostCallRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
    async fn workloads(
        &self,
        _: Request<wire::HostWorkloadRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
}
struct Fixture {
    task: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
    client: HostClient,
    host: Host,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = directory.path().join("host.sock");
        let mut socket = UnixSocket::bind(&path).unwrap();
        let incoming = socket.incoming().unwrap();
        let host = Host::default();
        let service = wire::plugin_host_server::PluginHostServer::new(host.clone());
        let task = tokio::spawn(async move {
            let _socket = socket;
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        let client = HostClient::connect_unix(path).await.unwrap();
        Self {
            task,
            _directory: directory,
            client,
            host,
        }
    }
}

#[tokio::test]
async fn current_runtime_authorization_preserves_scope_and_denies_inactive_or_failed_callbacks() {
    let f = Fixture::new().await;
    for (mode, code) in [
        (0, ErrorCode::PermissionDenied),
        (2, ErrorCode::Unavailable),
        (3, ErrorCode::PermissionDenied),
    ] {
        f.host.mode.store(mode, Ordering::SeqCst);
        let error = f.client.authorize_runtime("mail", 7).await.unwrap_err();
        assert_eq!(error.code, code);
        assert!(!error.message.contains("private fixture"));
    }
    f.host.mode.store(1, Ordering::SeqCst);
    f.client.authorize_runtime("mail", 7).await.unwrap();
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 4);
    let uid = rustix::process::geteuid().as_raw();
    let authority = RemoteAuthority::new(
        f.client.clone(),
        "mail".into(),
        7,
        vec![CorePeer::Unix { uid }],
    )
    .unwrap();
    authority
        .authorize_peer(&PeerIdentity::Unix { uid })
        .await
        .unwrap();
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 5);
    assert!(
        authority
            .authorize_peer(&PeerIdentity::Unix { uid: uid + 1 })
            .await
            .is_err()
    );
    assert_eq!(f.host.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn stalled_runtime_authorization_has_its_own_bounded_deadline() {
    let f = Fixture::new().await;
    f.host.mode.store(4, Ordering::SeqCst);
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        f.client.authorize_runtime("mail", 7),
    )
    .await
    .expect("authorization must not inherit the 55s channel deadline")
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::DeadlineExceeded);
}

#[tokio::test]
async fn lazy_unix_callback_recovers_after_initial_absence_and_rechecks_private_socket() {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("late.sock");
    assert!(
        webdock_plugin_runtime::unix_channel(&path).await.is_err(),
        "eager API must remain eager"
    );
    let client = HostClient::new(webdock_plugin_runtime::unix_channel_lazy(&path).unwrap());
    assert!(client.authorize_runtime("mail", 7).await.is_err());
    let mut socket = UnixSocket::bind(&path).unwrap();
    let incoming = socket.incoming().unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    let host = Host::default();
    host.mode.store(1, Ordering::SeqCst);
    let service = wire::plugin_host_server::PluginHostServer::new(host.clone());
    let task = tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    assert!(client.authorize_runtime("mail", 7).await.is_err());
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.authorize_runtime("mail", 7).await.is_err() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn lazy_pinned_callback_recovers_from_initial_offline_without_weakening_tls() {
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose,
    };
    use sha2::{Digest, Sha256};
    use std::time::Duration;
    use tonic::transport::{Certificate, Identity, ServerTlsConfig};
    use webdock_plugin_runtime::PinnedMutualTls;
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let key = KeyPair::generate().unwrap();
    let ca = params.self_signed(&key).unwrap();
    let issuer = rcgen::Issuer::new(params, key);
    let leaf = |server| {
        let mut params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        params.extended_key_usages = vec![if server {
            ExtendedKeyUsagePurpose::ServerAuth
        } else {
            ExtendedKeyUsagePurpose::ClientAuth
        }];
        let key = KeyPair::generate().unwrap();
        (params.signed_by(&key, &issuer).unwrap(), key)
    };
    let (server, server_key) = leaf(true);
    let (client_cert, client_key) = leaf(false);
    let material = || PinnedMutualTls {
        certificate: client_cert.pem().into_bytes(),
        private_key: client_key.serialize_pem().into_bytes(),
        authority: ca.pem().into_bytes(),
        server_certificate_sha256: hex::encode(Sha256::digest(server.der())),
    };
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reserved.local_addr().unwrap();
    let origin = format!("https://localhost:{}", address.port());
    drop(reserved);
    assert!(
        webdock_plugin_runtime::pinned_mtls_channel(origin.clone(), "localhost".into(), material())
            .await
            .is_err()
    );
    let channel = webdock_plugin_runtime::pinned_mtls_channel_lazy(
        origin.clone(),
        "localhost".into(),
        material(),
    )
    .await
    .unwrap();
    let client = HostClient::new(channel);
    assert!(client.authorize_runtime("mail", 7).await.is_err());
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let host = Host {
        client_certificate: Some(client_cert.der().to_vec()),
        ..Host::default()
    };
    host.mode.store(1, Ordering::SeqCst);
    let tls = ServerTlsConfig::new()
        .identity(Identity::from_pem(server.pem(), server_key.serialize_pem()))
        .client_ca_root(Certificate::from_pem(ca.pem()))
        .client_auth_optional(false);
    let service = wire::plugin_host_server::PluginHostServer::new(host.clone());
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(service)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.authorize_runtime("mail", 7).await.is_err() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let permitted = host.calls.load(Ordering::SeqCst);
    for problem in ["pin", "name", "ca"] {
        let mut tls = material();
        let domain = if problem == "name" {
            "other.invalid"
        } else {
            "localhost"
        };
        if problem == "pin" {
            tls.server_certificate_sha256 = "00".repeat(32);
        }
        if problem == "ca" {
            let (foreign, _) = leaf(true);
            tls.authority = foreign.pem().into_bytes();
        }
        let channel =
            webdock_plugin_runtime::pinned_mtls_channel_lazy(origin.clone(), domain.into(), tls)
                .await
                .unwrap();
        assert!(
            HostClient::new(channel)
                .authorize_runtime("mail", 7)
                .await
                .is_err()
        );
        assert_eq!(host.calls.load(Ordering::SeqCst), permitted);
    }
    let mut malformed = material();
    malformed.authority = b"invalid trust".to_vec();
    assert!(
        webdock_plugin_runtime::pinned_mtls_channel_lazy(origin, "localhost".into(), malformed)
            .await
            .is_err()
    );
    task.abort();
    let _ = task.await;
}
