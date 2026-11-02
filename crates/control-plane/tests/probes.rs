//! Render the native Helm probes, then send their exact host/path to the real router.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use sea_orm::DatabaseConnection;
use std::{collections::HashMap, process::Command};
use tower::ServiceExt;
use webdock_control_plane::http::{Services, build_app_with_domains};
use webdock_core::config::PlatformConfig;

#[tokio::test]
async fn rendered_native_probes_pass_the_canonical_host_boundary() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("helm")
        .current_dir(root)
        .args([
            "template",
            "fixture",
            "deploy/helm/webdock",
            "--set-string",
            &format!("image=example.invalid/core@sha256:{}", "a".repeat(64)),
            "--set-string",
            "existingSecret=fixture",
            "--set-string",
            "apiHost=api.example.test",
        ])
        .output()
        .expect("Helm is required for native deployment verification");
    assert!(
        output.status.success(),
        "chart must render with complete fixture configuration"
    );
    let objects: Vec<serde_json::Value> =
        serde_saphyr::from_multiple(&String::from_utf8(output.stdout).unwrap()).unwrap();
    let deployment = objects.iter().find(|o| o["kind"] == "Deployment").unwrap();
    let core = &deployment["spec"]["template"]["spec"]["containers"][0];
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
            "WEBDOCK_STUDIO_ORIGIN".into(),
            "https://studio.example.test".into(),
        ),
        (
            "WEBDOCK_CONSOLE_ORIGIN".into(),
            "https://console.example.test".into(),
        ),
    ]))
    .unwrap();
    let app = build_app_with_domains(config, DatabaseConnection::default(), Services::default());
    for name in ["startupProbe", "livenessProbe", "readinessProbe"] {
        let probe = &core[name]["httpGet"];
        let path = probe["path"].as_str().unwrap();
        let host = probe["httpHeaders"]
            .as_array()
            .and_then(|headers| headers.iter().find(|h| h["name"] == "Host"))
            .and_then(|h| h["value"].as_str())
            .unwrap_or("10.42.0.10:3130");
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
        // The deliberately unconfigured database fails readiness, after passing host admission.
        assert_eq!(
            response.status(),
            if path == "/readyz" {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            },
            "{name}"
        );
    }
}
