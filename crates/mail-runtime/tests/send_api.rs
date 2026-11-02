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
use webdock_mail_runtime::{
    payload::PrivatePayloadStore,
    send_api::SendApi,
    send_control::{ControlError, SendControl},
};

struct Control {
    calls: Mutex<Vec<Value>>,
    lose_commit: AtomicBool,
    replay: AtomicBool,
    denied: AtomicBool,
}
#[async_trait::async_trait]
impl SendControl for Control {
    async fn call(&self, input: Value) -> Result<Value, ControlError> {
        self.calls.lock().unwrap().push(input.clone());
        Ok(match input["action"].as_str().unwrap() {
            "send.authenticate" => {
                if self.denied.load(Ordering::SeqCst) {
                    return Err(ControlError { status: 401 });
                }
                json!({"customerID":"12"})
            }
            "send.admit" => {
                if self.replay.load(Ordering::SeqCst) {
                    json!({"id":"789","state":"queued","created":false})
                } else {
                    json!({"id":"789","state":"staging","created":true,"customerID":"12","payloadRef":input["submission"]["payloadRef"],"uploadToken":uuid::Uuid::new_v4()})
                }
            }
            "send.commitPayload" => {
                if self.lose_commit.load(Ordering::SeqCst) {
                    return Err(ControlError::unavailable());
                }
                json!({"queued":true})
            }
            "send.status" => json!({"id":"789","state":"queued","private":"do not expose"}),
            _ => panic!("unexpected action"),
        })
    }
}
fn fixture() -> (
    tempfile::TempDir,
    Arc<PrivatePayloadStore>,
    Arc<Control>,
    axum::Router,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(
        PrivatePayloadStore::open(dir.path(), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .unwrap(),
    );
    let control = Arc::new(Control {
        calls: Mutex::new(Vec::new()),
        lose_commit: AtomicBool::new(false),
        replay: AtomicBool::new(false),
        denied: AtomicBool::new(false),
    });
    let router = SendApi::new(control.clone(), store.clone()).router();
    (dir, store, control, router)
}
fn request(body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/emails")
        .header(
            "authorization",
            format!("Bearer wd_mail_{}", "x".repeat(43)),
        )
        .header("content-type", "application/json")
        .header("idempotency-key", "same-request")
        .body(body)
        .unwrap()
}
fn message() -> Value {
    json!({"from":"Sender <sender@example.test>","to":"a@example.test","bcc":"hidden@example.test","subject":"Hello","text":"Body"})
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn staged_upload_hash_scope_replay_and_lost_ack_do_not_resend() {
    use sha2::{Digest, Sha256};
    let (_dir, store, control, router) = fixture();
    control.lose_commit.store(true, Ordering::SeqCst);
    let response = router
        .clone()
        .oneshot(request(Body::from(message().to_string())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(response).await["id"], "789");
    let admission = control
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|x| x["action"] == "send.admit")
        .unwrap()["submission"]
        .clone();
    let reference = admission["payloadRef"].as_str().unwrap();
    let payload = store.get("12", reference).unwrap();
    assert_eq!(
        payload.len(),
        admission["payloadBytes"].as_u64().unwrap() as usize
    );
    assert_eq!(
        hex::encode(Sha256::digest(&payload)),
        admission["requestHash"]
    );
    assert_eq!(
        admission["envelope"]["to"],
        json!(["a@example.test", "hidden@example.test"])
    );
    assert!(store.get("13", reference).is_err());
    control.replay.store(true, Ordering::SeqCst);
    let response = router
        .clone()
        .oneshot(request(Body::from(message().to_string())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(body(response).await, json!({"id":"789","state":"queued"}));
    assert_eq!(
        control
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|x| x["action"] == "send.commitPayload")
            .count(),
        1
    );
    assert!(store.get("12", reference).is_ok());
    let response = router
        .oneshot(
            Request::builder()
                .uri("/v1/emails/789")
                .header(
                    "authorization",
                    format!("Bearer wd_mail_{}", "x".repeat(43)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(body(response).await, json!({"id":"789","state":"queued"}));
}

#[tokio::test]
async fn authentication_precedes_body_poll_and_duplicate_authority_is_denied() {
    use futures_util::stream;
    let (_dir, _store, control, router) = fixture();
    let unread = Body::from_stream(stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<axum::body::Bytes, std::io::Error>>> {
            panic!("unauthenticated body was polled")
        },
    ));
    let mut req = request(unread);
    req.headers_mut().remove("authorization");
    assert_eq!(
        router.clone().oneshot(req).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let mut req = request(Body::empty());
    req.headers_mut()
        .append("authorization", "Bearer other".parse().unwrap());
    assert_eq!(
        router.clone().oneshot(req).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(control.calls.lock().unwrap().is_empty());
    let mut req = request(Body::from(message().to_string()));
    req.headers_mut()
        .append("idempotency-key", "second".parse().unwrap());
    assert_eq!(
        router.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let mut req = request(Body::from(message().to_string()));
    req.headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    assert_eq!(
        router.oneshot(req).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn one_upload_per_customer_releases_capacity_on_disconnect_and_serves_real_http() {
    let (_dir, _store, _control, router) = fixture();
    let polled = Arc::new(tokio::sync::Notify::new());
    let signal = polled.clone();
    let slow = Body::from_stream(futures_util::stream::poll_fn(
        move |_| -> std::task::Poll<Option<Result<axum::body::Bytes, std::io::Error>>> {
            signal.notify_one();
            std::task::Poll::Pending
        },
    ));
    let pending = tokio::spawn(router.clone().oneshot(request(slow)));
    tokio::time::timeout(std::time::Duration::from_secs(2), polled.notified())
        .await
        .unwrap();
    assert_eq!(
        router
            .clone()
            .oneshot(request(Body::from(message().to_string())))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let response = reqwest::Client::new()
        .post(format!("{origin}/v1/emails"))
        .bearer_auth(format!("wd_mail_{}", "x".repeat(43)))
        .json(&message())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"id":"789","state":"queued"})
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn revoked_source_is_denied_before_any_body_poll() {
    let (_dir, _store, control, router) = fixture();
    control.denied.store(true, Ordering::SeqCst);
    let unread = Body::from_stream(futures_util::stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<axum::body::Bytes, std::io::Error>>> {
            panic!("denied body was polled")
        },
    ));
    let response = router.oneshot(request(unread)).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()["connection"], "close");
}

#[tokio::test]
async fn native_ingress_keeps_scheduling_cc_bcc_and_attachment_payloads() {
    let (_dir, store, control, router) = fixture();
    let scheduled = (chrono::Utc::now() + chrono::Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let message = json!({"from":"sender@example.test","to":"to@example.test","cc":"cc@example.test","bcc":"bcc@example.test","subject":"Test","text":"Body","attachments":[{"filename":"a.txt","content":"aGk="}],"scheduled_at":scheduled});
    let response = router
        .oneshot(request(Body::from(message.to_string())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let admission = control
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|call| call["action"] == "send.admit")
        .unwrap()["submission"]
        .clone();
    assert_eq!(
        admission["envelope"]["to"],
        json!(["to@example.test", "cc@example.test", "bcc@example.test"])
    );
    assert!(admission["notBefore"].is_string());
    let payload = store
        .get("12", admission["payloadRef"].as_str().unwrap())
        .unwrap();
    let stored: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(
        stored["attachments"],
        json!([{"filename":"a.txt","content":"aGk="}])
    );
    assert_eq!(stored["scheduled_at"], admission["notBefore"]);
}
