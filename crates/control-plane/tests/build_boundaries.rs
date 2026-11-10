//! Mounted native routes retain host/origin and tenant authentication boundaries.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use sea_orm::DatabaseConnection;
use spitzli_control_plane::http::{Services, build_app_with_domains};
use spitzli_core::config::PlatformConfig;
use std::collections::HashMap;
use tower::ServiceExt;
fn router() -> axum::Router {
    let config = PlatformConfig::from_map(&HashMap::from([
        (
            "WEBDOCK_API_ORIGIN".into(),
            "https://api.example.test".into(),
        ),
        (
            "WEBDOCK_AUTH_ORIGIN".into(),
            "https://auth.example.test".into(),
        ),
        (
            "WEBDOCK_CONSOLE_ORIGIN".into(),
            "https://console.example.test".into(),
        ),
        (
            "WEBDOCK_STUDIO_ORIGIN".into(),
            "https://studio.example.test".into(),
        ),
        (
            "WEBDOCK_FEATURES".into(),
            "{\"hosting\":\"enabled\"}".into(),
        ),
    ]))
    .unwrap();
    let cookies = spitzli_identity::CookiePolicy::new(
        false,
        "owned-build-boundaries-cookie-key-at-least-32-bytes",
    )
    .unwrap();
    let identity = std::sync::Arc::new(spitzli_identity::IdentityService::new(
        DatabaseConnection::default(),
        cookies,
    ));
    build_app_with_domains(
        config,
        DatabaseConnection::default(),
        Services {
            identity: Some(identity),
            ..Services::default()
        },
    )
}
#[tokio::test]
async fn logs_and_infrastructure_never_allow_anonymous_or_originless_mutation() {
    for path in [
        "/api/console/hosting/projects/123/builds/456/logs",
        "/api/console/hosting/projects/123/builds/456/logs/events",
    ] {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "console.example.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let response=router().oneshot(Request::builder().method("POST").uri("/api/console/hosting/build-infrastructure").header("host","console.example.test").header("content-type","application/json").body(Body::from("{\"action\":\"admission\",\"enabled\":true,\"globalSlots\":1,\"tenantSlots\":1}")).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn only_the_exact_signed_webhook_path_is_exempt_from_browser_origin() {
    let request = |path| {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("host", "api.example.test")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap()
    };
    let response = router()
        .oneshot(request("/api/integrations/github/webhook"))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "unconfigured webhooks cannot claim success"
    );
    let response = router()
        .oneshot(request("/api/integrations/github/webhook/other"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
