use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use futures_util::stream;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tower::ServiceExt;
use webdock_mail_runtime::{ControlError, SendControl, gateway::Gateway};
struct Deny(AtomicUsize);
#[async_trait::async_trait]
impl SendControl for Deny {
    async fn call(&self, v: Value) -> Result<Value, ControlError> {
        assert_eq!(v["action"], "send.authenticate");
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ControlError { status: 401 })
    }
}
#[tokio::test]
async fn gateway_authenticates_before_public_body_and_never_forwards_internal_or_query_paths() {
    let control = Arc::new(Deny(AtomicUsize::new(0)));
    let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
    let router = Gateway::new(control.clone(), channel).router();
    for path in [
        "/v1/emails",
        "/v1/emails?x=1",
        "/api/mail/gateway",
        "/v1/emails/123?x=1",
    ] {
        let polled = Arc::new(AtomicBool::new(false));
        let seen = polled.clone();
        let body = Body::from_stream(stream::poll_fn(move |_| {
            seen.store(true, Ordering::SeqCst);
            std::task::Poll::Ready(Some(Err::<Vec<u8>, _>(std::io::Error::other(
                "must not poll",
            ))))
        }));
        let result = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(
                        "authorization",
                        format!("Bearer wd_mail_{}", "x".repeat(43)),
                    )
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            result.status(),
            if path == "/v1/emails" {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::NOT_FOUND
            }
        );
        assert!(!polled.load(Ordering::SeqCst));
        assert_eq!(result.headers()["cache-control"], "no-store");
    }
    assert_eq!(control.0.load(Ordering::SeqCst), 1);
    let _ = json!({});
}

struct Authenticate;
#[async_trait::async_trait]
impl SendControl for Authenticate {
    async fn call(&self, v: Value) -> Result<Value, ControlError> {
        assert_eq!(v["action"], "send.authenticate");
        Ok(json!({"customerID":"101"}))
    }
}
#[tokio::test]
async fn streaming_proxy_preserves_native_oversized_body_status() {
    // Local plaintext HTTP/2 backend stand-in only; production constructor uses pinned mTLS.
    let backend = axum::Router::new().route(
        "/v1/emails",
        axum::routing::post(|request: axum::extract::Request| async move {
            match axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await {
                Ok(_) => StatusCode::ACCEPTED,
                Err(_) => StatusCode::PAYLOAD_TOO_LARGE,
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_routes(tonic::service::Routes::from(backend))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let router = Gateway::new(Arc::new(Authenticate), channel).router();
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/emails")
                .header(
                    "authorization",
                    format!("Bearer wd_mail_{}", "x".repeat(43)),
                )
                .body(Body::from(vec![b'x'; 16 * 1024 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    server.abort();
}
