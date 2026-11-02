//! Product action custody with a deliberate controlled Core stand-in, using owned TLS PostgreSQL.
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use webdock_mail::{
    Dns, Handler, Plugin, Settings, delivery::Delivery, payload::PgPrivatePayloadStore,
    seal::Sealer, store::Store, transport,
};
use webdock_mail_runtime::{ControlError, PrivatePayloadStore, SendControl};
use webdock_plugin_protocol::{ErrorCode, InvocationClaims};
use webdock_plugin_runtime::ActionHandler;
struct NoDns;
#[tonic::async_trait]
impl Dns for NoDns {
    async fn txt(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
}
struct Control {
    original: Mutex<Option<Value>>,
    proposals: Mutex<Vec<Value>>,
    lost: AtomicBool,
    pending: AtomicBool,
}
#[async_trait::async_trait]
impl SendControl for Control {
    async fn call(&self, v: Value) -> Result<Value, ControlError> {
        self.proposals.lock().unwrap().push(v.clone());
        match v["action"].as_str().unwrap() {
            "send.admitAction" => {
                assert_eq!(v["submission"].as_object().unwrap().len(), 4);
                assert_eq!(
                    v["submission"]["invocationToken"],
                    "actual invocation token"
                );
                let mut original = self.original.lock().unwrap();
                let created = original.is_none();
                if let Some(old) = &*original {
                    if old["input"] != v["submission"]["input"]
                        || old["privatePolicySha256"] != v["submission"]["privatePolicySha256"]
                    {
                        return Err(ControlError { status: 409 });
                    }
                } else {
                    *original = Some(v["submission"].clone());
                }
                if self.lost.swap(false, Ordering::SeqCst) {
                    return Err(ControlError::unavailable());
                }
                Ok(
                    json!({"id":"8811","state":if self.pending.load(Ordering::SeqCst){"queued"}else{"staging"},"created":created,"customerID":"8101","payloadRef":original.as_ref().unwrap()["payloadRef"],"uploadToken":"123e4567-e89b-42d3-a456-426614174000"}),
                )
            }
            "send.commitPayload" => {
                assert_eq!(
                    v["lease"]["payloadRef"],
                    self.original.lock().unwrap().as_ref().unwrap()["payloadRef"]
                );
                self.pending.store(true, Ordering::SeqCst);
                Ok(json!({"queued":true}))
            }
            "send.actionStatus" => {
                let token = v["invocationToken"].as_str().unwrap();
                if token == "history token" {
                    Ok(
                        json!({"items":[{"id":"8811","state":"queued","from":"hello@example.test","to":["a@example.test"],"subject":"subject","transport":"direct","at":"2026-10-24T00:00:00Z","private":"removed"}]}),
                    )
                } else {
                    Ok(json!({"id":"8811","state":"queued","private":"removed"}))
                }
            }
            "send.checkTransition" => {
                if self.pending.load(Ordering::SeqCst) {
                    Err(ControlError::unavailable())
                } else {
                    Ok(json!({"allowed":true}))
                }
            }
            _ => Err(ControlError::invalid()),
        }
    }
}
#[async_trait::async_trait]
impl webdock_mail_protocol::native_control::Control for Control {
    async fn call(
        &self,
        peer: &webdock_plugin_runtime::PeerIdentity,
        generation: u64,
        input: Value,
    ) -> Result<Value, webdock_mail_protocol::native_control::ControlError> {
        assert!(matches!(
            peer,
            webdock_plugin_runtime::PeerIdentity::Unix { .. }
        ));
        assert_eq!(generation, 2);
        <Self as SendControl>::call(self, input)
            .await
            .map_err(|e| webdock_mail_protocol::native_control::ControlError { status: e.status })
    }
}
fn claims(action: &str) -> InvocationClaims {
    InvocationClaims {
        job: None,
        v: 1,
        id: uuid::Uuid::new_v4().to_string(),
        plugin_id: "mail".into(),
        generation: 2,
        action: action.into(),
        project_id: "8201".into(),
        customer_id: "8101".into(),
        iat: 0,
        exp: 0,
    }
}
#[tokio::test]
#[ignore = "requires fresh task3-owned TLS PostgreSQL"]
async fn lost_admission_resumes_original_payload_and_transition_checks_share_customer_fence() {
    let url = std::env::var("WEBDOCK_MAIL_TASK3_TEST_URL").unwrap();
    let ca = std::fs::read(std::env::var("WEBDOCK_MAIL_TASK3_TEST_CA").unwrap()).unwrap();
    let store = Store::connect_tls(&url, &ca).await.unwrap();
    let plugin = Plugin::new(
        store,
        Sealer::new(&"24".repeat(32)).unwrap(),
        transport::Sender::new(transport::TURBOSMTP_ORIGIN),
        Box::new(NoDns),
        Settings {
            platform: transport::Platform::Smtp {
                host: "127.0.0.1".into(),
                port: 9,
            },
            trust_unverified_domains: true,
        },
        None,
    );
    let created = Handler(plugin.clone())
        .invoke(
            &claims("domains.create"),
            "create token",
            json!({"domain":format!("owned-{}.test",uuid::Uuid::new_v4().simple())}),
            json!({}),
        )
        .await
        .unwrap();
    let resource = created["resource"]["id"].clone();
    let input = json!({"resource_id":resource,"from":"hello","from_name":"😀".repeat(80),"to":"a@example.test","subject":"subject","text":"😀".repeat(30000)});
    assert_eq!(
        Handler(plugin.clone())
            .invoke(
                &claims("mail.send"),
                "actual invocation token",
                input.clone(),
                json!({})
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::CapabilityUnavailable
    );
    let store = plugin.store();
    let payloads = Arc::new(
        PgPrivatePayloadStore::new(store.clone(), &"24".repeat(32))
            .await
            .unwrap(),
    );
    let control = Arc::new(Control {
        original: Mutex::new(None),
        proposals: Mutex::new(Vec::new()),
        lost: AtomicBool::new(true),
        pending: AtomicBool::new(false),
    });
    let directory = tempfile::Builder::new()
        .permissions({
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o700)
        })
        .tempdir()
        .unwrap();
    let socket_path = directory.path().join("core.sock");
    let mut socket = webdock_plugin_runtime::UnixSocket::bind(&socket_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = webdock_mail_protocol::native_control::ControlServer::service(control.clone());
    let server = tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let channel = webdock_plugin_runtime::unix_channel(&socket_path)
        .await
        .unwrap();
    let native = Arc::new(
        webdock_mail_protocol::native_control::NativeControlClient::new(channel, 2).unwrap(),
    );
    let plugin = plugin
        .with_delivery(Delivery::new(native, payloads.clone()))
        .unwrap();
    let handler = Handler(plugin);
    assert!(
        handler
            .invoke(
                &claims("mail.send"),
                "actual invocation token",
                input.clone(),
                json!({})
            )
            .await
            .is_err()
    );
    assert!(
        control.original.lock().unwrap().is_some(),
        "30000 non-BMP action characters must reach NativeControl; old65KiB command cap cannot carry original input"
    );
    let first = control.original.lock().unwrap().as_ref().unwrap()["payloadRef"]
        .as_str()
        .unwrap()
        .to_owned();
    let receipt = handler
        .invoke(
            &claims("mail.send"),
            "actual invocation token",
            input.clone(),
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(receipt, json!({"id":"8811","state":"queued"}));
    let retained = payloads.get("8101", &first).await.unwrap();
    let request = webdock_mail_runtime::message::SendRequest::parse_stored_action(
        serde_json::from_slice(&retained).unwrap(),
    )
    .unwrap();
    let compiled = request.compile_action().unwrap();
    assert!(
        webdock_mail_runtime::message::prepare_action_message(
            &compiled.raw,
            &compiled.sender,
            &compiled.to
        )
        .is_ok()
    );
    let mut twenty_thousand = input.clone();
    twenty_thousand["text"] = json!("😀".repeat(20000));
    assert!(
        handler
            .invoke(
                &claims("mail.send"),
                "actual invocation token",
                twenty_thousand,
                json!({})
            )
            .await
            .is_err()
    );
    assert!(control.proposals.lock().unwrap().iter().any(|p| {
        p["submission"]["input"]["text"]
            .as_str()
            .is_some_and(|s| s.chars().count() == 20000)
    }));
    assert_eq!(
        handler
            .invoke(
                &claims("mail.send"),
                "actual invocation token",
                input.clone(),
                json!({})
            )
            .await
            .unwrap(),
        receipt
    );
    let mut changed = input;
    changed["text"] = json!("changed");
    assert!(
        handler
            .invoke(
                &claims("mail.send"),
                "actual invocation token",
                changed,
                json!({})
            )
            .await
            .is_err()
    );
    let status = handler
        .invoke(
            &claims("mail.status"),
            "status token",
            json!({"resource_id":resource,"id":"8811"}),
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(status, receipt);
    let history = handler
        .invoke(
            &claims("mail.history"),
            "history token",
            json!({"resource_id":resource}),
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(history["items"][0].as_object().unwrap().len(), 7);
    for (action, transition) in [
        ("domains.delete", json!({"resource_id":resource})),
        ("incoming.activate", json!({"resource_id":resource})),
        (
            "mailboxes.create",
            json!({"domain_id":resource,"local_part":"test"}),
        ),
    ] {
        assert!(
            handler
                .invoke(&claims(action), "transition token", transition, json!({}))
                .await
                .is_err()
        );
    }
    assert!(
        store
            .domain(
                "8201",
                &uuid::Uuid::parse_str(resource.as_str().unwrap()).unwrap()
            )
            .await
            .unwrap()
            .is_some()
    );
    assert!(store.engines().await.unwrap().is_empty());
    let other = Arc::new(Store::connect_tls(&url, &ca).await.unwrap());
    let held = store.customer_fence("8101").await.unwrap();
    let acquired = Arc::new(AtomicBool::new(false));
    let observed = acquired.clone();
    let waiter = tokio::spawn(async move {
        let _guard = other.customer_fence("8101").await.unwrap();
        observed.store(true, Ordering::SeqCst);
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert!(!acquired.load(Ordering::SeqCst));
    drop(held);
    waiter.await.unwrap();
    assert!(acquired.load(Ordering::SeqCst));
    // Every retained customer-engine state and legacy quarantine denies direct admission/handoff.
    let engine_id = uuid::Uuid::new_v4();
    store
        .engine("8301", &engine_id, b"owned fixture sealed placeholder")
        .await
        .unwrap();
    assert!(!store.direct_allowed("8301").await.unwrap());
    store.engine_state("8301", "failed").await.unwrap();
    assert!(!store.direct_allowed("8301").await.unwrap());
    store.engine_enabled("8301", false).await.unwrap();
    store.engine_state("8301", "suspended").await.unwrap();
    assert!(!store.direct_allowed("8301").await.unwrap());
    let db = webdock_plugin_kit::postgres::connect(&url, Some(&ca))
        .await
        .unwrap();
    let legacy = uuid::Uuid::new_v4();
    db.execute("INSERT INTO engines(project_id,id,admin_sealed,state) VALUES('owned-task3-quarantine',$1,$2,'failed')",&[&legacy,&&b"owned fixture"[..]]).await.unwrap();
    assert!(!store.direct_allowed("8401").await.unwrap());
    db.execute(
        "DELETE FROM engines WHERE project_id='owned-task3-quarantine' AND id=$1",
        &[&legacy],
    )
    .await
    .unwrap();

    assert_eq!(
        db.execute(
            "DELETE FROM customer_engines WHERE customer_id='8301' AND id=$1",
            &[&engine_id]
        )
        .await
        .unwrap(),
        1
    );
    server.abort();
}
