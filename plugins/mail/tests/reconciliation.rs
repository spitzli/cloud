//! Interrupted setup against an in-process JMAP boundary; no mail or external services.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webdock_mail::engine;
use webdock_plugin_protocol::PluginError;

#[derive(Default)]
struct State {
    listeners: BTreeMap<String, Value>,
    certificate: Option<String>,
    default_certificate: Option<String>,
    recovery: bool,
    applies: usize,
    password_updates: usize,
    delayed_removal: bool,
    pending_removal: usize,
}
#[derive(Clone)]
struct Host(Arc<Mutex<State>>);
#[tonic::async_trait]
impl engine::Workloads for Host {
    async fn call(&self, method: &str, input: Value) -> Result<Value, PluginError> {
        assert_eq!(
            method, "apply",
            "completion must never destroy retained storage"
        );
        assert_eq!(input["customer"], "101");
        assert_eq!(input["unit"], "engine-c101");
        let objects = input["objects"].as_array().unwrap();
        assert!(objects.iter().any(
            |o| o["kind"] == "PersistentVolumeClaim" && o["metadata"]["name"] == "engine-data"
        ));
        assert!(
            !objects
                .iter()
                .any(|o| o["metadata"]["name"] == "engine-recovery"),
            "an authenticated administrator needs no recovery bootstrap"
        );
        let mut state = self.0.lock().unwrap();
        if state.delayed_removal && state.recovery {
            state.pending_removal = 3;
        } else {
            state.recovery = false;
        }
        state.applies += 1;
        Ok(json!({"revision":state.applies}))
    }
}
fn jmap_response(state: &mut State, input: &Value, recovery: bool) -> (&'static str, Value) {
    let transient = recovery && state.pending_removal == 3;
    let forbidden = recovery && state.pending_removal == 2;
    if recovery && state.pending_removal > 0 {
        state.pending_removal -= 1;
        if state.pending_removal == 0 {
            state.recovery = false;
        }
    }
    if transient {
        ("503 Service Unavailable", json!({}))
    } else if forbidden {
        ("403 Forbidden", json!({}))
    } else if recovery && !state.recovery {
        ("401 Unauthorized", json!({}))
    } else {
        let call = &input["methodCalls"][0];
        let method = call[0].as_str().unwrap();
        let result = match method {
            "x:Domain/get" => {
                json!({"list":[{"id":"retained-domain","name":"retained.example.test"}]})
            }
            "x:Account/get" => {
                json!({"list":[{"id":"retained-admin","name":"webdock-admin"},{"id":"retained-mailbox","name":"hello"}]})
            }
            "x:Account/set" => {
                let update = &call[1]["update"];
                assert_eq!(update.as_object().unwrap().len(), 1);
                assert_eq!(
                    update["retained-admin"]["credentials"]["0"]["secret"],
                    "fixture-admin-password"
                );
                assert!(update.get("retained-mailbox").is_none());
                state.password_updates += 1;
                json!({"updated":{"retained-admin":null}})
            }
            "x:NetworkListener/get" => {
                json!({"list":state.listeners.keys().map(|name|json!({"id":name,"name":name})).collect::<Vec<_>>()})
            }
            "x:NetworkListener/set" => {
                for (name, value) in call[1]["create"].as_object().unwrap() {
                    assert!(
                        state
                            .listeners
                            .insert(name.clone(), value.clone())
                            .is_none(),
                        "completion must not duplicate a retained listener"
                    );
                }
                json!({"created":{}})
            }
            "x:Certificate/get" => {
                json!({"list":state.certificate.iter().map(|id|json!({"id":id})).collect::<Vec<_>>()})
            }
            "x:Certificate/set" => {
                assert!(
                    state.certificate.is_none(),
                    "completion must reuse a retained certificate"
                );
                state.certificate = Some("retained-certificate".into());
                json!({"created":{"c":{"id":"retained-certificate"}}})
            }
            "x:SystemSettings/set" => {
                assert_eq!(
                    call[1]["update"]["singleton"]["defaultCertificateId"],
                    "retained-certificate"
                );
                state.default_certificate = Some("retained-certificate".into());
                json!({"updated":{"singleton":null}})
            }
            _ => panic!("authenticated setup must complete without bootstrapping: {method}"),
        };
        ("200 OK", json!({"methodResponses":[[method,result,"w"]]}))
    }
}
async fn server(state: Arc<Mutex<State>>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (input, recovery) = loop {
                let mut chunk = [0; 8192];
                let read = socket.read(&mut chunk).await.unwrap();
                assert_ne!(read, 0);
                bytes.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&bytes);
                let Some((head, body)) = text.split_once("\r\n\r\n") else {
                    continue;
                };
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|n| n.parse::<usize>().unwrap())
                    })
                    .unwrap();
                if body.len() < length {
                    continue;
                }
                let recovery = head.lines().any(|l| {
                    l.to_ascii_lowercase()
                        .starts_with("authorization: basic ywrta")
                });
                break (
                    serde_json::from_str::<Value>(&body[..length]).unwrap(),
                    recovery,
                );
            };
            let (status, response) = jmap_response(&mut state.lock().unwrap(), &input, recovery);
            let body = response.to_string();
            socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    (format!("http://{address}"), server)
}

#[tokio::test]
async fn authenticated_engines_finish_each_interrupted_setup_boundary_idempotently() {
    for boundary in 0..5 {
        let mut initial = State {
            recovery: true,
            delayed_removal: boundary == 4,
            ..State::default()
        };
        if boundary >= 1 {
            for name in ["http", "imaps", "submissions"] {
                initial.listeners.insert(name.into(), json!({"name":name}));
            }
        }
        if boundary >= 2 {
            initial.certificate = Some("retained-certificate".into());
        }
        if boundary >= 3 {
            initial.default_certificate = Some("retained-certificate".into());
        }
        let state = Arc::new(Mutex::new(initial));
        let (origin, server) = server(state.clone()).await;
        let engines = engine::Engines::new(
            Box::new(Host(state.clone())),
            engine::EngineConfig {
                image: format!("fixture.invalid/stalwart@sha256:{}", "a".repeat(64)),
                domain: "mail.localhost".into(),
                imap_port: 1993,
                submission_port: 2465,
                admin: engine::AdminRoute {
                    origin,
                    domain: "mail-admin.localhost".into(),
                    entry_points: vec!["web".into()],
                },
                issuer: None,
            },
        )
        .unwrap();
        for completion in 1..=2 {
            engines
                .provision("101", "fixture-admin-password")
                .await
                .unwrap();
            let state = state.lock().unwrap();
            assert_eq!(state.listeners.len(), 3, "boundary {boundary}");
            assert_eq!(state.certificate.as_deref(), Some("retained-certificate"));
            assert_eq!(
                state.default_certificate.as_deref(),
                Some("retained-certificate")
            );
            assert!(
                !state.recovery,
                "boundary {boundary}: permanent authentication is not proof that recovery was removed"
            );
            assert_eq!(state.applies, completion);
            assert_eq!(state.password_updates, completion);
        }
        server.abort();
    }
}
