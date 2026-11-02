use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use webdock_repository_protocol::{
    runtime::{Authority, Handler, PeerIdentity, Runtime, UnixSocket},
    *,
};
struct Live(AtomicBool);
#[async_trait::async_trait]
impl Authority for Live {
    async fn authorize_peer(&self, peer: &PeerIdentity) -> Result<()> {
        if self.0.load(Ordering::SeqCst)
            && matches!(peer,PeerIdentity::Unix{uid}if *uid==rustix::process::geteuid().as_raw())
        {
            Ok(())
        } else {
            Err(PluginError::denied())
        }
    }
    async fn authorize_lease(&self, peer: &PeerIdentity, _: &LeaseClaims, _: &str) -> Result<()> {
        self.authorize_peer(peer).await
    }
}
struct FixtureHandler;
#[async_trait::async_trait]
impl Handler for FixtureHandler {
    async fn resolve(&self, _: &LeaseClaims, input: Value, _: Value) -> Result<Value> {
        if input["wait"] == true {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
        Ok(json!({"sha":"a".repeat(40)}))
    }
    async fn archive(&self, _: &LeaseClaims, _: Value, _: Value) -> Result<Vec<u8>> {
        Ok(vec![42; 1024 * 1024])
    }
}
fn request(key: &SigningKey, descriptor: &str, action: Action, input: Value) -> wire::LeaseRequest {
    let input = serde_json::to_vec(&input).unwrap();
    let config = b"{}".to_vec();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = LeaseClaims {
        v: 1,
        purpose: "repository".into(),
        audience: "webdock.repository.runtime.v1".into(),
        id: uuid::Uuid::new_v4().to_string(),
        plugin_id: "fixture-repository".into(),
        generation: 1,
        action,
        customer_id: "11".into(),
        project_id: "12".into(),
        source_id: "13".into(),
        source_revision: 1,
        connection_generation: 1,
        grant_revision: 1,
        configuration_revision: 0,
        descriptor_sha256: sha256(descriptor.as_bytes()),
        request_sha256: sha256(&input),
        configuration_sha256: sha256(&config),
        iat: now,
        exp: now + 60,
    };
    wire::LeaseRequest {
        lease_token: claims.sign(key).unwrap(),
        input_json: input,
        configuration_json: config,
    }
}
#[tokio::test]
async fn signed_private_runtime_streams_bounded_archives_and_loses_authority_live() {
    let publisher = SigningKey::from_bytes(&[30; 32]);
    let issuer = SigningKey::from_bytes(&[31; 32]);
    let descriptor = Descriptor {
        v: 1,
        plugin_id: "fixture-repository".into(),
        version: "1.0.0".into(),
        manifest_sha256: "a".repeat(64),
        image: format!("registry.invalid/plugin@sha256:{}", "b".repeat(64)),
        protocol: "webdock.repository.v1".into(),
        signer_key_id: "fixture-key".into(),
    }
    .sign(&publisher)
    .unwrap();
    let authority = Arc::new(Live(AtomicBool::new(true)));
    let runtime = Runtime::new(
        descriptor.clone(),
        &publisher.verifying_key(),
        1,
        issuer.verifying_key(),
        authority.clone(),
        Arc::new(FixtureHandler),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket_path = directory.path().join("repository.sock");
    let socket = UnixSocket::bind(&socket_path).unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(runtime.serve_unix(socket, async {
        let _ = shutdown.await;
    }));
    let channel = runtime::connect_unix(&socket_path).await.unwrap();
    let mut client = wire::repository_runtime_client::RepositoryRuntimeClient::new(channel);
    let response = client
        .resolve(request(&issuer, &descriptor, Action::Resolve, json!({})))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        serde_json::from_slice::<Value>(&response.output_json).unwrap()["sha"],
        "a".repeat(40)
    );
    let mut tampered = request(&issuer, &descriptor, Action::Resolve, json!({}));
    tampered.input_json = b"{\"altered\":true}".to_vec();
    assert_eq!(
        client.resolve(tampered).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    let stream = client
        .archive(request(&issuer, &descriptor, Action::Archive, json!({})))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        runtime::collect_archive(stream).await.unwrap(),
        vec![42; 1024 * 1024]
    );
    {
        let waiting = client.resolve(request(
            &issuer,
            &descriptor,
            Action::Resolve,
            json!({"wait":true}),
        ));
        tokio::pin!(waiting);
        tokio::select! {_=&mut waiting=>panic!("fixture should still be pending"),_=tokio::time::sleep(std::time::Duration::from_millis(50))=>{}}
        authority.0.store(false, Ordering::SeqCst);
        assert_eq!(
            waiting.await.unwrap_err().code(),
            tonic::Code::PermissionDenied
        );
    }
    assert!(client.describe(wire::DescribeRequest {}).await.is_err());
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}
