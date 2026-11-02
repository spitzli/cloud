#![cfg(feature = "local-provider-test")]
use axum::{
    Json,
    extract::{Request, State},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use webdock_github_repository::GithubProvider;
use webdock_repository_protocol::{Action, LeaseClaims, runtime::Handler};
#[derive(Clone)]
struct Fixture {
    origin: String,
    public: Vec<u8>,
    archive: Vec<u8>,
    mode: Arc<AtomicUsize>,
    downloads: Arc<AtomicUsize>,
}
async fn request(State(f): State<Fixture>, request: Request) -> Response {
    let uri = request.uri().clone();
    let path = uri.path();
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    if path.starts_with("/owner/repository/legacy.tar.gz/") {
        assert!(
            authorization.is_none(),
            "download never receives an installation credential"
        );
        assert_eq!(uri.query(), Some("token=download-fixture"));
        f.downloads.fetch_add(1, Ordering::SeqCst);
        return f.archive.into_response();
    }
    if path.starts_with("/app/") {
        let jwt = authorization.unwrap().strip_prefix("Bearer ").unwrap();
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.required_spec_claims.insert("iat".into());
        let claims = jsonwebtoken::decode::<Value>(
            jwt,
            &jsonwebtoken::DecodingKey::from_rsa_pem(&f.public).unwrap(),
            &validation,
        )
        .unwrap()
        .claims;
        assert_eq!(claims["iss"], 123);
        assert_eq!(
            claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
            600
        );
    } else {
        assert_eq!(authorization, Some("Bearer scoped-fixture-token"));
    }
    match path {
 "/app/installations/44"=>Json(json!({"id":44,"app_id":123,"suspended_at":null,"account":{"id":77},"permissions":{"contents":"read","checks":"write","metadata":"read"}})).into_response(),
 "/app/installations/44/access_tokens"=>{let bytes=axum::body::to_bytes(request.into_body(),1024).await.unwrap();let body:Value=serde_json::from_slice(&bytes).unwrap();assert_eq!(body["repository_ids"],json!([55]));assert_eq!(body["permissions"],json!({"contents":"read","metadata":"read"}));let mut permissions=body["permissions"].clone();if f.mode.load(Ordering::SeqCst)==1{permissions["administration"]=json!("write");}Json(json!({"token":"scoped-fixture-token","expires_at":(chrono::Utc::now()+chrono::Duration::minutes(5)).to_rfc3339(),"permissions":permissions,"repositories":[{"id":55}]})).into_response()},
 "/repositories/55"=>Json(json!({"id":if f.mode.load(Ordering::SeqCst)==2{56}else{55},"owner":{"login":"owner"},"name":"repository","default_branch":"main"})).into_response(),
 "/repos/owner/repository/branches/main"|"/repos/owner/repository/branches/feature%2Fbranch"=>{if f.mode.load(Ordering::SeqCst)==4{return " ".repeat(2_000_001).into_response();}Json(json!({"name":if path.ends_with("main"){"main"}else{"feature/branch"},"commit":{"sha":"a".repeat(40)}})).into_response()},
 _ if path==format!("/repos/owner/repository/commits/{}","a".repeat(40))=>Json(json!({"sha":"a".repeat(40)})).into_response(),
 _ if path==format!("/repos/owner/repository/tarball/{}","a".repeat(40))=>{let target=if f.mode.load(Ordering::SeqCst)==3{"http://127.0.0.1:9/forbidden".into()}else{format!("{}/owner/repository/legacy.tar.gz/{}?token=download-fixture",f.origin,"a".repeat(40))};(axum::http::StatusCode::FOUND,[("location",target)]).into_response()},
 _=>panic!("unexpected fixture request path {path}"),
 }
}
fn claims() -> LeaseClaims {
    LeaseClaims {
        v: 1,
        purpose: "repository".into(),
        audience: "webdock.repository.runtime.v1".into(),
        id: "00000000-0000-4000-8000-000000000001".into(),
        plugin_id: "github-repository".into(),
        generation: 1,
        action: Action::Resolve,
        customer_id: "1".into(),
        project_id: "2".into(),
        source_id: "3".into(),
        source_revision: 1,
        connection_generation: 1,
        grant_revision: 0,
        configuration_revision: 0,
        descriptor_sha256: "a".repeat(64),
        request_sha256: "a".repeat(64),
        configuration_sha256: "a".repeat(64),
        iat: chrono::Utc::now().timestamp(),
        exp: chrono::Utc::now().timestamp() + 60,
    }
}
#[tokio::test]
async fn exact_scoped_github_requests_reject_overbroad_tokens_and_cross_repository_downloads() {
    let keys = tempfile::tempdir().unwrap();
    let private = keys.path().join("private.pem");
    let public = keys.path().join("public.pem");
    assert!(
        std::process::Command::new("openssl")
            .args([
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:2048",
                "-out"
            ])
            .arg(&private)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        std::process::Command::new("openssl")
            .args(["pkey", "-in"])
            .arg(&private)
            .args(["-pubout", "-out"])
            .arg(&public)
            .output()
            .unwrap()
            .status
            .success()
    );
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(13);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, "wrapper/Dockerfile", &b"FROM scratch\n"[..])
        .unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar.into_inner().unwrap()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mode = Arc::new(AtomicUsize::new(0));
    let downloads = Arc::new(AtomicUsize::new(0));
    let fixture = Fixture {
        origin: format!("http://{address}"),
        public: std::fs::read(public).unwrap(),
        archive: gz.finish().unwrap(),
        mode: mode.clone(),
        downloads: downloads.clone(),
    };
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().fallback(request).with_state(fixture),
        )
        .await
        .unwrap()
    });
    struct Stop(tokio::task::JoinHandle<()>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _stop = Stop(server);
    let provider = GithubProvider::loopback_test(
        123,
        &std::fs::read(&private).unwrap(),
        &format!("http://{address}/"),
    )
    .unwrap();
    let claims = claims();
    for reference in ["main", "feature/branch", &"a".repeat(40)] {
        assert_eq!(
            provider
                .resolve(
                    &claims,
                    json!({"installationID":"44","repositoryID":"55","reference":reference}),
                    json!({})
                )
                .await
                .unwrap()["sha"],
            "a".repeat(40)
        );
    }
    let input = json!({"installationID":"44","repositoryID":"55","commit":"a".repeat(40)});
    let archive = provider
        .archive(&claims, input.clone(), json!({}))
        .await
        .unwrap();
    let mut tar = tar::Archive::new(archive.as_slice());
    assert_eq!(
        tar.entries()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .unwrap()
            .to_str(),
        Some("Dockerfile")
    );
    assert_eq!(downloads.load(Ordering::SeqCst), 1);
    for bad in [1, 2, 4] {
        mode.store(bad, Ordering::SeqCst);
        assert!(
            provider
                .resolve(
                    &claims,
                    json!({"installationID":"44","repositoryID":"55","reference":"main"}),
                    json!({})
                )
                .await
                .is_err()
        );
    }
    mode.store(3, Ordering::SeqCst);
    assert!(provider.archive(&claims, input, json!({})).await.is_err());
    assert_eq!(downloads.load(Ordering::SeqCst), 1);
    assert!(GithubProvider::loopback_test(123, b"bad key", "https://github.com/").is_err());
    mode.store(0, Ordering::SeqCst);
    installed_process(&private, &format!("http://{address}/")).await;
}

