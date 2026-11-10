use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sea_orm::DatabaseConnection;
use spitzli_control_plane::{config::runtime_config, http::build_app};
use spitzli_core::config::PlatformConfig;
use std::collections::HashMap;
use tower::ServiceExt;

fn app() -> axum::Router {
    build_app(
        PlatformConfig::from_map(&HashMap::new()).unwrap(),
        DatabaseConnection::default(),
    )
}
fn request(path: &str) -> axum::http::request::Builder {
    Request::builder()
        .uri(path)
        .header("host", "localhost:3130")
}

#[tokio::test]
async fn liveness_and_readiness_have_different_database_semantics() {
    let live = app()
        .oneshot(request("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(live.headers()["cache-control"], "no-store");
    assert!(live.headers().contains_key("x-request-id"));
    let ready = app()
        .oneshot(request("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn rejects_unknown_and_forwarded_authorities() {
    for builder in [
        Request::builder()
            .uri("/healthz")
            .header("host", "attacker.example"),
        request("/healthz").header("x-forwarded-host", "localhost:3130"),
    ] {
        let response = app()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
        ));
    }
}

#[tokio::test]
async fn duplicate_authority_headers_are_denied_before_machine_scope_admission() {
    for name in ["host", "origin", "authorization"] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/mail/gateway")
            .header("host", "localhost:3125")
            .header("content-type", "application/json");
        if name != "host" {
            request = request.header(name, "first");
        }
        request = request.header(name, "second");
        let response = app()
            .oneshot(request.body(Body::from("{}")).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let request_id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["requestId"], request_id);
    }
}

#[tokio::test]
async fn malformed_origin_is_not_treated_as_an_absent_machine_origin() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/mail/gateway")
                .header("host", "localhost:3125")
                .header(
                    "origin",
                    axum::http::HeaderValue::from_bytes(b"\xff").unwrap(),
                )
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn console_configuration_never_leaks_backend_credentials() {
    let response = app()
        .oneshot(request("/console-config.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"apiOrigin":"http://localhost:3130","workspaceApiOrigin":"http://localhost:3130","authOrigin":"http://localhost:3125","enabledFeatures":[],"localePreference":"system","databaseOrderingEnabled":false})
    );
}

#[tokio::test]
async fn database_order_retirement_does_not_hide_existing_database_access() {
    for enabled in [false, true] {
        let config = PlatformConfig::from_map(&HashMap::from([
            (
                "WEBDOCK_FEATURES".into(),
                r#"{"databases":"enabled"}"#.into(),
            ),
            (
                "WEBDOCK_DATABASE_NEW_ORDERS_ENABLED".into(),
                enabled.to_string(),
            ),
        ]))
        .unwrap();
        let response = build_app(config, DatabaseConnection::default())
            .oneshot(request("/console-config.json").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["databaseOrderingEnabled"], enabled);
        assert_eq!(json["enabledFeatures"], serde_json::json!(["databases"]));
    }
}

#[test]
fn native_database_boot_keeps_explicit_runtime_policy() {
    let env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    let config = runtime_config(&env).unwrap();
    assert_eq!(config.database.min_connections, 1);
    assert_eq!(config.database.max_connections, 10);
    assert_eq!(config.server.binding, "127.0.0.1");
    assert!(runtime_config(&HashMap::new()).is_err());
}

#[test]
fn database_configuration_rejects_query_authority_overrides_and_tls_downgrades() {
    for suffix in [
        "?host=elsewhere.invalid",
        "?dbname=other",
        "?sslmode=verify-full&ssl-mode=disable",
    ] {
        let env = HashMap::from([(
            "WEBDOCK_DATABASE_URL".into(),
            format!("postgres://runtime:fixture@127.0.0.1/webdock_test{suffix}"),
        )]);
        assert!(runtime_config(&env).is_err(), "query override was accepted");
    }
}

#[tokio::test]
async fn browser_facade_fails_closed_until_identity_is_configured() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/api/console/session")
                .header("host", "localhost:3120")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let wrong_host = app()
        .oneshot(request("/api/console/session").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(wrong_host.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn console_origin_uses_only_the_same_origin_facade() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/console-config.json")
                .header("host", "localhost:3120")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let config: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(config["browserFacade"], true);
    assert_eq!(config["apiOrigin"], "http://localhost:3120");
    assert_eq!(config["workspaceApiOrigin"], "http://localhost:3130");
    assert_eq!(config["authOrigin"], "http://localhost:3125");
}

#[tokio::test]
async fn oauth_credentials_do_not_disable_browser_csrf_or_host_boundaries() {
    for (host, path, origin, status) in [
        (
            "localhost:3125",
            "/api/auth/oauth2/token",
            None,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "localhost:3130",
            "/api/auth/oauth2/token",
            None,
            StatusCode::NOT_FOUND,
        ),
        (
            "localhost:3125",
            "/api/auth/oauth2/token",
            Some("https://attacker.invalid"),
            StatusCode::FORBIDDEN,
        ),
        (
            "localhost:3125",
            "/api/auth/sign-out",
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "localhost:3125",
            "/api/auth/sign-out",
            Some("http://localhost:3120"),
            StatusCode::FORBIDDEN,
        ),
        (
            "localhost:3125",
            "/api/auth/oauth2/consent",
            None,
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("host", host)
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(origin) = origin {
            builder = builder.header("origin", origin);
        }
        let response = app().oneshot(builder.body(Body::from("client_id=fixture&grant_type=authorization_code&code=fixture&redirect_uri=https%3A%2F%2Fclient.invalid&code_verifier=fixture")).unwrap()).await.unwrap();
        assert_eq!(response.status(), status, "{host} {path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/.well-known/openid-configuration")
                .header("host", "localhost:3125")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn passkey_challenges_require_the_canonical_auth_origin() {
    for origin in [
        None,
        Some("http://localhost:3120"),
        Some("https://attacker.invalid"),
    ] {
        let mut request = Request::builder()
            .uri("/api/auth/passkey/generate-authenticate-options")
            .header("host", "localhost:3125");
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = app()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn gateway_credentials_keep_canonical_host_and_browser_origin_checks() {
    for gateway in ["/api/databases/gateway", "/api/mail/gateway"] {
        let wrong_host = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(gateway)
                    .header("host", "localhost:3130")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_host.status(), StatusCode::NOT_FOUND);
        let machine = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(gateway)
                    .header("host", "localhost:3125")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            machine.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "exact machine route reaches configured-service admission"
        );
        let browser = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(gateway)
                    .header("host", "localhost:3125")
                    .header("origin", "https://attacker.invalid")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(browser.status(), StatusCode::FORBIDDEN);
    }
}
#[tokio::test]
async fn language_cookie_is_host_only_and_requires_exact_browser_origin() {
    let make = |origin: &str| {
        Request::builder()
            .method("POST")
            .uri("/api/locale")
            .header("host", "localhost:3120")
            .header("origin", origin)
            .header("content-type", "application/json")
            .body(Body::from("{\"preference\":\"de\"}"))
            .unwrap()
    };
    let response = app().oneshot(make("http://localhost:3120")).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cookie.contains("HttpOnly")
            && cookie.contains("SameSite=Lax")
            && !cookie.contains("Domain=")
    );
    let other = app().oneshot(make("http://localhost:3125")).await.unwrap();
    assert_eq!(other.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn realtime_is_opt_in_and_capabilities_are_not_public_configuration() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/api/console/realtime/capability")
                .header("host", "localhost:3120")
                .method("POST")
                .header("origin", "http://localhost:3120")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"topic":"workspace"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn public_waitlist_submission_does_not_redirect_to_sign_in() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/api/console/waitlist")
                .method("POST")
                .header("host", "localhost:3120")
                .header("origin", "http://localhost:3120")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"email":"u@example.test","privacyAccepted":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!response.headers().contains_key("location"));
}
