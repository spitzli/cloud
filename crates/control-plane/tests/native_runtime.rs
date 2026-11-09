use axum::{Router, routing::get};
use std::{collections::HashMap, time::Duration};
use tokio::sync::{Notify, oneshot};
use webdock_control_plane::{config::runtime_config, runtime::serve_with_shutdown};

#[test]
fn native_startup_keeps_explicit_pool_and_binding_policy() {
    let env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    let config = runtime_config(&env).unwrap();
    assert_eq!(config.database.min_connections, 1);
    assert_eq!(config.database.max_connections, 10);
    assert_eq!(config.database.connect_timeout, Duration::from_secs(5));
    assert_eq!(config.database.acquire_timeout, Duration::from_secs(30));
    assert_eq!(config.database.idle_timeout, Duration::from_secs(30));
    assert_eq!(config.server.binding, "127.0.0.1");
    assert_eq!(config.server.port, 3130);
    assert_eq!(config.server.shutdown_timeout, Duration::from_secs(20));
}

#[test]
fn native_id_backend_requires_an_explicit_supported_choice() {
    use webdock_control_plane::config::IdBackend;
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().id_backend,
        IdBackend::Legacy
    ));
    env.insert("WEBDOCK_ID_BACKEND".into(), "tokio-postgres".into());
    assert!(matches!(
        runtime_config(&env).unwrap().id_backend,
        IdBackend::Native
    ));
    env.insert("WEBDOCK_ID_BACKEND".into(), "typo".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_startup_errors_do_not_include_database_credentials() {
    let env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:never-show-this@127.0.0.1/db?host=foreign.invalid".into(),
    )]);
    let error = runtime_config(&env).err().unwrap();
    assert!(!format!("{error:?} {error}").contains("never-show-this"));
    assert!(!format!("{error}").contains("foreign.invalid"));
}

#[test]
fn native_binding_preserves_existing_bracketed_ipv6_addresses() {
    for (binding, expected) in [("[::]", "::"), ("[::1]", "::1"), ("::1", "::1")] {
        let env = HashMap::from([
            (
                "WEBDOCK_DATABASE_URL".into(),
                "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
            ),
            ("WEBDOCK_BIND_ADDRESS".into(), binding.into()),
        ]);
        assert_eq!(runtime_config(&env).unwrap().server.binding, expected);
    }
}

#[test]
fn retained_production_environment_still_enforces_tls_policy() {
    let mut env = HashMap::from([
        ("LOCO_ENV".into(), "production".into()),
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
            "WEBDOCK_DATABASE_URL".into(),
            "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
        ),
    ]);
    assert!(runtime_config(&env).is_err());
    env.insert(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test?sslmode=verify-full".into(),
    );
    assert!(runtime_config(&env).unwrap().platform.production);
}

