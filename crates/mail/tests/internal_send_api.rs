use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tower::ServiceExt;
use webdock_mail::{
    payload::PrivatePayloadStore,
    send_api::SendApi,
    send_control::{ControlError, SendControl},
};
const INTERNAL: &str = "owned-webmail-ingress-secret-at-least32";
struct Control {
    calls: Mutex<Vec<Value>>,
    allow: AtomicBool,
}
#[async_trait::async_trait]
impl SendControl for Control {
    async fn call(&self, input: Value) -> Result<Value, ControlError> {
        self.calls.lock().unwrap().push(input.clone());
        if !self.allow.load(Ordering::SeqCst) {
            return Err(ControlError::unavailable());
        }
        Ok(match input["action"].as_str().unwrap() {
            "send.policy.auth" => json!({"ok":true}),
            "send.policy" => json!({"accept":true}),
            "send.webmail.lookup" => Value::Null,
            "send.webmail.prepare" => {
                json!({"existing":true,"id":"789","state":"unknown","filed":false,"filingState":null})
            }
            _ => panic!("unexpected send command"),
        })
    }
}
fn fixture() -> (tempfile::TempDir, Arc<Control>, axum::Router) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(
        PrivatePayloadStore::open(dir.path(), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .unwrap(),
    );
    let control = Arc::new(Control {
        calls: Mutex::new(vec![]),
        allow: AtomicBool::new(true),
    });
    let router = SendApi::with_internal(control.clone(), store, INTERNAL.into())
        .unwrap()
        .router();
    (dir, control, router)
}
fn request(path: &str, secret: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {secret}"))
        .header("content-type", "application/json")
        .body(body)
        .unwrap()
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
fn panic_body() -> Body {
    Body::from_stream(futures_util::stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<axum::body::Bytes, std::io::Error>>> {
            panic!("unauthenticated body polled")
        },
    ))
}
#[tokio::test]
async fn confidential_webmail_rejects_before_body_and_preserves_unknown_intent() {
    let (_dir, control, router) = fixture();
    let denied = router
        .clone()
        .oneshot(request("/internal/webmail", "wrong", panic_body()))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert!(control.calls.lock().unwrap().is_empty());
    let token = "t".repeat(43);
    let unknown = json!({"action":"submit","token":token,"requestID":uuid::Uuid::new_v4(),"emailID":"draft1","identityID":"identity1"});
    let response = router
        .clone()
        .oneshot(request(
            "/internal/webmail",
            INTERNAL,
            Body::from(unknown.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body(response).await,
        json!({"id":"789","state":"unknown","filed":false,"filingState":null})
    );
    assert_eq!(control.calls.lock().unwrap().len(), 1);
    let response = router
        .oneshot(request(
            "/internal/webmail",
            INTERNAL,
            Body::from(
                json!({"action":"lookup","token":token,"emailID":"draft1","customerID":"foreign"})
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(control.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn smtp_hook_authenticates_first_and_binds_observed_digest_and_single_proof() {
    let (_dir, control, router) = fixture();
    let token = format!("wd_hook_123_{}", "h".repeat(43));
    control.allow.store(false, Ordering::SeqCst);
    let rejected = router
        .clone()
        .oneshot(request("/internal/smtp/123", &token, panic_body()))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    control.allow.store(true, Ordering::SeqCst);
    let proof = "p".repeat(43);
    let submission = format!("789.{proof}");
    let headers = vec![
        ("From", "sender@example.test"),
        ("To", "target@example.test"),
        ("X-Webdock-Submission", &submission),
    ];
    let hook = json!({"context":{"stage":"data","sasl":{"login":"submission-fixture"},"queue":{"id":"a12"}},"envelope":{"from":{"address":"sender@example.test"},"to":[{"address":"target@example.test"}]},"message":{"headers":headers,"contents":"hello\r\n","size":100}});
    let response = router
        .clone()
        .oneshot(request(
            "/internal/smtp/123",
            &token,
            Body::from(hook.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await["action"], "accept");
    let call = control.calls.lock().unwrap().last().unwrap().clone();
    assert_eq!(call["action"], "send.policy");
    assert_eq!(call["submission"]["id"], "789");
    assert_eq!(call["submission"]["proof"], proof);
    assert_eq!(call["submission"]["queueID"], "a12");
    let canonical = webdock_mail::proof::hook_digest(
        &headers
            .into_iter()
            .map(|(a, b)| (a.into(), b.into()))
            .collect::<Vec<_>>(),
        "hello\r\n",
        100,
    )
    .unwrap();
    assert_eq!(call["submission"]["digest"], canonical);
    let mut duplicated = hook;
    duplicated["message"]["headers"]
        .as_array_mut()
        .unwrap()
        .push(json!(["x-webdock-submission", format!("789.{proof}")]));
    let response = router
        .oneshot(request(
            "/internal/smtp/123",
            &token,
            Body::from(duplicated.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(body(response).await["action"], "reject");
    assert_eq!(
        control.calls.lock().unwrap().last().unwrap()["action"],
        "send.policy.auth"
    );
}
