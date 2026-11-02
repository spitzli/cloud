use super::*;
use crate::mail::client::Client;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webdock_agent_protocol::mail::{MailLease, MailResources};
struct Authority {
    allowed: AtomicBool,
    revoke_handoff: AtomicBool,
    handed_off: AtomicBool,
    checkpoints: Mutex<Vec<ConfigurationCheckpoint>>,
}
#[async_trait]
impl ConfigurationAuthority for Authority {
    async fn guard(&self, claim: &ConfigurationClaim) -> Result<()> {
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        if !self.allowed.load(Ordering::SeqCst) {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(
        &self,
        claim: &ConfigurationClaim,
        checkpoint: ConfigurationCheckpoint,
    ) -> Result<()> {
        self.guard(claim).await?;
        let handoff = matches!(
            checkpoint.phase,
            ConfigurationCheckpointPhase::Handoff { .. }
        );
        self.checkpoints.lock().unwrap().push(checkpoint);
        if handoff {
            self.handed_off.store(true, Ordering::SeqCst);
            if self.revoke_handoff.load(Ordering::SeqCst) {
                self.allowed.store(false, Ordering::SeqCst);
            }
        }
        Ok(())
    }
}
struct Runtime(std::net::SocketAddr);
#[async_trait]
impl Inspector for Runtime {
    async fn inspect_configuration(
        &self,
        _: &ConfigurationClaim,
        expected: Option<&str>,
        _: &dyn ConfigurationAuthority,
    ) -> Result<Inspection> {
        if expected.is_some_and(|expected| expected != "normal-fixture") {
            return Err(ExecutionError::Ownership);
        }
        Ok(Inspection {
            runtime_id: "normal-fixture".into(),
            client: Client::local_fixture(self.0)?,
        })
    }
}
struct State {
    domains: Mutex<Vec<Value>>,
    creates: AtomicUsize,
    drop_reply: AtomicBool,
    revoke_write: AtomicBool,
    authority: Arc<Authority>,
}
impl State {
    fn call(&self, method: &str, arguments: &Value) -> (Value, bool) {
        match method {
            "x:Domain/get" => (json!({"list":self.domains.lock().unwrap().clone()}), false),
            "x:Domain/set" => {
                assert!(
                    self.authority.handed_off.load(Ordering::SeqCst),
                    "handoff must be accepted before an external mutation"
                );
                assert!(
                    arguments.get("update").is_none(),
                    "fixture already has DKIM, no regeneration is allowed"
                );
                let mut domain = arguments["create"]["resource"].clone();
                self.creates.fetch_add(1, Ordering::SeqCst);
                domain["id"] = json!("owned-domain");
                self.domains.lock().unwrap().push(domain);
                if self.revoke_write.load(Ordering::SeqCst) {
                    self.authority.allowed.store(false, Ordering::SeqCst);
                }
                (
                    json!({"created":{"resource":{"id":"owned-domain"}}}),
                    self.drop_reply.swap(false, Ordering::SeqCst),
                )
            }
            "x:DkimSignature/get" => (
                json!({"list":[{"id":"key","domainId":"owned-domain","selector":"webdock-rsa-20261020","publicKey":include_str!("../../../../mail/tests/configuration_public_key.txt").trim(),"stage":"active"}]}),
                false,
            ),
            _ => panic!("configuration must not invoke {method}"),
        }
    }
}
struct Fixture {
    runtime: Runtime,
    state: Arc<State>,
    server: tokio::task::JoinHandle<()>,
    claim: ConfigurationClaim,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn open() -> Self {
        let authority = Arc::new(Authority {
            allowed: AtomicBool::new(true),
            revoke_handoff: AtomicBool::new(false),
            handed_off: AtomicBool::new(false),
            checkpoints: Mutex::new(Vec::new()),
        });
        let state = Arc::new(State {
            domains: Mutex::new(vec![
                json!({"id":"internal","name":"mail-123.example.test","description":null,"dkimManagement":{"@type":"Manual"}}),
            ]),
            creates: AtomicUsize::new(0),
            drop_reply: AtomicBool::new(false),
            revoke_write: AtomicBool::new(false),
            authority,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime = Runtime(listener.local_addr().unwrap());
        let engine = state.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break None;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    assert!(bytes.len() <= 1048576);
                    if let Some(position) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break Some(position + 4);
                    }
                };
                let Some(header_end) = header_end else {
                    continue;
                };
                let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse().unwrap())
                    })
                    .unwrap();
                assert!(length <= 1048576);
                while bytes.len() < header_end + length {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let request: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                let call = &request["methodCalls"][0];
                let method = call[0].as_str().unwrap();
                let (response, drop_reply) = engine.call(method, &call[1]);
                if drop_reply {
                    continue;
                }
                let body =
                    serde_json::to_vec(&json!({"methodResponses":[[method,response,call[2]]]}))
                        .unwrap();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
        });
        let material = ConfigurationMaterial {
            administrator: ConfigurationCredential {
                username: "webdock-admin@mail-123.example.test".into(),
                password: "Synthetic-Private-Fixture-Only".into(),
            },
            identities: BTreeMap::new(),
        };
        let claim = ConfigurationClaim {
            spec: ConfigurationSpec {
                version: 1,
                operation_id: "456".into(),
                customer_id: "123".into(),
                cluster_id: "789".into(),
                revision: "1".into(),
                service_revision: "1".into(),
                agent_generation: 1,
                mode: ConfigurationMode::Apply,
                target: ConfigurationTarget {
                    instance_key: "mail-123".into(),
                    hostname: "mail-123.example.test".into(),
                    image: format!("registry.example.test/mail@sha256:{}", "a".repeat(64)),
                    resources: MailResources::FIXED,
                    management_sha256: material.management_fingerprint().unwrap(),
                    certificate_sha256: "b".repeat(64),
                },
                material_sha256: material.fingerprint().unwrap(),
                snapshot: ConfigurationSnapshot {
                    domains: vec![ConfigurationDomain {
                        id: "7001".into(),
                        name: "customer.example.test".into(),
                        provider_id: None,
                        ownership_verified: true,
                    }],
                    mailboxes: vec![],
                    identities: vec![],
                    grants: vec![],
                },
            },
            material,
            lease: MailLease {
                lease_token: uuid::Uuid::new_v4().to_string(),
            },
            lease_until: chrono::Utc::now() + chrono::Duration::seconds(120),
            checkpoints: vec![],
        };
        Self {
            runtime,
            state,
            server,
            claim,
        }
    }
}
#[tokio::test]
async fn dropped_engine_create_reply_reads_back_only_the_marked_handoff_without_recreating() {
    let fixture = Fixture::open().await;
    fixture.state.drop_reply.store(true, Ordering::SeqCst);
    let proof = execute(
        &fixture.runtime,
        &fixture.claim,
        fixture.state.authority.as_ref(),
    )
    .await
    .unwrap();
    assert_eq!(proof.domains["7001"], "owned-domain");
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 1);
    let checkpoints = fixture.state.authority.checkpoints.lock().unwrap();
    assert_eq!(checkpoints.len(), 3);
    assert!(matches!(
        checkpoints[2].phase,
        ConfigurationCheckpointPhase::Applied { .. }
    ));
}
#[tokio::test]
async fn revoked_handoff_prevents_mutation_and_an_absent_uncertain_create_is_never_repeated() {
    let mut fixture = Fixture::open().await;
    fixture
        .state
        .authority
        .revoke_handoff
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        execute(
            &fixture.runtime,
            &fixture.claim,
            fixture.state.authority.as_ref()
        )
        .await,
        Err(ExecutionError::Denied)
    ));
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 0);
    fixture.claim.checkpoints = fixture.state.authority.checkpoints.lock().unwrap().clone();
    fixture
        .state
        .authority
        .allowed
        .store(true, Ordering::SeqCst);
    fixture
        .state
        .authority
        .revoke_handoff
        .store(false, Ordering::SeqCst);
    assert!(matches!(
        execute(
            &fixture.runtime,
            &fixture.claim,
            fixture.state.authority.as_ref()
        )
        .await,
        Err(ExecutionError::Conflict)
    ));
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn revocation_after_an_uncertain_write_cannot_publish_applied_custody() {
    let fixture = Fixture::open().await;
    fixture.state.drop_reply.store(true, Ordering::SeqCst);
    fixture.state.revoke_write.store(true, Ordering::SeqCst);
    assert!(matches!(
        execute(
            &fixture.runtime,
            &fixture.claim,
            fixture.state.authority.as_ref()
        )
        .await,
        Err(ExecutionError::Denied)
    ));
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.state.authority.checkpoints.lock().unwrap().len(), 2);
}
