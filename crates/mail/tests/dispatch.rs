use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use webdock_mail::{
    dispatch::{DispatchTransport, dispatch_one},
    message::{DispatchMessage, PreparedMessage, SendRequest},
    outbox::{PayloadFormat, Receipt, SendLease},
    payload::PrivatePayloadStore,
    provider_transport::{ProviderBinding, ProviderTransport, RoutingTransport},
    send_control::{ControlError, SendControl},
};

struct Control {
    claim: Mutex<Option<SendLease>>,
    calls: Mutex<Vec<Value>>,
    lost: AtomicBool,
    review_response: Mutex<Value>,
}

#[tokio::test]
async fn unavailable_provider_routes_enter_review_before_handoff_and_retain_payloads() {
    let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
    let issuer = ed25519_dalek::SigningKey::from_bytes(&[72; 32]).verifying_key();
    let binding = ProviderBinding {
        plugin_id: "relay-fixture".into(),
        generation: 1,
        descriptor_sha256: "a".repeat(64),
        worker_id: "mail-worker".into(),
        worker_generation: 1,
        worker_credential_sha256: "b".repeat(64),
    };
    let transport = RoutingTransport::new(vec![
        ProviderTransport::new(channel, binding, issuer).unwrap(),
    ])
    .unwrap();
    for field in [
        "plugin",
        "generation",
        "descriptor",
        "missing-generation",
        "missing-descriptor",
    ] {
        let (_dir, store, control, lease) = fixture();
        {
            let mut held = control.claim.lock().unwrap();
            let job = held.as_mut().unwrap();
            job.route = json!({
                "kind":"direct","pluginID":"relay-fixture","generation":1,
                "transportRevision":1,"grantRevision":1,"configurationRevision":0,
                "descriptorSha256":"a".repeat(64),"bindingSha256":"c".repeat(64)
            });
            match field {
                "plugin" => job.route["pluginID"] = json!("unconfigured-relay"),
                "generation" => job.route["generation"] = json!(2),
                "descriptor" => job.route["descriptorSha256"] = json!("d".repeat(64)),
                "missing-generation" => {
                    job.route.as_object_mut().unwrap().remove("generation");
                }
                "missing-descriptor" => {
                    job.route
                        .as_object_mut()
                        .unwrap()
                        .remove("descriptorSha256");
                }
                _ => unreachable!(),
            }
        }
        let dispatched = dispatch_one(control.as_ref(), store.clone(), &transport)
            .await
            .unwrap()
            .unwrap();
        assert!(dispatched.receipt.is_none(), "{field}");
        assert!(store.get("12", &lease.payload_ref).is_ok(), "{field}");
        let calls = control.calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "{field}");
        assert_eq!(calls[1]["action"], "send.reviewPreparation", "{field}");
        assert!(!calls.iter().any(|call| call["action"] == "send.begin"));
        assert!(!calls.iter().any(|call| call["action"] == "send.finish"));
    }
}
#[async_trait::async_trait]
impl SendControl for Control {
    async fn call(&self, input: Value) -> Result<Value, ControlError> {
        self.calls.lock().unwrap().push(input.clone());
        Ok(match input["action"].as_str().unwrap() {
            "send.claim" => serde_json::to_value(self.claim.lock().unwrap().take()).unwrap(),
            "send.begin" => {
                assert_eq!(input["digest"].as_str().unwrap().len(), 64);
                assert_eq!(
                    input["preparation"]["metadataSha256"]
                        .as_str()
                        .unwrap()
                        .len(),
                    64
                );
                assert_eq!(
                    input["preparation"]["mimeSha256"].as_str().unwrap().len(),
                    64
                );
                assert!(input["preparation"]["mimeBytes"].as_u64().unwrap() > 0);
                json!({"kind":"direct","provider":{"lease":"private-provider-lease"}})
            }
            "send.finish" => {
                if self.lost.load(Ordering::SeqCst) {
                    return Err(ControlError::unavailable());
                }
                json!(true)
            }
            "send.reviewPreparation" => self.review_response.lock().unwrap().clone(),
            _ => panic!("unexpected action"),
        })
    }
}
struct Transport {
    attempts: AtomicUsize,
    uncertain: bool,
}
#[async_trait::async_trait]
impl DispatchTransport for Transport {
    fn can_submit(&self, _job: &SendLease) -> bool {
        true
    }
    async fn submit(
        &self,
        _job: &SendLease,
        authorization: &Value,
        message: &DispatchMessage,
        prepared: &PreparedMessage,
    ) -> Receipt {
        assert_eq!(authorization["provider"]["lease"], "private-provider-lease");
        assert_eq!(prepared.from, "sender@example.test");
        assert_eq!(message.bcc, ["hidden@example.test"]);
        assert!(!String::from_utf8_lossy(&prepared.raw).contains("hidden@example.test"));
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.uncertain {
            Receipt::Unknown {}
        } else {
            Receipt::Accepted {
                provider_id: "receipt".into(),
            }
        }
    }
}
fn fixture() -> (
    tempfile::TempDir,
    Arc<PrivatePayloadStore>,
    Arc<Control>,
    SendLease,
) {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(
        PrivatePayloadStore::open(dir.path(), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .unwrap(),
    );
    let message=SendRequest::parse(json!({"from":"sender@example.test","to":"a@example.test","bcc":"hidden@example.test","subject":"Hi","text":"Body"}),chrono::Utc::now()).unwrap();
    let payload = serde_json::to_vec(&message).unwrap();
    let saved = store.put("12", &payload, None).unwrap();
    let lease = SendLease {
        id: "789".into(),
        customer_id: "12".into(),
        source_id: "key:34".into(),
        payload_ref: saved.reference,
        payload_bytes: payload.len() as i32,
        request_hash: hex::encode(Sha256::digest(payload)),
        payload_format: PayloadFormat::Request,
        source_context: json!({}),
        route: json!({"kind":"direct"}),
        envelope: json!({"from":"sender@example.test","to":["a@example.test","hidden@example.test"]}),
        lease_token: uuid::Uuid::new_v4().to_string(),
    };
    let control = Arc::new(Control {
        claim: Mutex::new(Some(lease.clone())),
        calls: Mutex::new(Vec::new()),
        lost: AtomicBool::new(false),
        review_response: Mutex::new(json!(true)),
    });
    (dir, store, control, lease)
}

#[tokio::test]
async fn unsupported_route_requires_confirmed_review_without_handoff() {
    for response in [json!(false), Value::Null, json!({"recorded":true})] {
        let (_dir, store, control, lease) = fixture();
        *control.review_response.lock().unwrap() = response;
        let transport = RoutingTransport::new(vec![]).unwrap();
        assert!(
            dispatch_one(control.as_ref(), store.clone(), &transport)
                .await
                .is_err()
        );
        assert!(store.get("12", &lease.payload_ref).is_ok());
        let calls = control.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["action"], "send.claim");
        assert_eq!(calls[1]["action"], "send.reviewPreparation");
    }
}
#[tokio::test]
async fn lost_receipt_and_unknown_preserve_payload_and_never_retry_transport() {
    for lost in [false, true] {
        let (_dir, store, control, lease) = fixture();
        control.lost.store(lost, Ordering::SeqCst);
        let transport = Transport {
            attempts: AtomicUsize::new(0),
            uncertain: !lost,
        };
        let result = dispatch_one(control.as_ref(), store.clone(), &transport).await;
        assert_eq!(result.is_err(), lost);
        assert!(store.get("12", &lease.payload_ref).is_ok());
        assert!(
            dispatch_one(control.as_ref(), store.clone(), &transport)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(transport.attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            control
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|x| x["action"] == "send.finish")
                .count(),
            1
        );
    }
}
#[tokio::test]
async fn payload_and_envelope_integrity_fail_before_handoff_and_accepted_cleanup_follows_receipt() {
    for field in ["hash", "envelope", "accepted"] {
        let (_dir, store, control, lease) = fixture();
        {
            let mut held = control.claim.lock().unwrap();
            let job = held.as_mut().unwrap();
            if field == "hash" {
                job.request_hash = "f".repeat(64);
            }
            if field == "envelope" {
                job.envelope["to"] = json!(["injected@example.test"]);
            }
        }
        let transport = Transport {
            attempts: AtomicUsize::new(0),
            uncertain: false,
        };
        let result = dispatch_one(control.as_ref(), store.clone(), &transport).await;
        if field == "accepted" {
            assert!(matches!(
                result.unwrap().unwrap().receipt,
                Some(Receipt::Accepted { .. })
            ));
            assert!(store.get("12", &lease.payload_ref).is_err());
        } else {
            assert!(result.is_err());
            assert_eq!(transport.attempts.load(Ordering::SeqCst), 0);
            let calls = control.calls.lock().unwrap();
            assert_eq!(calls.last().unwrap()["action"], "send.reviewPreparation");
            assert!(!calls.iter().any(|x| x["action"] == "send.begin"));
            assert!(store.get("12", &lease.payload_ref).is_ok());
        }
    }
}
