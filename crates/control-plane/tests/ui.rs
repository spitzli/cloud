use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sea_orm::DatabaseConnection;
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    ui::UiAssets,
};
use webdock_core::config::PlatformConfig;
#[tokio::test]
async fn built_ui_reserves_apis_hosts_and_filesystem_boundaries() {
    let root = std::env::temp_dir().join(format!("webdock-ui-{}", uuid::Uuid::new_v4()));
    let dist = root.join("dist");
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::write(
        dist.join("index.html"),
        "<!doctype html><title>Account fixture</title>",
    )
    .unwrap();
    std::fs::write(dist.join("assets/index-ABC12345.css"), "body{color:black}").unwrap();
    std::fs::write(root.join("outside"), "do-not-serve").unwrap();
    std::os::unix::fs::symlink(root.join("outside"), dist.join("assets/escape.txt")).unwrap();
    let ui = UiAssets::new(
        &dist,
        &["https://plugin.example.invalid".into()],
        &["https://database.example.invalid".into()],
        true,
    )
    .unwrap();
    // Browser-called gateways the Core advertises must be reachable under this exact CSP.
    assert!(
        ui.require_connect("https://database.example.invalid")
            .is_ok()
    );
    assert!(
        ui.require_connect("https://DATABASE.example.invalid/")
            .is_ok()
    );
    assert!(
        ui.require_connect("https://mail-gateway.example.invalid")
            .is_err()
    );
    assert!(
        ui.require_connect("https://database.example.invalid:8443")
            .is_err()
    );
    assert!(ui.require_connect("not an origin").is_err());
    let app = build_app_with_domains(
        PlatformConfig::from_map(&HashMap::new()).unwrap(),
        DatabaseConnection::default(),
        Services {
            ui: Some(Arc::new(ui)),
            ..Default::default()
        },
    );
    for host in ["localhost:3125", "localhost:3120"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/account")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        let policy = response.headers()["content-security-policy"]
            .to_str()
            .unwrap();
        assert!(policy.contains("frame-src https://plugin.example.invalid"));
        assert!(policy.contains("connect-src 'self' https://database.example.invalid"));
        assert!(policy.contains("wss://database.example.invalid"));
        assert!(policy.contains("font-src 'self' data:"));
        assert!(policy.contains("worker-src 'self' blob:"));
        assert!(
            String::from_utf8(
                response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .to_vec()
            )
            .unwrap()
            .contains("Account fixture")
        );
    }
    for (host, path) in [
        ("localhost:3130", "/account"),
        ("untrusted.invalid", "/account"),
        ("localhost:3125", "/api/missing"),
        ("localhost:3125", "/v1/missing"),
        ("localhost:3125", "/.well-known/missing"),
        ("localhost:3125", "/%2e%2e/outside"),
        ("localhost:3125", "/assets/escape.txt"),
        ("localhost:3125", "/assets/missing.js"),
        ("localhost:3125", "/%61pi/missing"),
        ("localhost:3125", "/%252e%252e/outside"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{host}{path}");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&bytes).contains("Account fixture"));
        assert!(!String::from_utf8_lossy(&bytes).contains("do-not-serve"));
    }
    let asset = app
        .oneshot(
            Request::builder()
                .uri("/assets/index-ABC12345.css")
                .header("host", "localhost:3125")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        asset.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert!(
        UiAssets::new(
            &dist,
            &["https://user:pass@plugin.invalid".into()],
            &[],
            true
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}
