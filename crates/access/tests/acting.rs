//! Customers list and create projects in their own workspace, against the retained local fixture.
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use std::{collections::BTreeMap, sync::Arc};
use webdock_access::*;
use webdock_identity::{CookiePolicy, IdentityService, RlsSigner, SecretKeys};

fn guarded(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let u = url::Url::parse(&raw).unwrap();
    assert!([Some("localhost"), Some("127.0.0.1")].contains(&u.host_str()));
    assert_eq!(u.path(), "/webdock_rls_test");
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
async fn id(db: &DatabaseConnection) -> String {
    db.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT webdock_admin.next_snowflake() AS id",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "id")
    .unwrap()
}

#[tokio::test]
#[ignore = "requires owned retained-schema restricted fixture"]
async fn customer_acting_scope_is_owner_only_and_session_bound() {
    let owner = Database::connect(guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    let auth_url = guarded("AUTH_DATABASE_URL");
    let auth = Database::connect(auth_url.clone()).await.unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let keys = SecretKeys::new(
        BTreeMap::from([(0, secret.clone())]),
        0,
        Some(secret.clone()),
    )
    .unwrap();
    let cookies = CookiePolicy::new(false, secret.clone()).unwrap();
    let identity = Arc::new(
        IdentityService::new(
            auth.clone(),
            CookiePolicy::new(false, secret.clone()).unwrap(),
        )
        .with_allocator(allocator.clone()),
    );
    let fixture_ids = allocator.clone();
    let access = AccessService::new(
        Database::connect(guarded("DATABASE_URL")).await.unwrap(),
        auth.clone(),
        identity.clone(),
        allocator,
        RlsSigner::new(
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            "webdock_auth_runtime",
        )
        .unwrap(),
        Arc::new(keys),
        AccessConfig {
            auth_origin: url::Url::parse("https://auth.example.invalid").unwrap(),
            console_origins: vec![url::Url::parse("https://studio.example.invalid").unwrap()],
            allow_local_http: false,
        },
    )
    .unwrap();
    let nonce = format!(
        "projects-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    let mut customers = vec![];
    let mut orgs = vec![];
    for label in ["a", "b"] {
        let cid = id(&owner).await;
        sql(
            &owner,
            "WITH hint AS MATERIALIZED (SELECT set_config('webdock.organization_allocation',jsonb_build_object('customer_id',$1::text,'organization_id',$3::text)::text,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,$2 FROM hint",
            vec![cid.clone().into(), format!("{nonce}-{label}").into(),fixture_ids.allocate().await.unwrap().to_string().into()],
        )
        .await;
        let org: String = owner
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
                [cid.clone().into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "organization_id")
            .unwrap();
        customers.push(cid);
        orgs.push(org);
    }
    let uid = format!("{nonce}-operator");
    let session = format!("{uid}-session");
    sql(&owner, "INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,'operator',true,false)", vec![uid.clone().into(), format!("{uid}@example.invalid").into()]).await;
    sql(&owner, "INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$1,$2,now()+interval '1 hour',now())", vec![session.clone().into(),uid.clone().into()]).await;
    sql(&owner, "INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$2)", vec![id(&owner).await.into(),uid.clone().into(),format!("{uid}@example.invalid").into()]).await;
    let cookie = format!("{}={}", cookies.name(), cookies.sign(&session).unwrap());
    let principal = identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    assert!(principal.is_operator());
    let preview = access
        .start_preview_as(&principal, &customers[0])
        .await
        .unwrap();
    assert_eq!(preview["readOnly"], true);
    assert!(
        access
            .create_project_as(&principal, &customers[0], "Denied")
            .await
            .is_err()
    );
    access.exit_preview_as(&principal).await.unwrap();
    // Explicit owner mode is separate from the old read-only default.
    let started = access
        .start_customer_scope_as(&principal, &customers[0], PreviewMode::Owner)
        .await
        .unwrap();
    assert_eq!(started["mode"], "owner");
    assert_eq!(started["readOnly"], false);
    let acting = identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    assert!(!acting.is_operator());
    assert!(acting.in_tenant_preview());
    let created = access
        .create_project_as(&acting, &customers[0], "Acting owner")
        .await
        .unwrap();
    assert_eq!(created["name"], "Acting owner");
    assert!(
        access
            .create_project_as(&acting, &customers[1], "Foreign")
            .await
            .is_err()
    );
    assert!(access.require_operator_as(&acting).await.is_err());
    assert_eq!(
        access
            .get_project_as(&acting, created["id"].as_str().unwrap())
            .await
            .unwrap()["role"],
        "owner"
    );
    assert_eq!(access.list_tenants_as(&acting).await.unwrap().len(), 1);
    assert_eq!(
        access.list_tenants_as(&acting).await.unwrap()[0]["role"],
        "owner"
    );
    assert!(access.get_tenant_as(&acting, &customers[0]).await.unwrap()["canManage"] == true);
    access
        .manage_tenant_as(
            &acting,
            &customers[0],
            TenantCommand::Profile {
                profile: Box::new(TenantProfileInput {
                    name: "Acted customer".into(),
                    ..Default::default()
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        access.get_tenant_as(&acting, &customers[0]).await.unwrap()["tenant"]["name"],
        "Acted customer"
    );
    assert!(
        access
            .add_domain_as(&acting, &customers[0], "acting.example.invalid")
            .await
            .is_ok()
    );
    assert!(
        access
            .add_domain_as(&acting, &customers[1], "foreign.example.invalid")
            .await
            .is_err()
    );
    assert!(
        access
            .create_api_token_as(&acting, &customers[0], "Escape", true, 1)
            .await
            .is_err()
    );
    let audit=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT actor_id FROM webdock_auth.access_event WHERE target_id=$1 AND action='project-created' ORDER BY created_at DESC LIMIT 1",[customers[0].clone().into()])).await.unwrap().unwrap();
    assert_eq!(audit.try_get::<String>("", "actor_id").unwrap(), uid);
    sql(&owner,"INSERT INTO webdock_auth.customer_plan(customer_id,overrides,updated_by) VALUES($1,$2,'fixture')",vec![customers[0].clone().into(),serde_json::json!({"projects":1}).into()]).await;
    assert!(matches!(
        access
            .create_project_as(&acting, &customers[0], "Plan limit")
            .await,
        Err(Error::ProjectLimit)
    ));
    sql(&owner,"UPDATE webdock_auth.studio_tenant_preview SET expires_at=now()-interval '1 second' WHERE session_id=$1",vec![session.clone().into()]).await;
    assert!(
        access
            .create_project_as(&acting, &customers[0], "Expired")
            .await
            .is_err()
    );
    assert!(
        access
            .create_project_as(&acting, &customers[1], "Expired foreign")
            .await
            .is_err()
    );
    assert!(access.require_operator_as(&acting).await.is_err());
    access.exit_preview_as(&acting).await.unwrap();
    assert!(
        identity
            .authenticate_cookie(&cookie)
            .await
            .unwrap()
            .unwrap()
            .is_operator()
    );
    access
        .start_customer_scope_as(&principal, &customers[0], PreviewMode::Owner)
        .await
        .unwrap();
    let reentered = identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(reentered.customer_scope(), acting.customer_scope());
    assert!(
        access
            .add_domain_as(&acting, &customers[0], "old-epoch.example.invalid")
            .await
            .is_err()
    );
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
        vec![uid.clone().into()],
    )
    .await;
    assert!(
        access
            .add_domain_as(&acting, &customers[0], "demoted.example.invalid")
            .await
            .is_err()
    );
    assert!(
        access
            .create_project_as(&acting, &customers[0], "Demoted")
            .await
            .is_err()
    );
    sql(
        &owner,
        "DELETE FROM webdock_auth.session WHERE id=$1",
        vec![session.clone().into()],
    )
    .await;
    assert!(access.list_tenants_as(&acting).await.is_err());
    // All rows belong to this disposable fixture, never customer/operator state.
    sql(
        &owner,
        "DELETE FROM webdock_auth.access_event WHERE actor_id=$1",
        vec![uid.clone().into()],
    )
    .await;
    for customer in &customers {
        sql(
            &owner,
            "DELETE FROM webdock_auth.customer_domain WHERE customer_id=$1",
            vec![customer.clone().into()],
        )
        .await;
        sql(
            &owner,
            "DELETE FROM webdock_admin.projects WHERE customer_id=$1",
            vec![customer.clone().into()],
        )
        .await;
        sql(
            &owner,
            "DELETE FROM webdock_admin.customers WHERE id=$1",
            vec![customer.clone().into()],
        )
        .await;
    }
    sql(
        &owner,
        "DELETE FROM webdock_admin.users WHERE auth_subject=$1",
        vec![uid.clone().into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        vec![uid.into()],
    )
    .await;
}
