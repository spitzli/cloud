//! The preserved Mail action character contract needs one narrowly authenticated input exception.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use webdock_plugin_protocol::{InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::{
    ActionHandler, InvocationAuthority, JobHandler, PeerIdentity, Runtime, UnixSocket, wire,
};
struct Authority;
#[tonic::async_trait]
impl InvocationAuthority for Authority {
    async fn authorize_peer(&self, _: &PeerIdentity) -> Result<(), PluginError> {
        Ok(())
    }
    async fn authorize_invocation(
        &self,
        _: &PeerIdentity,
        _: &InvocationClaims,
    ) -> Result<(), PluginError> {
        Ok(())
    }
    async fn authorize_request(
        &self,
        _: &PeerIdentity,
        _: &InvocationClaims,
        _: &[u8],
        _: &[u8],
    ) -> Result<(), PluginError> {
        Ok(())
    }
}
struct Handler(AtomicUsize);
#[tonic::async_trait]
impl ActionHandler for Handler {
    async fn invoke(
        &self,
        _: &InvocationClaims,
        _: &str,
        input: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"characters":input["text"].as_str().unwrap().chars().count()}))
    }
}
#[tonic::async_trait]
impl JobHandler for Handler {
    async fn run(
        &self,
        _: &InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        panic!("oversized job must not reach handler")
    }
}
fn token(plugin: &str, action: &str, job: bool) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut c = json!({"v":1,"id":uuid_fixture(),"pluginID":plugin,"generation":2,"action":action,"customerID":"101","projectID":"123","iat":now,"exp":now+60});
    if job {
        c["job"] = json!({"id":format!("job-{}","a".repeat(64)),"attempt":1});
    }
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&c).unwrap());
    format!(
        "{body}.{}",
        URL_SAFE_NO_PAD.encode(
            SigningKey::from_bytes(&[61; 32])
                .sign(body.as_bytes())
                .to_bytes()
        )
    )
}
fn uuid_fixture() -> &'static str {
    "123e4567-e89b-42d3-a456-426614174000"
}
fn manifest(plugin: &str) -> Manifest {
    let mut v: Value = serde_json::from_str(include_str!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    v["name"] = json!(plugin);
    v["actions"] = json!([{"name":"mail.send","intent":"write","audience":"operator","capabilities":[]},{"name":"mail.status","intent":"read","audience":"operator","capabilities":[]}]);
    v["configuration"] = json!({"type":"object","properties":{"value":{"type":"string","maxLength":65536}},"additionalProperties":false});
    v["jobs"] = json!([{"name":"mail.send","capabilities":[]}]);
    Manifest::parse(&serde_json::to_vec(&v).unwrap()).unwrap()
}
#[tokio::test]
async fn actual_invoke_preserves_mail_unicode_only_and_keeps_other_action_plugin_job_caps() {
    for plugin in ["mail", "project-notes"] {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = dir.path().join("runtime.sock");
        let socket = UnixSocket::bind(&path).unwrap();
        let handler = Arc::new(Handler(AtomicUsize::new(0)));
        let runtime = Runtime::new(
            manifest(plugin),
            2,
            SigningKey::from_bytes(&[61; 32]).verifying_key(),
            Arc::new(Authority),
            handler.clone(),
        )
        .unwrap()
        .with_jobs(handler.clone());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(runtime.serve_unix(socket, async {
            let _ = stopped.await;
        }));
        let mut client = webdock_plugin_runtime::connect_unix(&path)
            .await
            .unwrap()
            .max_encoding_message_size(224 * 1024);
        for chars in [20000, 30000] {
            let result = client
                .invoke(wire::InvokeRequest {
                    invocation_token: token(plugin, "mail.send", false),
                    action: "mail.send".into(),
                    input_json: serde_json::to_vec(&json!({"text":"😀".repeat(chars)})).unwrap(),
                    configuration_json: b"{}".to_vec(),
                })
                .await;
            if plugin == "mail" {
                assert_eq!(
                    serde_json::from_slice::<Value>(&result.unwrap().into_inner().output_json)
                        .unwrap()["characters"],
                    chars
                );
            } else {
                assert_eq!(result.unwrap_err().code(), tonic::Code::ResourceExhausted);
            }
        }
        if plugin == "mail" {
            let result = client
                .invoke(wire::InvokeRequest {
                    invocation_token: token(plugin, "mail.send", false),
                    action: "mail.send".into(),
                    input_json: serde_json::to_vec(&json!({"text":"😀".repeat(30000)})).unwrap(),
                    configuration_json: serde_json::to_vec(&json!({"value":"x".repeat(60000)}))
                        .unwrap(),
                })
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&result.into_inner().output_json).unwrap()["characters"],
                30000
            );
        }
        for (action, text) in [
            ("mail.status", "x".repeat(32768)),
            ("mail.send", "x".repeat(131072)),
        ] {
            let expected = if plugin != "mail" && action == "mail.send" {
                tonic::Code::OutOfRange
            } else {
                tonic::Code::ResourceExhausted
            };
            assert_eq!(
                client
                    .invoke(wire::InvokeRequest {
                        invocation_token: token(plugin, action, false),
                        action: action.into(),
                        input_json: serde_json::to_vec(&json!({"text":text})).unwrap(),
                        configuration_json: b"{}".to_vec()
                    })
                    .await
                    .unwrap_err()
                    .code(),
                expected
            );
        }
        assert_eq!(
            client
                .invoke(wire::InvokeRequest {
                    invocation_token: token(plugin, "mail.send", false),
                    action: "mail.send".into(),
                    input_json: serde_json::to_vec(&json!({"text":"small"})).unwrap(),
                    configuration_json: serde_json::to_vec(&json!({"value":"x".repeat(65536)}))
                        .unwrap()
                })
                .await
                .unwrap_err()
                .code(),
            tonic::Code::ResourceExhausted
        );
        assert_eq!(
            client
                .run_job(wire::RunJobRequest {
                    invocation_token: token(plugin, "mail.send", true),
                    name: "mail.send".into(),
                    input_json: serde_json::to_vec(&json!({"text":"x".repeat(32768)})).unwrap(),
                    configuration_json: b"{}".to_vec()
                })
                .await
                .unwrap_err()
                .code(),
            tonic::Code::ResourceExhausted
        );
        assert_eq!(
            handler.0.load(Ordering::SeqCst),
            if plugin == "mail" { 3 } else { 0 }
        );
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }
}
