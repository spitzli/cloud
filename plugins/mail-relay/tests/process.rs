#![cfg(feature = "local-provider-test")]
mod common;
use ed25519_dalek::SigningKey;
use serde_json::json;
use std::{
    collections::HashSet,
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tonic::{Request, Response, Status};
use webdock_mail_protocol::{
    wire::{
        mail_authority_server::{MailAuthority, MailAuthorityServer},
        mail_runtime_client::MailRuntimeClient,
    },
    *,
};
use webdock_plugin_runtime::{PeerIdentity, UnixSocket, authenticated_peer};
#[derive(Clone)]
struct Core {
    key: ed25519_dalek::VerifyingKey,
    active: Arc<AtomicBool>,
    attempts: Arc<Mutex<HashSet<String>>>,
    configuration: Arc<Mutex<serde_json::Value>>,
}
impl Core {
    fn claims(&self, request: &Request<wire::LeaseProof>) -> Result<LeaseClaims> {
        if !matches!(authenticated_peer(request), Ok(PeerIdentity::Unix { .. }))
            || !self.active.load(Ordering::SeqCst)
        {
            return Err(Denied);
        }
        verify_lease(
            &request.get_ref().lease_token,
            &self.key,
            "relay-fixture",
            1,
            now(),
        )
    }
}
#[tonic::async_trait]
impl MailAuthority for Core {
    async fn guard(
        &self,
        request: Request<wire::LeaseProof>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        Ok(Response::new(wire::GuardReply {
            active: self.claims(&request).is_ok(),
        }))
    }
    async fn begin_attempt(
        &self,
        request: Request<wire::LeaseProof>,
    ) -> std::result::Result<Response<wire::AttemptReply>, Status> {
        let claims = self
            .claims(&request)
            .map_err(|_| Status::permission_denied("denied"))?;
        let admitted = self.attempts.lock().unwrap().insert(claims.id);
        Ok(Response::new(wire::AttemptReply {
            admitted,
            private_configuration_json: if admitted {
                serde_json::to_vec(&*self.configuration.lock().unwrap()).unwrap()
            } else {
                vec![]
            },
        }))
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn frames(key: &SigningKey) -> Vec<wire::SendFrame> {
    let metadata=serde_json::to_vec(&json!({"sender":{"email":"sender@example.test"},"to":["target@example.test"],"cc":[],"bcc":[],"subject":"Fixture"})).unwrap();
    let raw = b"From: <sender@example.test>\r\nSubject: Fixture\r\n\r\nBody";
    let claims = LeaseClaims {
        v: 1,
        purpose: "mail-send".into(),
        audience: "webdock.mail.runtime.v1".into(),
        id: uuid::Uuid::new_v4().to_string(),
        plugin_id: "relay-fixture".into(),
        generation: 1,
        customer_id: "123".into(),
        job_id: "456".into(),
        source_id: "key:789".into(),
        transport_revision: 1,
        grant_revision: 1,
        configuration_revision: 0,
        descriptor_sha256: "a".repeat(64),
        binding_sha256: "b".repeat(64),
        worker_id: "mail-worker".into(),
        worker_generation: 1,
        worker_credential_sha256: "c".repeat(64),
        metadata_sha256: sha256(&metadata),
        mime_sha256: sha256(raw),
        mime_bytes: raw.len() as u64,
        iat: now(),
        exp: now() + 60,
    };
    vec![
        wire::SendFrame {
            offset: 0,
            data: vec![],
            header: Some(wire::SendHeader {
                lease_token: claims.sign(key).unwrap(),
                metadata_json: metadata,
            }),
        },
        wire::SendFrame {
            offset: 0,
            data: raw.to_vec(),
            header: None,
        },
    ]
}
#[tokio::test]
async fn actual_provider_process_uses_confidential_live_callback_and_one_external_attempt() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let key = SigningKey::from_bytes(&[76; 32]);
    let core = Core {
        key: key.verifying_key(),
        active: Arc::new(AtomicBool::new(true)),
        attempts: Arc::new(Mutex::new(HashSet::new())),
        configuration: Arc::new(Mutex::new(serde_json::Value::Null)),
    };
    let active = core.active.clone();
    let configuration = core.configuration.clone();
    let core_path = dir.path().join("core.sock");
    let mut socket = UnixSocket::bind(&core_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let authority = tokio::spawn(async move {
        let result = tonic::transport::Server::builder()
            .add_service(MailAuthorityServer::new(core))
            .serve_with_incoming_shutdown(incoming, async {
                let _ = shutdown.await;
            })
            .await;
        drop(socket);
        result
    });
    let sends = Arc::new(AtomicUsize::new(0));
    let count = sends.clone();
    let upstream = common::Fixture::start(move |headers, body| {
        assert!(headers.contains("consumersecret: private-fixture-secret\r\n"));
        assert_eq!(body["reference_id"], "456");
        count.fetch_add(1, Ordering::SeqCst);
        (200, "{\"mid\":\"98765432109876543210\"}".into())
    })
    .await;
    let origin = upstream.origin.clone();
    *configuration.lock().unwrap() =
        upstream.configuration("private-fixture-key", "private-fixture-secret");
    let runtime_path = dir.path().join("relay.sock");
    let config = dir.path().join("provider.json");
    std::fs::write(&config,serde_json::to_vec(&json!({"pluginID":"relay-fixture","version":"0.1.0","generation":1,"descriptorSha256":"a".repeat(64),"issuerPublicKeyHex":hex::encode(key.verifying_key().as_bytes()),"worker":{"kind":"unix","uid":dir.path().metadata().unwrap().uid()},"corePeer":{"kind":"unix","uid":dir.path().metadata().unwrap().uid()},"listen":{"kind":"unix","path":runtime_path},"core":{"kind":"unix","path":core_path},"testOrigin":origin})).unwrap()).unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_webdock-mail-relay"))
        .arg(&config)
        .env_clear()
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let channel = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "provider exited during startup"
            );
            if let Ok(channel) = webdock_plugin_runtime::unix_channel(&runtime_path).await {
                break channel;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut client = MailRuntimeClient::new(channel);
    let descriptor = client
        .describe(wire::DescribeRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(descriptor.plugin_id, "relay-fixture");
    assert_eq!(descriptor.version, "0.1.0");
    assert_eq!(descriptor.protocol, "webdock.mail.v1");
    assert_eq!(descriptor.descriptor_sha256, "a".repeat(64));
    let packet = frames(&key);
    assert_eq!(
        client
            .submit(tokio_stream::iter(packet.clone()))
            .await
            .unwrap()
            .into_inner()
            .provider_id,
        "98765432109876543210"
    );
    assert!(client.submit(tokio_stream::iter(packet)).await.is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    active.store(false, Ordering::SeqCst);
    assert!(
        client
            .submit(tokio_stream::iter(frames(&key)))
            .await
            .is_err()
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert!(client.describe(wire::DescribeRequest {}).await.is_ok());
    drop(client);
    // SAFETY: the child is still owned and kill takes no pointers.
    let signal_result = unsafe { libc::kill(child.id().unwrap() as i32, libc::SIGTERM) };
    assert_eq!(signal_result, 0);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!runtime_path.exists());
    stop.send(()).unwrap();
    authority.await.unwrap().unwrap();
    drop(upstream);
    // Configuration must be private even if the process has otherwise valid channel settings.
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();
    let rejected = tokio::process::Command::new(env!("CARGO_BIN_EXE_webdock-mail-relay"))
        .arg(config)
        .env_clear()
        .output()
        .await
        .unwrap();
    assert!(!rejected.status.success());
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("private-fixture-secret"));
}