#[tokio::test]
async fn direct_axum_delivers_requests_and_drains_on_shutdown() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, signal) = oneshot::channel();
    let task = tokio::spawn(serve_with_shutdown(
        listener,
        Router::new().route("/healthz", get(|| async { "ok" })),
        async {
            let _ = signal.await;
        },
        Duration::from_secs(1),
    ));
    let response = reqwest::get(format!("http://{address}/healthz"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "ok");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn shutdown_drains_inflight_work_but_does_not_wait_forever() {
    for bounded in [false, true] {
        let entered = std::sync::Arc::new(Notify::new());
        let release = std::sync::Arc::new(Notify::new());
        let route_entered = entered.clone();
        let route_release = release.clone();
        let app = Router::new().route(
            "/hold",
            get(move || {
                let entered = route_entered.clone();
                let release = route_release.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    "complete"
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, signal) = oneshot::channel();
        let task = tokio::spawn(serve_with_shutdown(
            listener,
            app,
            async {
                let _ = signal.await;
            },
            Duration::from_millis(50),
        ));
        let client = tokio::spawn(reqwest::get(format!("http://{address}/hold")));
        entered.notified().await;
        stop.send(()).unwrap();
        if bounded {
            let error = tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            release.notify_one();
            let _ = client.await;
        } else {
            release.notify_one();
            assert_eq!(
                client.await.unwrap().unwrap().text().await.unwrap(),
                "complete"
            );
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }
}

#[test]
fn native_identity_uncertainty_is_service_unavailability() {
    let response =
        webdock_control_plane::auth::failure(webdock_identity::IdentityError::Persistence);
    assert_eq!(response.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.code, "unavailable");
}

#[test]
fn native_session_read_backend_requires_explicit_opt_in() {
    use webdock_control_plane::config::SessionReadBackend;
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().session_read_backend,
        SessionReadBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_SESSION_READ_BACKEND".into(),
        "tokio-postgres".into(),
    );
    assert!(matches!(
        runtime_config(&env).unwrap().session_read_backend,
        SessionReadBackend::Native
    ));
    env.insert("WEBDOCK_SESSION_READ_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_session_management_is_explicit_and_shares_native_reads() {
    use webdock_control_plane::config::{SessionManagementBackend, SessionReadBackend};
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().session_management_backend,
        SessionManagementBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_SESSION_MANAGEMENT_BACKEND".into(),
        "tokio-postgres".into(),
    );
    let config = runtime_config(&env).unwrap();
    assert!(matches!(
        config.session_management_backend,
        SessionManagementBackend::Native
    ));
    assert!(matches!(
        config.session_read_backend,
        SessionReadBackend::Native
    ));
    env.insert("WEBDOCK_SESSION_READ_BACKEND".into(), "sea-orm".into());
    assert!(matches!(
        runtime_config(&env).unwrap().session_read_backend,
        SessionReadBackend::Native
    ));
    env.insert("WEBDOCK_SESSION_MANAGEMENT_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_browser_flow_storage_requires_explicit_opt_in() {
    use webdock_control_plane::config::BrowserFlowBackend;
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().browser_flow_backend,
        BrowserFlowBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_BROWSER_FLOW_BACKEND".into(),
        "tokio-postgres".into(),
    );
    assert!(matches!(
        runtime_config(&env).unwrap().browser_flow_backend,
        BrowserFlowBackend::Native
    ));
    env.insert("WEBDOCK_BROWSER_FLOW_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_oauth_client_store_requires_explicit_opt_in() {
    use webdock_control_plane::config::OAuthClientBackend;
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().oauth_client_backend,
        OAuthClientBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_OAUTH_CLIENT_BACKEND".into(),
        "tokio-postgres".into(),
    );
    assert!(matches!(
        runtime_config(&env).unwrap().oauth_client_backend,
        OAuthClientBackend::Native
    ));
    env.insert("WEBDOCK_OAUTH_CLIENT_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_oauth_introspection_requires_explicit_opt_in_and_implies_client_reads() {
    use webdock_control_plane::config::{OAuthClientBackend, OAuthIntrospectionBackend};
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().oauth_introspection_backend,
        OAuthIntrospectionBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_OAUTH_INTROSPECTION_BACKEND".into(),
        "tokio-postgres".into(),
    );
    let config = runtime_config(&env).unwrap();
    assert!(matches!(
        config.oauth_introspection_backend,
        OAuthIntrospectionBackend::Native
    ));
    assert!(matches!(
        config.oauth_client_backend,
        OAuthClientBackend::Native
    ));
    env.insert("WEBDOCK_OAUTH_INTROSPECTION_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_browser_authority_is_explicit_and_shares_native_dependencies() {
    use webdock_control_plane::config::{
        BrowserAuthorityBackend, BrowserFlowBackend, OAuthClientBackend, OAuthIntrospectionBackend,
    };
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().browser_authority_backend,
        BrowserAuthorityBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_BROWSER_AUTHORITY_BACKEND".into(),
        "tokio-postgres".into(),
    );
    let config = runtime_config(&env).unwrap();
    assert!(matches!(
        config.browser_authority_backend,
        BrowserAuthorityBackend::Native
    ));
    assert!(matches!(
        config.browser_flow_backend,
        BrowserFlowBackend::Native
    ));
    assert!(matches!(
        config.oauth_client_backend,
        OAuthClientBackend::Native
    ));
    assert!(matches!(
        config.oauth_introspection_backend,
        OAuthIntrospectionBackend::Native
    ));
    env.insert("WEBDOCK_BROWSER_AUTHORITY_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_oauth_security_is_explicit_and_implies_its_native_dependencies() {
    use webdock_control_plane::config::{
        IdBackend, OAuthClientBackend, OAuthIntrospectionBackend, OAuthSecurityBackend,
        SessionReadBackend,
    };
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().oauth_security_backend,
        OAuthSecurityBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_OAUTH_SECURITY_BACKEND".into(),
        "tokio-postgres".into(),
    );
    let c = runtime_config(&env).unwrap();
    assert!(matches!(
        c.oauth_security_backend,
        OAuthSecurityBackend::Native
    ));
    assert!(matches!(c.oauth_client_backend, OAuthClientBackend::Native));
    assert!(matches!(
        c.oauth_introspection_backend,
        OAuthIntrospectionBackend::Native
    ));
    assert!(matches!(c.id_backend, IdBackend::Native));
    assert!(matches!(c.session_read_backend, SessionReadBackend::Native));
    env.insert("WEBDOCK_OAUTH_SECURITY_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_oauth_grants_is_explicit_and_composes_native_authorities() {
    use webdock_control_plane::config::{
        IdBackend, OAuthGrantsBackend, OAuthIntrospectionBackend, OAuthSecurityBackend,
        SessionReadBackend,
    };
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().oauth_grants_backend,
        OAuthGrantsBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_OAUTH_GRANTS_BACKEND".into(),
        "tokio-postgres".into(),
    );
    let c = runtime_config(&env).unwrap();
    assert!(matches!(c.oauth_grants_backend, OAuthGrantsBackend::Native));
    assert!(matches!(
        c.oauth_security_backend,
        OAuthSecurityBackend::Native
    ));
    assert!(matches!(
        c.oauth_introspection_backend,
        OAuthIntrospectionBackend::Native
    ));
    assert!(matches!(c.id_backend, IdBackend::Native));
    assert!(matches!(c.session_read_backend, SessionReadBackend::Native));
    env.insert("WEBDOCK_OAUTH_GRANTS_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_oauth_tokens_is_explicit_and_composes_the_complete_owned_lifecycle() {
    use webdock_control_plane::config::{
        IdBackend, OAuthGrantsBackend, OAuthSecurityBackend, OAuthTokensBackend, SessionReadBackend,
    };
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().oauth_tokens_backend,
        OAuthTokensBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_OAUTH_TOKENS_BACKEND".into(),
        "tokio-postgres".into(),
    );
    let c = runtime_config(&env).unwrap();
    assert!(matches!(c.oauth_tokens_backend, OAuthTokensBackend::Native));
    assert!(matches!(c.oauth_grants_backend, OAuthGrantsBackend::Native));
    assert!(matches!(
        c.oauth_security_backend,
        OAuthSecurityBackend::Native
    ));
    assert!(matches!(c.id_backend, IdBackend::Native));
    assert!(matches!(c.session_read_backend, SessionReadBackend::Native));
    env.insert("WEBDOCK_OAUTH_TOKENS_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_mcp_authority_is_explicit_and_additive_to_existing_capabilities() {
    use webdock_control_plane::config::{
        McpAuthorityBackend, OAuthIntrospectionBackend, OAuthTokensBackend,
    };
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().mcp_authority_backend,
        McpAuthorityBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_MCP_AUTHORITY_BACKEND".into(),
        "tokio-postgres".into(),
    );
    assert!(matches!(
        runtime_config(&env).unwrap().mcp_authority_backend,
        McpAuthorityBackend::Native
    ));
    assert!(matches!(
        runtime_config(&env).unwrap().oauth_introspection_backend,
        OAuthIntrospectionBackend::Native
    ));
    env.insert(
        "WEBDOCK_OAUTH_TOKENS_BACKEND".into(),
        "tokio-postgres".into(),
    );
    assert!(matches!(
        runtime_config(&env).unwrap().oauth_tokens_backend,
        OAuthTokensBackend::Native
    ));
    env.insert("WEBDOCK_MCP_AUTHORITY_BACKEND".into(), "other".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_account_recovery_is_explicit_and_preserves_other_capabilities() {
    use webdock_control_plane::config::{
        AccountRecoveryBackend, IdBackend, SessionManagementBackend,
    };
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().account_recovery_backend,
        AccountRecoveryBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_ACCOUNT_RECOVERY_BACKEND".into(),
        "tokio-postgres".into(),
    );
    assert!(matches!(
        runtime_config(&env).unwrap().account_recovery_backend,
        AccountRecoveryBackend::Native
    ));
    assert!(matches!(
        runtime_config(&env).unwrap().id_backend,
        IdBackend::Native
    ));
    env.insert(
        "WEBDOCK_SESSION_MANAGEMENT_BACKEND".into(),
        "tokio-postgres".into(),
    );
    assert!(matches!(
        runtime_config(&env).unwrap().session_management_backend,
        SessionManagementBackend::Native
    ));
    env.insert("WEBDOCK_ACCOUNT_RECOVERY_BACKEND".into(), "unknown".into());
    assert!(runtime_config(&env).is_err());
}

#[test]
fn native_password_login_is_explicit_and_composes_its_required_capabilities() {
    use webdock_control_plane::config::{
        AccountRecoveryBackend, IdBackend, PasswordLoginBackend, SessionManagementBackend,
        SessionReadBackend,
    };
    let mut env = HashMap::from([(
        "WEBDOCK_DATABASE_URL".into(),
        "postgres://runtime:fixture@127.0.0.1/webdock_test".into(),
    )]);
    assert!(matches!(
        runtime_config(&env).unwrap().password_login_backend,
        PasswordLoginBackend::Legacy
    ));
    env.insert(
        "WEBDOCK_PASSWORD_LOGIN_BACKEND".into(),
        "tokio-postgres".into(),
    );
    let c = runtime_config(&env).unwrap();
    assert!(matches!(
        c.password_login_backend,
        PasswordLoginBackend::Native
    ));
    assert!(matches!(c.id_backend, IdBackend::Native));
    assert!(matches!(c.session_read_backend, SessionReadBackend::Native));
    assert!(matches!(
        c.session_management_backend,
        SessionManagementBackend::Native
    ));
    env.insert(
        "WEBDOCK_ACCOUNT_RECOVERY_BACKEND".into(),
        "tokio-postgres".into(),
    );
    assert!(matches!(
        runtime_config(&env).unwrap().account_recovery_backend,
        AccountRecoveryBackend::Native
    ));
    env.insert("WEBDOCK_PASSWORD_LOGIN_BACKEND".into(), "unknown".into());
    assert!(runtime_config(&env).is_err());
}
