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
    ActionHandler, InvocationAuthority, PeerIdentity, Runtime, UnixSocket, wire,
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
struct Handler(AtomicUsize);
#[tonic::async_trait]
impl ActionHandler for Handler {
    async fn authorize_storage(
        &self,
        a: &webdock_plugin_protocol::storage::StorageAuthorization,
    ) -> Result<(), PluginError> {
        if self.0.load(Ordering::SeqCst) == 0 && a.resource_epoch == "active-1" {
            Ok(())
        } else {
            Err(PluginError::denied())
        }
    }

    async fn invoke(
        &self,
        _: &InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> Result<Value, PluginError> {
        let mode = self.0.load(Ordering::SeqCst);
        Ok(
            json!({"version":1,"binding":"objects","direction":if mode==1{"upload"}else{"download"},"resourceEpoch":"active-1","resourceId":if mode==2{"foreign"}else{"owned"},"url":"https://storage.invalid/private/object?X-Amz-Signature=private","headers":{},"expiresAt":if mode==3{0}else{SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+120},"exactBytes":7,"expectedSha256":"a".repeat(64),"attachment":{"filename":"file.bin","contentType":"application/octet-stream"}}),
        )
    }
}
#[tokio::test]
async fn actual_runtime_uses_private_plan_abi_and_rejects_changed_resource_direction_or_expiry() {
    let mut v: Value = serde_json::from_str(include_str!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    v["capabilities"] = json!(["plugin.storage.read"]);
    v["services"]["required"] = json!([{"name":"storage","version":1}]);
    v["jobs"] = json!([]);
    v["areas"]["selfService"] = json!({"pages":[]});
    v["actions"] = json!([{"name":"download","intent":"read","audience":"customer","capabilities":["plugin.storage.read"],"storage":"download","resource":"use","input":{"type":"object","properties":{"resource_id":{"type":"string","maxLength":128}},"required":["resource_id"],"additionalProperties":false},"output":{"type":"object","properties":{"id":{"type":"string","maxLength":64},"path":{"type":"string","maxLength":256}},"required":["id","path"],"additionalProperties":false}}]);
    let manifest = Manifest::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("runtime.sock");
    let handler = Arc::new(Handler(AtomicUsize::new(0)));
    let runtime = Runtime::new(
        manifest,
        2,
        SigningKey::from_bytes(&[61; 32]).verifying_key(),
        Arc::new(Authority),
        handler.clone(),
    )
    .unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let socket = UnixSocket::bind(&path).unwrap();
    let server = tokio::spawn(runtime.serve_unix(socket, async {
        let _ = stopped.await;
    }));
    let mut client = webdock_plugin_runtime::connect_unix(&path).await.unwrap();
    for mode in 0..4 {
        handler.0.store(mode, Ordering::SeqCst);
        let result = client
            .invoke(wire::InvokeRequest {
                invocation_token: token("project-notes", "download", false),
                action: "download".into(),
                input_json: br#"{"resource_id":"owned"}"#.to_vec(),
                configuration_json: b"{}".to_vec(),
            })
            .await;
        if mode == 0 {
            let reply: Value =
                serde_json::from_slice(&result.unwrap().into_inner().output_json).unwrap();
            assert_eq!(reply["binding"], "objects");
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code(), tonic::Code::Internal);
            assert!(!error.message().contains("storage.invalid"));
            assert!(!error.message().contains("Signature"));
        }
    }
    let authorization = json!({"version":1,"transferId":uuid_fixture(),"sourceInvocation":uuid_fixture(),"pluginId":"project-notes","generation":2,"action":"download","customerId":"101","projectId":"123","resourceId":"owned","resourceEpoch":"active-1","direction":"download","exactBytes":7,"expectedSha256":"a".repeat(64),"expiresAt":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+120});
    for mode in [0, 4, 5] {
        handler.0.store(mode, Ordering::SeqCst);
        let result = client
            .authorize_storage(wire::StorageAuthorizationRequest {
                authorization_json: serde_json::to_vec(&authorization).unwrap(),
            })
            .await;
        if mode == 0 {
            assert!(result.unwrap().into_inner().active);
        } else {
            assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
        }
    }
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
