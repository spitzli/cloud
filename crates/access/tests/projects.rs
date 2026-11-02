//! Customers list and create projects in their own workspace, against the retained local fixture.
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use std::{collections::BTreeMap, sync::Arc};
use webdock_access::*;
use webdock_identity::{CookiePolicy, IdentityService, Principal, RlsSigner, SecretKeys};

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
async fn customer_projects() {
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
            "INSERT INTO webdock_admin.customers(id,name) VALUES($1,$2)",
            vec![cid.clone().into(), format!("{nonce}-{label}").into()],
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
    let existing = id(&owner).await;
    sql(
        &owner,
        "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,'Existing',$2)",
        vec![existing.clone().into(), customers[0].clone().into()],
    )
    .await;
    let mut users = vec![];
    let mut principals = std::collections::HashMap::<&str, Principal>::new();
    for (label, role, org) in [
        ("operator", "operator", None),
        ("owner", "user", Some((0, "owner"))),
        ("admin", "user", Some((0, "admin"))),
        ("member", "user", Some((0, "member"))),
        ("foreign", "user", Some((1, "owner"))),
        ("preview", "operator", None),
    ] {
        let uid = format!("{nonce}-{label}");
        let token = format!("{uid}-token");
        sql(&owner, "INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,$3,true,false)", vec![uid.clone().into(), format!("{uid}@example.invalid").into(), role.into()]).await;
        sql(&owner, "INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$2,$3,now()+interval '1 hour',now())", vec![format!("{uid}-session").into(), token.clone().into(), uid.clone().into()]).await;
        if role == "operator" {
            sql(&owner, "INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$2)", vec![id(&owner).await.into(), uid.clone().into(), format!("{uid}@example.invalid").into()]).await;
        }
        if let Some((index, member_role)) = org {
            sql(&owner, "INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,$4,now())", vec![format!("{uid}-member").into(), orgs[index].clone().into(), uid.clone().into(), member_role.into()]).await;
        }
        if label == "preview" {
            sql(&owner, "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES($1,$2,$3,$4)", vec![format!("{uid}-session").into(), uid.clone().into(), customers[0].clone().into(), orgs[0].clone().into()]).await;
        }
        let cookie = format!("{}={}", cookies.name(), cookies.sign(&token).unwrap());
        principals.insert(
            label,
            identity
                .authenticate_cookie(&cookie)
                .await
                .unwrap()
                .unwrap(),
        );
        users.push(uid);
    }
    let p = |label: &str| &principals[label];
    async {
        // member_lists_projects
        let listed = access
            .list_projects_as(p("member"), &customers[0])
            .await
            .unwrap();
        // Every customer starts with a Default project; lists sort by name.
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["name"], "Default");
        assert_eq!(listed[1]["id"], existing.as_str());
        assert_eq!(listed[1]["name"], "Existing");
        // admin_creates_project
        let created = access
            .create_project_as(p("admin"), &customers[0], "  Shop  ")
            .await
            .unwrap();
        assert_eq!(created["name"], "Shop");
        let shop = created["id"].as_str().unwrap().to_owned();
        assert_eq!(
            access
                .list_projects_as(p("owner"), &customers[0])
                .await
                .unwrap()
                .len(),
            3
        );
        let detail = access.get_project_as(p("member"), &shop).await.unwrap();
        assert_eq!(detail["customer"]["id"], customers[0].as_str());
        assert_eq!(detail["role"], "member");
        assert_eq!(
            access.get_project_as(p("operator"), &shop).await.unwrap()["role"],
            "operator"
        );
        // member_cannot_create_project
        assert!(matches!(
            access
                .create_project_as(p("member"), &customers[0], "No")
                .await,
            Err(Error::Forbidden)
        ));
        // foreign_customer_denied
        assert!(
            access
                .list_projects_as(p("foreign"), &customers[0])
                .await
                .is_err()
        );
        assert!(access.get_project_as(p("foreign"), &shop).await.is_err());
        assert!(
            access
                .create_project_as(p("foreign"), &customers[0], "No")
                .await
                .is_err()
        );
        // preview_cannot_create
        assert!(
            access
                .create_project_as(p("preview"), &customers[0], "No")
                .await
                .is_err()
        );
        assert!(
            access
                .create_project_as(p("owner"), &customers[0], "")
                .await
                .is_err()
        );
        // Operators create for any customer.
        assert!(
            access
                .create_project_as(p("operator"), &customers[1], "Ops")
                .await
                .is_ok()
        );
        // The plan's project limit counts what exists, for operators too.
        sql(&owner, "INSERT INTO webdock_auth.customer_plan(customer_id,overrides,updated_by) VALUES($1,$2,'fixture')", vec![customers[1].clone().into(), serde_json::json!({"projects": 1}).into()]).await;
        assert!(matches!(
            access
                .create_project_as(p("operator"), &customers[1], "Over")
                .await,
            Err(webdock_access::Error::ProjectLimit)
        ));

        // Domains: owners add one and get the TXT record to publish; members only read.
        let added = access
            .add_domain_as(p("owner"), &customers[0], " Shop.Example.Invalid. ")
            .await
            .unwrap();
        assert_eq!(added["domain"], "shop.example.invalid");
        assert_eq!(added["verified"], false);
        assert_eq!(added["record"]["name"], "_webdock-challenge.shop.example.invalid");
        assert_eq!(added["record"]["value"].as_str().unwrap().len(), 32);
        assert!(matches!(
            access.add_domain_as(p("owner"), &customers[0], "shop.example.invalid").await,
            Err(Error::Conflict)
        ));
        assert!(matches!(
            access.add_domain_as(p("member"), &customers[0], "other.example.invalid").await,
            Err(Error::Forbidden)
        ));
        assert!(access.add_domain_as(p("owner"), &customers[0], "*.example.invalid").await.is_err());
        assert_eq!(access.list_domains_as(p("member"), &customers[0]).await.unwrap().len(), 1);
        assert!(access.list_domains_as(p("foreign"), &customers[0]).await.is_err());
        // Without the record the domain stays unverified, and local names need DNS here too.
        let checked = access
            .verify_domain_as(p("admin"), &customers[0], "shop.example.invalid")
            .await
            .unwrap();
        assert_eq!(checked["verified"], false);
        assert!(!checked["checkedAt"].is_null());
        access.add_domain_as(p("owner"), &customers[0], "shop.localhost").await.unwrap();
        assert_eq!(
            access.verify_domain_as(p("owner"), &customers[0], "shop.localhost").await.unwrap()["verified"],
            false
        );
        access.remove_domain_as(p("owner"), &customers[0], "shop.localhost").await.unwrap();
        assert!(matches!(
            access.remove_domain_as(p("member"), &customers[0], "shop.example.invalid").await,
            Err(Error::Forbidden)
        ));
        access.remove_domain_as(p("owner"), &customers[0], "shop.example.invalid").await.unwrap();
        assert!(access.list_domains_as(p("owner"), &customers[0]).await.unwrap().is_empty());

        // An apply holding the customer lock may be about to commit a host for this domain.
        // Removing the domain must wait and then see that committed use.
        access.add_domain_as(p("owner"), &customers[0], "race.example.invalid").await.unwrap();
        let cluster = id(&owner).await;
        let plugin = format!("domain-{nonce}");
        sql(&owner, "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence) VALUES($1,$1,'fixture','eu','fixture')", vec![cluster.clone().into()]).await;
        sql(&owner, "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','1.0.0','ready',1)", vec![plugin.clone().into()]).await;
        let applying = owner.begin().await.unwrap();
        sql(&applying, "SELECT pg_advisory_xact_lock(hashtext('webdock-limits:'||$1))", vec![customers[0].clone().into()]).await;
        sql(&applying, "INSERT INTO webdock_auth.plugin_workload(plugin_id,unit,project_id,cluster_id,namespace,objects,hosts) VALUES($1,'race',$2,$3,'wd-fixture','[]',ARRAY['race.example.invalid'])", vec![plugin.clone().into(), shop.clone().into(), cluster.clone().into()]).await;
        let removal = access.remove_domain_as(p("owner"), &customers[0], "race.example.invalid");
        tokio::pin!(removal);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(200), &mut removal).await.is_err(), "domain removal must wait for the pending apply");
        applying.commit().await.unwrap();
        assert!(matches!(removal.await, Err(Error::DomainInUse)));
        sql(&owner, "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1", vec![plugin.clone().into()]).await;
        sql(&owner, "DELETE FROM webdock_auth.plugin_installation WHERE id=$1", vec![plugin.into()]).await;
        sql(&owner, "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1", vec![cluster.into()]).await;
        access.remove_domain_as(p("owner"), &customers[0], "race.example.invalid").await.unwrap();

        // API tokens: owners and admins make them; the secret is shown once and acts as its maker.
        let made = access
            .create_api_token_as(p("admin"), &customers[0], "CI deploy", false, 30)
            .await
            .unwrap();
        let token = made["token"].as_str().unwrap().to_owned();
        assert!(token.starts_with("wdk_") && token.len() == 68);
        assert_eq!(made["canWrite"], false);
        assert!(matches!(
            access.create_api_token_as(p("member"), &customers[0], "No", true, 30).await,
            Err(Error::Forbidden)
        ));
        assert!(access.create_api_token_as(p("owner"), &customers[0], "Too long", true, 400).await.is_err());
        let listed = access.list_api_tokens_as(p("owner"), &customers[0]).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].get("token").is_none(), "the secret is never listed");
        assert!(access.list_api_tokens_as(p("member"), &customers[0]).await.is_err());
        let actor = access.api_actor(&token).await.unwrap();
        assert_eq!(actor.customer, customers[0]);
        assert!(!actor.can_write);
        assert_eq!(
            access.list_projects_as(&actor.principal, &customers[0]).await.unwrap().len(),
            3
        );
        for wrong in [format!("{token}0"), token.replace("wdk_", "wdx_"), "wdk_".to_owned() + &"0".repeat(64)] {
            assert!(access.api_actor(&wrong).await.is_err(), "{wrong}");
        }
        access
            .revoke_api_token_as(p("owner"), &customers[0], made["id"].as_str().unwrap())
            .await
            .unwrap();
        assert!(access.api_actor(&token).await.is_err(), "a revoked token stops working");
    }
    .await;
    for uid in &users {
        sql(
            &owner,
            "DELETE FROM webdock_auth.studio_tenant_preview WHERE actor_id=$1",
            vec![uid.clone().into()],
        )
        .await;
        sql(
            &owner,
            "DELETE FROM webdock_auth.access_event WHERE actor_id=$1",
            vec![uid.clone().into()],
        )
        .await;
    }
    for cid in &customers {
        sql(
            &owner,
            "DELETE FROM webdock_admin.projects WHERE customer_id=$1",
            vec![cid.clone().into()],
        )
        .await;
        sql(
            &owner,
            "DELETE FROM webdock_admin.customers WHERE id=$1",
            vec![cid.clone().into()],
        )
        .await;
    }
    for uid in &users {
        sql(
            &owner,
            "DELETE FROM webdock_auth.member WHERE \"userId\"=$1",
            vec![uid.clone().into()],
        )
        .await;
        sql(
            &owner,
            "DELETE FROM webdock_admin.users WHERE name=$1",
            vec![uid.clone().into()],
        )
        .await;
        sql(
            &owner,
            "DELETE FROM webdock_auth.session WHERE \"userId\"=$1",
            vec![uid.clone().into()],
        )
        .await;
        sql(
            &owner,
            "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
            vec![uid.clone().into()],
        )
        .await;
    }
}
