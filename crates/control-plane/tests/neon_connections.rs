use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use spitzli_control_plane::http::{Services, build_app, build_app_with_domains};
use spitzli_core::config::PlatformConfig;
use spitzli_identity::{CookiePolicy, IdentityService};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tower::ServiceExt;

#[tokio::test]
async fn neon_endpoint_authenticates_before_polling_and_preserves_origin_gate() {
    use tokio_stream::StreamExt;
    let polled = Arc::new(AtomicBool::new(false));
    let seen = polled.clone();
    let body = Body::from_stream(tokio_stream::iter([()]).map(move |()| {
        seen.store(true, Ordering::SeqCst);
        Ok::<_, std::io::Error>(vec![b'x'; 100_000])
    }));
    let app = build_app(
        PlatformConfig::from_map(&HashMap::new()).unwrap(),
        DatabaseConnection::default(),
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/console/byok/neon")
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
    assert!(!polled.load(Ordering::SeqCst));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/console/byok/neon")
                .header("host", "localhost:3120")
                .header("origin", "https://foreign.invalid")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
fn guarded(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let u = url::Url::parse(&raw).unwrap();
    assert_eq!(u.host_str(), Some("127.0.0.1"));
    assert_eq!(u.path(), "/webdock_rls_test");
    assert!(u.query().is_none());
    raw
}
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
async fn call(app: &Router, cookie: &str, value: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/console/byok/neon")
                .header("host", "localhost:3120")
                .header("origin", "http://localhost:3120")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(value.to_string()))
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
#[ignore = "requires a fresh owned restricted-role Core PostgreSQL fixture"]
async fn native_http_neon_customer_boundary_strict_input_and_disconnect_replay() {
    let owner = Database::connect(guarded("SPITZLI_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    let auth_url = guarded("AUTH_DATABASE_URL");
    let auth = Database::connect(&auth_url).await.unwrap();
    let runtime = Database::connect(guarded("DATABASE_URL")).await.unwrap();
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let cookies = CookiePolicy::new(false, &secret).unwrap();
    let identity = Arc::new(IdentityService::new(
        auth.clone(),
        CookiePolicy::new(false, &secret).unwrap(),
    ));
    let ids = spitzli_persistence::IdAllocator::connect(
        &auth_url,
        spitzli_ids::Identity::platform(0, 2).unwrap(),
    )
    .await
    .unwrap();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut customers = Vec::new();
    let mut orgs = Vec::new();
    for _ in 0..2 {
        let customer = ids.allocate().await.unwrap().to_string();
        let org = ids.allocate().await.unwrap().to_string();
        sql(&owner,"WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation',jsonb_build_object('customer_id',$1::text,'organization_id',$2::text)::text,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'HTTP Neon fixture' FROM hint",vec![customer.clone().into(),org.clone().into()]).await;
        customers.push(customer);
        orgs.push(org);
    }
    let mut actor_cookies = Vec::new();
    let mut actor_ids = Vec::new();
    for role in ["owner", "admin", "member", "operator"] {
        let user = format!("neon-http-{role}-{nonce}");
        let session = format!("{user}-session");
        let user_role = if role == "operator" {
            "operator"
        } else {
            "user"
        };
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,$3,true,false)",vec![user.clone().into(),format!("{user}@example.invalid").into(),user_role.into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$1,$2,now()+interval '1 hour',now())",vec![session.clone().into(),user.clone().into()]).await;
        if role == "operator" {
            sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$2)",vec![ids.allocate().await.unwrap().to_string().into(),user.clone().into(),format!("{user}@example.invalid").into()]).await;
        } else {
            sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,$4,now())",vec![ids.allocate().await.unwrap().to_string().into(),orgs[0].clone().into(),user.clone().into(),role.into()]).await;
        }
        actor_cookies.push(format!(
            "{}={}",
            cookies.name(),
            cookies.sign(&session).unwrap()
        ));
        actor_ids.push((user, session));
    }
    let service = Arc::new(
        spitzli_control_plane::neon_connections::NeonConnections::new(
            runtime,
            auth.clone(),
            identity.clone(),
            None,
            &std::env::var("DATABASE_RLS_SECRET").unwrap(),
            &secret,
        )
        .unwrap(),
    );
    let app = build_app_with_domains(
        PlatformConfig::from_map(&HashMap::new()).unwrap(),
        auth,
        Services {
            identity: Some(identity),
            neon_connections: Some(service),
            ..Services::default()
        },
    );
    let customer = &customers[0];
    for cookie in &actor_cookies[..2] {
        let (status, value) =
            call(&app, cookie, json!({"action":"get","customerID":customer})).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        assert_eq!(value["data"], json!({"connection":null,"canManage":true}));
    }
    assert_eq!(
        call(
            &app,
            &actor_cookies[2],
            json!({"action":"get","customerID":customer})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            &actor_cookies[0],
            json!({"action":"get","customerID":customers[1]})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for value in [
        json!({"action":"get","customerID":customer,"apiKey":"never-store"}),
        json!({"action":"connect","customerID":customer,"apiKey":"fixture","expectedRevision":0}),
        json!({"action":"disconnect","customerID":customer,"expectedRevision":-1,"idempotencyKey":"http-disconnect-001"}),
    ] {
        assert_eq!(
            call(&app, &actor_cookies[0], value).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let command = json!({"action":"disconnect","customerID":customer,"expectedRevision":0,"idempotencyKey":"http-disconnect-001"});
    let (status, receipt) = call(&app, &actor_cookies[0], command.clone()).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["data"]["connection"]["status"], "disconnected");
    assert_eq!(receipt["data"]["connection"]["revision"], 1);
    assert_eq!(
        call(&app, &actor_cookies[0], command.clone()).await,
        (StatusCode::OK, receipt)
    );
    let mut changed = command.clone();
    changed["expectedRevision"] = json!(1);
    assert_eq!(
        call(&app, &actor_cookies[0], changed).await.0,
        StatusCode::CONFLICT
    );
    sql(&owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES($1,$2,$3,$4)",vec![actor_ids[3].1.clone().into(),actor_ids[3].0.clone().into(),customer.clone().into(),orgs[0].clone().into()]).await;
    let (status, preview) = call(
        &app,
        &actor_cookies[3],
        json!({"action":"get","customerID":customer}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["data"]["canManage"], false);
    assert_eq!(
        call(&app, &actor_cookies[3], command).await.0,
        StatusCode::FORBIDDEN
    );
}
