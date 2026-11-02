use std::collections::HashMap;
use webdock_control_plane::{mail::native_control_configured, plugin_config::MailConfig};
#[test]
fn required_native_service_is_unavailable_when_listener_feature_is_absent() {
    let mail = MailConfig {
        signing_key: "/not-read".into(),
        worker_id: "legacy".into(),
        worker_generation: 1,
    };
    let mut env = HashMap::from([("WEBDOCK_NATIVE_MAIL_SENDING_ENABLED".into(), "true".into())]);
    assert!(!native_control_configured(Some(&mail), &env));
    env.insert("WEBDOCK_FEATURES".into(), r#"{"mail":"disabled"}"#.into());
    assert!(!native_control_configured(Some(&mail), &env));
    env.insert("WEBDOCK_FEATURES".into(), r#"{"mail":"enabled"}"#.into());
    assert!(native_control_configured(Some(&mail), &env));
    assert!(!native_control_configured(None, &env));
    env.insert("WEBDOCK_NATIVE_MAIL_SENDING_ENABLED".into(), "false".into());
    assert!(!native_control_configured(Some(&mail), &env));
}

#[tokio::test]
async fn console_mail_admission_authenticates_before_polling_a_large_body() {
    use axum::{
        body::{Body, Bytes},
        http::{Request, StatusCode},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tower::ServiceExt;
    let polled = Arc::new(AtomicBool::new(false));
    let seen = polled.clone();
    use tokio_stream::StreamExt;
    let body = Body::from_stream(tokio_stream::iter([()]).map(move |()| {
        seen.store(true, Ordering::SeqCst);
        Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 100_000]))
    }));
    let app = webdock_control_plane::http::build_app(
        webdock_core::config::PlatformConfig::from_map(&HashMap::new()).unwrap(),
        sea_orm::DatabaseConnection::default(),
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/console/plugins/invoke")
                .header("host", "localhost:3120")
                .header("origin", "http://localhost:3120")
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::SERVICE_UNAVAILABLE
    ));
    assert!(
        !polled.load(Ordering::SeqCst),
        "unauthenticated request body was polled"
    );
}
