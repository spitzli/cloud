use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tower::ServiceExt;
use webdock_control_plane::http::build_app_with_oauth;
use webdock_core::config::PlatformConfig;
use webdock_identity::{CookiePolicy, IdentityService, RlsSigner, SecretKeys};

fn target(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let url = url::Url::parse(&raw).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    raw
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    value: Value,
) -> (StatusCode, Value, Option<String>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "localhost:3120")
                .header("origin", "http://localhost:3120")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(value.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("webdock-auth.session_token="))
        .map(|v| v.split(';').next().unwrap().to_owned());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap(), cookie)
}

#[tokio::test]
#[ignore = "requires the explicitly owned restricted-role fixture"]
async fn native_http_setup_mfa_registry_audit_and_revocation_are_connected() {
    let owner = Database::connect(target("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    let auth_url = target("AUTH_DATABASE_URL");
    let db = Database::connect(&auth_url).await.unwrap();
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let identity = Arc::new(
        IdentityService::new(db.clone(), CookiePolicy::new(false, &secret).unwrap())
            .with_secret_keys(
                SecretKeys::new(
                    BTreeMap::from([(1, secret.clone())]),
                    1,
                    Some(secret.clone()),
                )
                .unwrap(),
            )
            .with_allocator(ids.clone()),
    );
    let email = format!("http-{}@example.invalid", uuid::Uuid::new_v4());
    let user = identity
        .create_local_account(&email, "HTTP fixture", "initial-test-password", true)
        .await
        .unwrap();
    // Only this disposable fixture confirms its synthetic mailbox; MFA is completed through HTTP below.
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
            [user.clone().into()],
        ))
        .await
        .unwrap();
    let mapping = ids.allocate().await.unwrap().to_string();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'HTTP fixture',$2,'operator',$3)",[mapping.into(),email.clone().into(),user.clone().into()])).await.unwrap();
    let registry = Arc::new(
        webdock_registry::RegistryService::new(
            Database::connect(target("WEBDOCK_REGISTRY_DATABASE_URL"))
                .await
                .unwrap(),
            RlsSigner::new(
                std::env::var("DATABASE_RLS_SECRET").unwrap(),
                "webdock_registry_runtime",
            )
            .unwrap(),
        )
        .with_allocator(ids.clone()),
    );
    let oauth = Arc::new(webdock_oauth::OAuthService::new(
        db.clone(),
        identity.clone(),
        ids,
        Arc::new(SecretKeys::new(BTreeMap::from([(1, secret.clone())]), 1, Some(secret)).unwrap()),
        webdock_oauth::OAuthConfig::new(
            "http://localhost:3125/api/auth",
            "http://localhost:3120/api/mcp",
            None,
        )
        .unwrap(),
    ));
    let oauth_client = oauth
        .provision_client(
            &owner,
            webdock_oauth::ClientRegistration {
                name: "HTTP fixture".into(),
                redirect_uris: vec!["https://client.example.invalid/callback".into()],
                post_logout_redirect_uris: vec![],
                scopes: vec!["openid".into(), "profile".into(), "email".into()],
                auth_method: webdock_oauth::ClientAuthMethod::ClientSecretPost,
                allow_refresh: false,
                skip_consent: false,
                purpose: webdock_oauth::ClientPurpose::OperatorOnly {
                    allowed_subjects: vec![user.clone()],
                },
            },
        )
        .await
        .unwrap();
    oauth.provision_signing_key(&owner).await.unwrap();
    let app = build_app_with_oauth(
        PlatformConfig::from_map(&HashMap::new()).unwrap(),
        db,
        Some(identity),
        Some(registry),
        Some(oauth),
    );
    let (status, _, cookie) = call(
        &app,
        "POST",
        "/api/console/auth/sign-in/email",
        "",
        json!({"email":email,"password":"initial-test-password"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = cookie.unwrap();
    // Complete the actual protocol over HTTP, with consent bound to this MFA session.
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.extend_pairs([
        ("client_id", oauth_client.client_id.as_str()),
        ("redirect_uri", "https://client.example.invalid/callback"),
        ("response_type", "code"),
        ("scope", "openid profile email"),
        ("state", "fixture-state"),
        (
            "code_challenge",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        ),
        ("code_challenge_method", "S256"),
    ]);
    let authorization_query = params.finish();
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/console/registry/customers",
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status,_,cookie)=call(&app,"POST","/api/console/auth/change-password",&cookie,json!({"currentPassword":"initial-test-password","newPassword":"updated-test-password","revokeOtherSessions":true})).await;
    assert_eq!(status, StatusCode::OK);
    let cookie = cookie.unwrap();
    let (status, enrollment, _) = call(
        &app,
        "POST",
        "/api/console/auth/two-factor/enable",
        &cookie,
        json!({"password":"updated-test-password"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let totp = totp_rs::Totp::from_url(enrollment["totpURI"].as_str().unwrap()).unwrap();
    let (status, _, cookie) = call(
        &app,
        "POST",
        "/api/console/auth/two-factor/verify-totp",
        &cookie,
        json!({"code":totp.generate_current().to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = cookie.unwrap();
    let authorize = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/auth/oauth2/authorize?{authorization_query}"))
                .header("host", "localhost:3125")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorize.status(), StatusCode::OK);
    let consent: Value =
        serde_json::from_slice(&authorize.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let consent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/oauth2/consent")
                .header("host", "localhost:3125")
                .header("origin", "http://localhost:3125")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"request_id":consent["consent"]["request_id"],"approved":true})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(consent.status(), StatusCode::SEE_OTHER);
    let redirect = url::Url::parse(consent.headers()["location"].to_str().unwrap()).unwrap();
    assert_eq!(
        redirect
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1,
        "fixture-state"
    );
    let code = redirect
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.extend_pairs([
        ("grant_type", "authorization_code"),
        ("client_id", oauth_client.client_id.as_str()),
        (
            "client_secret",
            oauth_client.client_secret.as_deref().unwrap(),
        ),
        ("code", code.as_str()),
        ("redirect_uri", "https://client.example.invalid/callback"),
        (
            "code_verifier",
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        ),
    ]);
    let token_form = form.finish();
    let token = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/oauth2/token")
                .header("host", "localhost:3125")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(token_form.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(token.status(), StatusCode::OK);
    assert_eq!(token.headers()["pragma"], "no-cache");
    let token: Value =
        serde_json::from_slice(&token.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(token["id_token"].is_string());
    let info = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/oauth2/userinfo")
                .header("host", "localhost:3125")
                .header(
                    "authorization",
                    format!("Bearer {}", token["access_token"].as_str().unwrap()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(info.status(), StatusCode::OK);
    let info: Value =
        serde_json::from_slice(&info.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(info["sub"], user);
    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/oauth2/token")
                .header("host", "localhost:3125")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(token_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    let (status, customer, _) = call(
        &app,
        "POST",
        "/api/console/registry/customers",
        &cookie,
        json!({"name":"HTTP integration customer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let organization = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            [customer["id"].as_str().unwrap().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "organization_id")
        .unwrap();
    let organization_number: u64 = organization.parse().unwrap();
    assert_eq!(
        (organization_number >> 12) & 1023,
        2,
        "implicit organization IDs must use the private native allocator"
    );
    let (status, project, _) = call(
        &app,
        "POST",
        "/api/console/registry/projects",
        &cookie,
        json!({"name":"HTTP integration project","customer":customer["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(project["id"].is_string());
    let (status, audit, _) = call(
        &app,
        "GET",
        &format!(
            "/api/console/registry/audit-events?targetID={}",
            project["id"].as_str().unwrap()
        ),
        &cookie,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(audit["totalDocs"], 1);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/console/registry/projects?limit=1&limit=2",
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/console/registry/audit-events",
            &cookie,
            json!({})
        )
        .await
        .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/console/auth/sign-out",
            &cookie,
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/console/registry/projects",
            &cookie,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",
            [user.into()],
        ))
        .await
        .unwrap();
}
