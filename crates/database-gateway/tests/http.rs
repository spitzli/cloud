use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;
use webdock_database_gateway::{
    Error, Result,
    control::{Access, Control, Launch},
    server::{Gateway, Options},
};
struct DeniedControl;
#[async_trait::async_trait]
impl Control for DeniedControl {
    async fn exchange(&self, _: &str) -> Result<Launch> {
        Err(Error::new(401, "Launch code expired or was already used."))
    }
    async fn inspect(&self, _: &str) -> Result<Access> {
        Err(Error::new(401, "Access revoked."))
    }
    async fn logout(&self, _: &str) -> Result<()> {
        Ok(())
    }
}
fn gateway() -> Arc<Gateway> {
    Gateway::new(Options {
        public_origin: "http://localhost:3137".into(),
        allowed_origins: vec!["https://studio.example.invalid".into()],
        runtime_origins: vec![],
        expected_commit: Some("fixture".into()),
        control: Arc::new(DeniedControl),
        managed: None,
    })
    .unwrap()
}
#[tokio::test]
async fn browser_gateway_enforces_origin_json_and_single_use_launch_contract() {
    let app = gateway().router();
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    for (origin, body, status) in [
        ("https://attacker.invalid", "{}", StatusCode::FORBIDDEN),
        (
            "https://studio.example.invalid",
            "{\"code\":\"bad\"}",
            StatusCode::BAD_REQUEST,
        ),
        (
            "https://studio.example.invalid",
            "{\"code\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}",
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/session")
                    .header("host", "localhost:3137")
                    .header("origin", origin)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/s/123/api/v1/session")
                .header("host", "localhost:3137")
                .header("origin", "https://studio.example.invalid")
                .header("cookie", "webdock_database=a; webdock_database=b")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