struct TestCore {
    issuer: ed25519_dalek::VerifyingKey,
    descriptor_sha: String,
    active: Arc<std::sync::atomic::AtomicBool>,
}
#[tonic::async_trait]
impl webdock_repository_protocol::wire::repository_authority_server::RepositoryAuthority
    for TestCore
{
    async fn authorize_peer(
        &self,
        request: tonic::Request<webdock_repository_protocol::wire::PeerProof>,
    ) -> Result<tonic::Response<webdock_repository_protocol::wire::AuthorityReply>, tonic::Status>
    {
        webdock_repository_protocol::runtime::peer(&request)?;
        let proof = request.into_inner();
        if !self.active.load(Ordering::SeqCst)
            || proof.plugin_id != "github-repository"
            || proof.generation != 1
            || proof.descriptor_sha256 != self.descriptor_sha
        {
            return Err(tonic::Status::permission_denied("fixture revoked"));
        }
        Ok(tonic::Response::new(
            webdock_repository_protocol::wire::AuthorityReply { active: true },
        ))
    }
    async fn authorize(
        &self,
        request: tonic::Request<webdock_repository_protocol::wire::LeaseProof>,
    ) -> Result<tonic::Response<webdock_repository_protocol::wire::AuthorityReply>, tonic::Status>
    {
        webdock_repository_protocol::runtime::peer(&request)?;
        if !self.active.load(Ordering::SeqCst) {
            return Err(tonic::Status::permission_denied("fixture revoked"));
        }
        webdock_repository_protocol::verify_lease(
            &request.into_inner().lease_token,
            &self.issuer,
            "github-repository",
            1,
            chrono::Utc::now().timestamp(),
        )
        .map_err(|_| tonic::Status::permission_denied("invalid fixture lease"))?;
        Ok(tonic::Response::new(
            webdock_repository_protocol::wire::AuthorityReply { active: true },
        ))
    }
}
async fn installed_process(private: &std::path::Path, origin: &str) {
    use std::os::unix::fs::PermissionsExt;
    use webdock_repository_protocol::{self as p, runtime, wire};
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let callback_path = directory.path().join("core.sock");
    let plugin_path = directory.path().join("plugin.sock");
    let config_path = directory.path().join("config.json");
    let issuer = ed25519_dalek::SigningKey::from_bytes(&[21; 32]);
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[22; 32]);
    let descriptor = p::Descriptor {
        v: 1,
        plugin_id: "github-repository".into(),
        version: "1.0.0".into(),
        manifest_sha256: "a".repeat(64),
        image: format!("registry.invalid/github@sha256:{}", "b".repeat(64)),
        protocol: "webdock.repository.v1".into(),
        signer_key_id: "fixture".into(),
    }
    .sign(&publisher)
    .unwrap();
    let descriptor_sha = p::sha256(descriptor.as_bytes());
    let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut socket = runtime::UnixSocket::bind(&callback_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let callback = wire::repository_authority_server::RepositoryAuthorityServer::new(TestCore {
        issuer: issuer.verifying_key(),
        descriptor_sha: descriptor_sha.clone(),
        active: active.clone(),
    });
    let server = tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(callback)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server = Server(server);
    std::fs::set_permissions(private, std::fs::Permissions::from_mode(0o600)).unwrap();
    // SAFETY: geteuid has no preconditions or pointer arguments.
    let uid = unsafe { libc::geteuid() };
    std::fs::write(&config_path,json!({"descriptor":descriptor,"publisherPublicKeyHex":hex::encode(publisher.verifying_key().as_bytes()),"issuerPublicKeyHex":hex::encode(issuer.verifying_key().as_bytes()),"generation":1,"appId":123,"appPrivateKey":private,"listen":{"kind":"unix","path":plugin_path},"core":{"kind":"unix","path":callback_path},"corePeers":[{"kind":"unix","uid":uid}],"testOrigin":origin}).to_string()).unwrap();
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(
        std::process::Command::new(env!("CARGO_BIN_EXE_webdock-github-repository"))
            .arg(&config_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !plugin_path.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "standalone provider exited before readiness"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut client = wire::repository_runtime_client::RepositoryRuntimeClient::new(
        runtime::connect_unix(&plugin_path).await.unwrap(),
    );
    let request = |action: Action, input: Value| {
        let input = serde_json::to_vec(&input).unwrap();
        let config = b"{}".to_vec();
        let mut claims = claims();
        claims.action = action;
        claims.descriptor_sha256 = descriptor_sha.clone();
        claims.request_sha256 = p::sha256(&input);
        claims.configuration_sha256 = p::sha256(&config);
        wire::LeaseRequest {
            lease_token: claims.sign(&issuer).unwrap(),
            input_json: input,
            configuration_json: config,
        }
    };
    let input = json!({"installationID":"44","repositoryID":"55","reference":"main"});
    let reply = client
        .resolve(request(Action::Resolve, input.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        serde_json::from_slice::<Value>(&reply.output_json).unwrap()["sha"],
        "a".repeat(40)
    );
    let stream = client
        .archive(request(
            Action::Archive,
            json!({"installationID":"44","repositoryID":"55","commit":"a".repeat(40)}),
        ))
        .await
        .unwrap()
        .into_inner();
    let archive = runtime::collect_archive(stream).await.unwrap();
    assert!(archive.len() > 512);
    active.store(false, Ordering::SeqCst);
    assert!(
        client
            .resolve(request(Action::Resolve, input))
            .await
            .is_err()
    );
}
