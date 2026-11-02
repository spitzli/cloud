use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use sea_orm::DatabaseConnection;
use std::collections::HashMap;
use tower::ServiceExt;
use webdock_control_plane::http::build_app;
use webdock_core::config::PlatformConfig;

#[tokio::test]
async fn public_resources_require_bearer_on_retained_studio_host() {
    let app = build_app(
        PlatformConfig::from_map(&HashMap::new()).unwrap(),
        DatabaseConnection::default(),
    );
    for path in [
        "/api/registry",
        "/api/registry/customers",
        "/api/hosting/clusters",
        "/api/mcp",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "localhost:3120")
                    .header("cookie", "better-auth.session_token=not-a-bearer")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert!(
            response.headers().contains_key("www-authenticate"),
            "{path}"
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        if path == "/api/mcp" {
            assert!(
                response.headers()["www-authenticate"]
                    .to_str()
                    .unwrap()
                    .contains("scope=\"webdock:read\"")
            );
        }
    }
}

#[tokio::test]
async fn resource_host_and_origin_checks_precede_bearer_configuration() {
    let app = build_app(
        PlatformConfig::from_map(&HashMap::new()).unwrap(),
        DatabaseConnection::default(),
    );
    for (host, origin, status) in [
        ("localhost:3130", None, StatusCode::NOT_FOUND),
        ("localhost:3125", None, StatusCode::NOT_FOUND),
        (
            "localhost:3120",
            Some("https://untrusted.example.invalid"),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut builder = Request::builder()
            .uri("/api/registry/customers")
            .header("host", host)
            .header("authorization", "Bearer untrusted-token");
        if let Some(origin) = origin {
            builder = builder.header("origin", origin);
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
}

#[path = "support/resources.rs"]
mod resources;
use resources::*;
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires explicit owned local retained-schema PostgreSQL fixture"]
async fn native_resources_and_mcp_preserve_scopes_and_live_authority_without_plugins() {
    let fixture = Arc::new(Fixture::new().await);
    let f = fixture.clone();
    let result = tokio::spawn(resource_scenario(f)).await;
    fixture.cleanup().await;
    result.unwrap();
}

struct DelayAllocator {
    inner: webdock_persistence::IdAllocator,
    first: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl webdock_registry::Allocator for DelayAllocator {
    async fn allocate(
        &self,
        _tx: &sea_orm::DatabaseTransaction,
    ) -> webdock_registry::Result<String> {
        if self.first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        Ok(self.inner.allocate().await?.to_string())
    }
}
#[tokio::test]
#[ignore = "requires explicit owned local retained-schema PostgreSQL fixture"]
async fn expires_between_admission_and_registry_commit_rolls_back_record_and_audit() {
    let fixture = Arc::new(Fixture::new().await);
    let f = fixture.clone();
    let result = tokio::spawn(expiry_scenario(f)).await;
    fixture.cleanup().await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires explicit owned local retained-schema PostgreSQL fixture"]
async fn hosting_bearer_rejects_legacy_mixed_registry_scopes() {
    let fixture = Arc::new(Fixture::new().await);
    let f = fixture.clone();
    let result = tokio::spawn(mixed_scope_scenario(f)).await;
    fixture.cleanup().await;
    result.unwrap();
}

async fn resource_scenario(f: Arc<Fixture>) {
    let app = f.app(None).await;
    let read = &f.registry_client.read;
    let write = &f.registry_client.write;
    let (status, discovery) = request(&app, "GET", "/api/registry", Some(read), None).await;
    assert_eq!(status, StatusCode::OK, "{discovery}");
    assert_eq!(discovery["resource"], "http://localhost:3120/api/mcp");
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/registry/customers",
            Some(read),
            Some(json!({"name":f.nonce}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, customer) = request(
        &app,
        "POST",
        "/api/registry/customers",
        Some(write),
        Some(json!({"name":f.nonce})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{customer}");
    let cid = customer["id"].as_str().unwrap();
    let (status, project) = request(
        &app,
        "POST",
        "/api/registry/projects",
        Some(write),
        Some(json!({"name":f.nonce,"customer":cid})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let pid = project["id"].as_str().unwrap();
    let (_, record) = request(
        &app,
        "GET",
        &format!("/api/registry/projects/{pid}"),
        Some(read),
        None,
    )
    .await;
    assert_eq!(record["customer"], cid);
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/registry/customers?page=1&page=2",
            Some(read),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&app, "GET", "/api/registry/users", Some(read), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/registry",
            Some(&f.hosting_client.read),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", "/api/hosting/clusters", Some(read), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, hosting) = request(
        &app,
        "GET",
        &format!("/api/hosting/customers/{cid}/projects"),
        Some(&f.hosting_client.read),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hosting}");
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/api/hosting/projects/{pid}/apps"),
            Some(&f.hosting_client.write),
            Some(json!({"projectID":pid}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let cluster_input = json!({"name":f.nonce,"provider":"owned-fixture","country":"DE","region":"local","locationEvidence":"isolated API contract fixture","runtimeKind":"docker","idempotencyKey":format!("{}-cluster",f.nonce)});
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/hosting/clusters",
            Some(&f.hosting_client.read),
            Some(cluster_input.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, cluster) = request(
        &app,
        "POST",
        "/api/hosting/clusters",
        Some(&f.hosting_client.write),
        Some(cluster_input.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cluster}");
    assert_eq!(cluster["runtimeKind"], "docker");
    let (_, replay) = request(
        &app,
        "POST",
        "/api/hosting/clusters",
        Some(&f.hosting_client.write),
        Some(cluster_input.clone()),
    )
    .await;
    assert_eq!(replay, cluster);
    let (_,mcp_replay)=request(&app,"POST","/api/mcp",Some(&f.hosting_client.write),Some(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"register_hosting_cluster","arguments":cluster_input}}))).await;
    let mcp_cluster: Value =
        serde_json::from_str(mcp_replay["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(mcp_cluster, cluster);
    let mut changed = cluster_input;
    changed["region"] = json!("different");
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/hosting/clusters",
            Some(&f.hosting_client.write),
            Some(changed)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for (token, registry, write) in [
        (read, true, false),
        (&f.registry_client.write, true, true),
        (&f.hosting_client.read, false, false),
        (&f.hosting_client.write, false, true),
    ] {
        let (status, list) = request(
            &app,
            "POST",
            "/api/mcp",
            Some(token),
            Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{list}");
        let tools = list["result"]["tools"].as_array().expect("native tools");
        let names = tools
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names.contains(&"list_records"), registry);
        assert_eq!(names.contains(&"list_hosting_apps"), !registry);
        assert_eq!(names.contains(&"save_customer"), registry && write);
        assert_eq!(names.contains(&"create_hosting_app"), !registry && write);
        assert!(!names.iter().any(|n| n.starts_with("plugin__")));
    }
    let (_,called)=request(&app,"POST","/api/mcp",Some(read),Some(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_record","arguments":{"collection":"projects","id":pid}}}))).await;
    let record: Value =
        serde_json::from_str(called["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(record["id"], pid);
    let (denied_status, _denied)=request(&app,"POST","/api/mcp",Some(read),Some(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"save_customer","arguments":{"data":{"name":"forbidden"}}}}))).await;
    assert_eq!(denied_status, StatusCode::FORBIDDEN);
    sql(
        &f.owner,
        "UPDATE webdock_auth.\"oauthAccessToken\" SET revoked=now() WHERE token=$1",
        vec![webdock_oauth::legacy_token_hash(read).into()],
    )
    .await;
    assert_eq!(
        request(&app, "GET", "/api/registry", Some(read), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/mcp",
            Some(read),
            Some(json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    sql(
        &f.owner,
        "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
        vec![f.uid.clone().into()],
    )
    .await;
    assert_eq!(
        request(&app, "GET", "/api/registry", Some(write), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/hosting/clusters",
            Some(&f.hosting_client.read),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/api/hosting/customers/{cid}/projects"),
            Some(&f.hosting_client.read),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    sql(&f.owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") SELECT $1,organization_id,$2,'member',now() FROM webdock_auth.tenant_customer WHERE customer_id=$3",vec![format!("{}-member",f.nonce).into(),f.uid.clone().into(),cid.into()]).await;
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/api/hosting/customers/{cid}/projects"),
            Some(&f.hosting_client.read),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.member WHERE id=$1 AND \"userId\"=$2",
        vec![format!("{}-member", f.nonce).into(), f.uid.clone().into()],
    )
    .await;
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/api/hosting/customers/{cid}/projects"),
            Some(&f.hosting_client.read),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

async fn expiry_scenario(f: Arc<Fixture>) {
    let credentials = webdock_oauth::ClientAuthentication {
        client_id: f.registry_client.id.clone(),
        client_secret: Some(f.registry_client.secret.clone()),
        method: webdock_oauth::ClientAuthMethod::ClientSecretPost,
    };
    let token = issue(
        &f.oauth,
        &f.cookie,
        &credentials,
        "openid webdock:read webdock:write",
        &f.resource,
    )
    .await;
    sql(&f.owner,"UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=now()+interval '2 seconds' WHERE token=$1",vec![webdock_oauth::legacy_token_hash(&token).into()]).await;
    let delayed = (*f.registry)
        .clone()
        .with_allocator(Arc::new(DelayAllocator {
            inner: webdock_persistence::IdAllocator::new(
                db("AUTH_DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            ),
            first: std::sync::atomic::AtomicBool::new(true),
        }));
    let app = f.app(Some(Arc::new(delayed))).await;
    let name = format!("{}-expiry", f.nonce);
    let (status, value) = request(
        &app,
        "POST",
        "/api/registry/customers",
        Some(&token),
        Some(json!({"name":name})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
    assert_eq!(
        count(
            &f.owner,
            "SELECT count(*) AS n FROM webdock_admin.customers WHERE name=$1",
            vec![name.into()]
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &f.owner,
            "SELECT count(*) AS n FROM webdock_admin.audit_events WHERE actor_id=$1",
            vec![f.mapped.clone().into()]
        )
        .await,
        0
    );
}

async fn mixed_scope_scenario(f: Arc<Fixture>) {
    sql(&f.owner,"UPDATE webdock_auth.\"oauthClient\" SET scopes=scopes || '[\"webdock:read\"]'::jsonb WHERE \"clientId\"=$1",vec![f.hosting_client.id.clone().into()]).await;
    sql(&f.owner,"UPDATE webdock_auth.\"oauthAccessToken\" SET scopes=scopes || '[\"webdock:read\"]'::jsonb WHERE token=$1",vec![webdock_oauth::legacy_token_hash(&f.hosting_client.read).into()]).await;
    let app = f.app(None).await;
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/hosting/clusters",
            Some(&f.hosting_client.read),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/mcp",
            Some(&f.hosting_client.read),
            Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[path = "support/resource_commit.rs"]
mod resource_commit;
