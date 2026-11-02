use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use webdock_mail_runtime::{
    dispatch::cleanup_payloads,
    payload::PrivatePayloadStore,
    send_control::{ControlError, SendControl},
};
struct Control {
    reference: String,
    lose_ack: AtomicBool,
    invalid: AtomicBool,
}
#[async_trait::async_trait]
impl SendControl for Control {
    async fn call(&self, input: Value) -> Result<Value, ControlError> {
        match input["action"].as_str().unwrap() {
            "send.cleanup" => Ok(
                json!([{"id":"789","customerID":if self.invalid.load(Ordering::SeqCst){"0"}else{"123"},"ref":self.reference}]),
            ),
            "send.cleanupDone" => {
                if self.lose_ack.load(Ordering::SeqCst) {
                    Err(ControlError::unavailable())
                } else {
                    Ok(json!({"ok":true}))
                }
            }
            _ => panic!("unexpected cleanup command"),
        }
    }
}
#[tokio::test]
async fn cleanup_removes_only_valid_core_selection_and_replays_lost_ack_without_resend() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(
        PrivatePayloadStore::open(dir.path(), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .unwrap(),
    );
    let accepted = store.put("123", b"accepted", None).unwrap();
    let unknown = store.put("123", b"unknown", None).unwrap();
    let control = Control {
        reference: accepted.reference.clone(),
        lose_ack: AtomicBool::new(true),
        invalid: AtomicBool::new(true),
    };
    assert!(cleanup_payloads(&control, store.clone()).await.is_err());
    assert_eq!(store.get("123", &accepted.reference).unwrap(), b"accepted");
    control.invalid.store(false, Ordering::SeqCst);
    assert!(cleanup_payloads(&control, store.clone()).await.is_err());
    assert!(store.get("123", &accepted.reference).is_err());
    assert_eq!(store.get("123", &unknown.reference).unwrap(), b"unknown");
    control.lose_ack.store(false, Ordering::SeqCst);
    assert_eq!(cleanup_payloads(&control, store.clone()).await.unwrap(), 1);
    assert_eq!(store.get("123", &unknown.reference).unwrap(), b"unknown");
}
