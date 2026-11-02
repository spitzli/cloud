use super::*;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use std::collections::HashMap;
use tower::ServiceExt;
use webdock_control_plane::http::{Services, build_app_with_domains};
use webdock_core::config::PlatformConfig;

async fn app(fixture: &Fixture, enabled: bool) -> (Router, String) {
    let cookies = CookiePolicy::new(false, "fixture-hosting-cookie-key-at-least-32-bytes").unwrap();
    let cookie = format!(
        "{}={}",
        cookies.name(),
        cookies.sign(&format!("{}-token", fixture.nonce)).unwrap()
    );
    let identity = Arc::new(IdentityService::new(db("AUTH_DATABASE_URL").await, cookies));
    let config = PlatformConfig::from_map(&HashMap::from([(
        "WEBDOCK_FEATURES".into(),
        json!({"hosting": if enabled {"enabled"} else {"disabled"}}).to_string(),
    )]))
    .unwrap();
    (
        build_app_with_domains(
            config,
            DatabaseConnection::default(),
            Services {
                identity: Some(identity),
                hosting: Some(fixture.service.clone()),
                ..Default::default()
            },
        ),
        cookie,
    )
}

async fn read(app: &Router, id: &str, cookie: &str, host: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/console/hosting/reads/{id}"))
                .header("host", host)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
#[ignore = "requires the explicitly owned restricted-role PostgreSQL fixture"]
async fn browser_read_polling_is_scoped_typed_and_redacted() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let outcome = tokio::spawn(async move {
        f.configure("k3s").await;
        let created = f.service.execute(&f.principal, json!({
            "action":"apps.create", "projectID":f.pid, "name":"http-read",
            "spec":spec(), "environment":[{"name":"TOKEN","value":"private-http-log-secret"}],
            "subscriptionRevision":1,"idempotencyKey":"http-read-create-1"
        })).await.unwrap();
        let initial = f
            .service
            .agent_request(&f.agent, "claim", json!({}))
            .await
            .unwrap();
        f.proof(&initial, "http-read-owned-uid").await;
        let requested = f
            .service
            .execute(
                &f.principal,
                json!({
                    "action":"apps.logs","appID":created["appID"],
                    "idempotencyKey":"http-read-logs-1"
                }),
            )
            .await
            .unwrap();
        let id = requested["id"].as_str().unwrap();
        let (router, cookie) = app(&f, true).await;
        let (status, queued) = read(&router, id, &cookie, "localhost:3120").await;
        assert_eq!(status, StatusCode::OK, "{queued}");
        assert_eq!(queued["status"], "queued");
        assert!(queued["result"].is_null());
        assert_eq!(
            read(&router, id, "", "localhost:3120").await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            read(&router, id, &cookie, "localhost:3125").await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            read(&router, "invalid", &cookie, "localhost:3120").await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            read(
                &router,
                created["id"].as_str().unwrap(),
                &cookie,
                "localhost:3120"
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        let command = f.service.claim_read(&f.agent).await.unwrap().unwrap();
        assert_eq!(
            read(&router, id, &cookie, "localhost:3120").await.1["status"],
            "running"
        );
        f.service
            .complete_read(
                &f.agent,
                &command,
                json!({
                    "logs":"TOKEN=private-http-log-secret safe", "truncated":false
                }),
            )
            .await
            .unwrap();
        let (status, complete) = read(&router, id, &cookie, "localhost:3120").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(complete["status"], "succeeded");
        assert_eq!(complete["result"]["logs"], "TOKEN=[redacted] safe");
        assert!(!complete.to_string().contains("private-http-log-secret"));
        assert!(complete.get("desired").is_none());
        assert!(complete.get("nativeReadReceipt").is_none());
        let (disabled, _) = app(&f, false).await;
        assert_eq!(
            read(&disabled, id, &cookie, "localhost:3120").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        sql(
            &f.owner,
            "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
            vec![f.uid.clone().into()],
        )
        .await;
        let without_customer_access = read(&router, id, &cookie, "localhost:3120").await;
        sql(
            &f.owner,
            "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",
            vec![f.uid.clone().into()],
        )
        .await;
        assert_eq!(without_customer_access.0, StatusCode::NOT_FOUND);
        sql(
            &f.owner,
            "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) SELECT $1,$2,customer_id,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$3",
            vec![f.sid.clone().into(), f.uid.clone().into(), f.cid.clone().into()],
        )
        .await;
        let preview = read(&router, id, &cookie, "localhost:3120").await;
        sql(
            &f.owner,
            "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1 AND actor_id=$2",
            vec![f.sid.clone().into(), f.uid.clone().into()],
        )
        .await;
        assert_eq!(preview.0, StatusCode::FORBIDDEN);
        sql(
            &f.owner,
            "UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",
            vec![f.uid.clone().into()],
        )
        .await;
        assert_eq!(
            read(&router, id, &cookie, "localhost:3120").await.0,
            StatusCode::UNAUTHORIZED
        );
    })
    .await;
    fixture.cleanup().await;
    outcome.unwrap();
}
