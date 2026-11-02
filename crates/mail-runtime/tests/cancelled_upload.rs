use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;
use tower::ServiceExt;
use webdock_mail_runtime::{
    ControlError, FilesystemPayloadStore, PrivatePayloadStore, Result, SavedPayload, SendControl,
    send_api::SendApi,
};

struct SlowStore {
    filesystem: FilesystemPayloadStore,
    entered: Notify,
    release: Notify,
    completed: AtomicBool,
    first: AtomicBool,
}
#[async_trait]
impl PrivatePayloadStore for SlowStore {
    async fn put(&self, customer: &str, reference: &str, bytes: &[u8]) -> Result<SavedPayload> {
        if self.first.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        let saved = PrivatePayloadStore::put(&self.filesystem, customer, reference, bytes).await?;
        self.completed.store(true, Ordering::SeqCst);
        Ok(saved)
    }
    async fn get(&self, customer: &str, reference: &str) -> Result<Vec<u8>> {
        PrivatePayloadStore::get(&self.filesystem, customer, reference).await
    }
    async fn remove(&self, customer: &str, reference: &str) -> Result<()> {
        PrivatePayloadStore::remove(&self.filesystem, customer, reference).await
    }
}
struct Control;
#[async_trait]
impl SendControl for Control {
    async fn call(&self, input: Value) -> std::result::Result<Value, ControlError> {
        Ok(match input["action"].as_str().unwrap() {
            "send.authenticate" => json!({"customerID":"12"}),
            "send.admit" => {
                json!({"id":"789","state":"staging","created":true,"customerID":"12","payloadRef":input["submission"]["payloadRef"],"uploadToken":uuid::Uuid::new_v4()})
            }
            "send.commitPayload" => json!({"queued":true}),
            _ => return Err(ControlError::unavailable()),
        })
    }
}
fn request() -> Request<Body> {
    Request::post("/v1/emails")
        .header("authorization", format!("Bearer wd_mail_{}", "x".repeat(43)))
        .header("content-type", "application/json")
        .body(Body::from(json!({"from":"sender@example.test","to":"recipient@example.test","subject":"Test","text":"Body"}).to_string())).unwrap()
}
#[tokio::test]
async fn disconnected_upload_keeps_customer_exclusion_until_private_custody_finishes() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(SlowStore {
        filesystem: FilesystemPayloadStore::open(
            directory.path(),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap(),
        entered: Notify::new(),
        release: Notify::new(),
        completed: AtomicBool::new(false),
        first: AtomicBool::new(true),
    });
    let router = SendApi::new(Arc::new(Control), store.clone()).router();
    let pending = tokio::spawn(router.clone().oneshot(request()));
    tokio::time::timeout(std::time::Duration::from_secs(2), store.entered.notified())
        .await
        .unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    let response = router.oneshot(request()).await.unwrap();
    let status = response.status();
    store.release.notify_one();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !store.completed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
