//! Uses only uniquely owned records in the pre-provisioned local RLS fixture.
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use webdock_identity::{CookiePolicy, IdentityService, RlsSigner};
use webdock_registry::{Allocator, RegistryError, RegistryService};
const COOKIE_SECRET: &str = "registry-test-cookie-secret-at-least-32-bytes";
async fn connect(key: &str) -> DatabaseConnection {
    let value = std::env::var(key).expect("explicit disposable database URL");
    let url = url::Url::parse(&value).unwrap();
    assert!(
        matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
            && url.path() == "/webdock_rls_test"
            && url.query().is_none(),
        "only local webdock_rls_test allowed"
    );
    Database::connect(value).await.unwrap()
}
async fn sql(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
async fn scalar(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}
struct FailAudit {
    count: AtomicUsize,
}
#[async_trait::async_trait]
impl Allocator for FailAudit {
    async fn allocate(
        &self,
        tx: &sea_orm::DatabaseTransaction,
    ) -> webdock_registry::Result<String> {
        if self.count.fetch_add(1, Ordering::SeqCst) > 0 {
            return Err(RegistryError::Conflict("fixture audit failure"));
        }
        webdock_registry::DatabaseAllocator.allocate(tx).await
    }
}
struct RevokeOnAudit {
    count: AtomicUsize,
    owner: DatabaseConnection,
    user_id: String,
}
#[async_trait::async_trait]
impl Allocator for RevokeOnAudit {
    async fn allocate(
        &self,
        tx: &sea_orm::DatabaseTransaction,
    ) -> webdock_registry::Result<String> {
        if self.count.fetch_add(1, Ordering::SeqCst) > 0 {
            sql(
                &self.owner,
                "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
                vec![self.user_id.clone().into()],
            )
            .await;
        }
        webdock_registry::DatabaseAllocator.allocate(tx).await
    }
}
#[tokio::test]
#[ignore = "requires explicit local retained-schema RLS fixture environment"]
async fn operator_crud_atomic_audit_retained_guard_and_revocation() {
    let owner = connect("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    owner
        .execute_unprepared(include_str!("../migrations/0001_customer_deletion.sql"))
        .await
        .unwrap();
    let runtime = connect("WEBDOCK_REGISTRY_DATABASE_URL").await;
    let auth = connect("AUTH_DATABASE_URL").await;
    let nonce = format!(
        "rust-registry-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    let uid = nonce.clone();
    let sid = format!("{nonce}-session");
    let token = format!("{nonce}-token");
    let email = format!("{nonce}@example.invalid");
    sql(&owner,"INSERT INTO webdock_auth.\"user\" (id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES ($1,'Registry test',$2,true,'operator',true,false)",vec![uid.clone().into(),email.clone().into()]).await;
    sql(&owner,"INSERT INTO webdock_auth.session (id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES ($1,$2,$3,now()+interval '1 hour',now())",vec![sid.clone().into(),token.clone().into(),uid.clone().into()]).await;
    let mapped = owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_admin.next_snowflake() AS id",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "id")
        .unwrap();
    sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES ($1,'Registry test',$2,'operator',$3)",vec![mapped.clone().into(),email.into(),uid.clone().into()]).await;
    let cookies = CookiePolicy::new(false, COOKIE_SECRET).unwrap();
    let cookie = format!("{}={}", cookies.name(), cookies.sign(&token).unwrap());
    let identity = IdentityService::new(auth, cookies);
    let principal = identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    assert!(principal.is_operator());
    let signer = || {
        RlsSigner::new(
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            "webdock_registry_runtime",
        )
        .unwrap()
    };
    let allocator = webdock_persistence::IdAllocator::new(
        connect("WEBDOCK_REGISTRY_DATABASE_URL").await,
        webdock_ids::Identity::platform(0, 2).unwrap(),
    );
    let service =
        RegistryService::new(runtime.clone(), signer()).with_allocator(Arc::new(allocator));
    let result = {
        let owner = owner.clone();
        let nonce = nonce.clone();
        let mapped = mapped.clone();
        let uid = uid.clone();
        async move {
            assert_eq!(scalar(&runtime,"SELECT count(*) AS n FROM pg_roles WHERE rolname=current_user AND (rolsuper OR rolbypassrls)",vec![]).await,0);
            sql(
                &owner,
                "UPDATE webdock_admin.users SET auth_subject=NULL WHERE id=$1",
                vec![mapped.clone().into()],
            )
            .await;
            assert!(
                service
                    .list(&principal, json!({"collection":"customers"}))
                    .await
                    .is_err()
            );
            sql(
                &owner,
                "UPDATE webdock_admin.users SET auth_subject=$1 WHERE id=$2",
                vec![uid.clone().into(), mapped.clone().into()],
            )
            .await;
            assert_eq!(
                scalar(
                    &runtime,
                    "SELECT count(*) AS n FROM webdock_admin.customers",
                    vec![]
                )
                .await,
                0,
                "unsigned role sees no records"
            );
            let customer = service
                .save_customer(
                    &principal,
                    None,
                    json!({"name":nonce,"notes":"retain","country":"ch"}),
                )
                .await
                .unwrap();
            let cid = customer["id"].as_str().unwrap();
            assert_eq!(customer["country"], "CH");
            assert!(customer["createdAt"].as_str().unwrap().ends_with('Z'));
            let customer = service
                .save_customer(&principal, Some(cid), json!({"name":nonce}))
                .await
                .unwrap();
            assert_eq!(customer["notes"], "retain");
            let customer = service
                .save_customer(&principal, Some(cid), json!({"name":nonce,"notes":null}))
                .await
                .unwrap();
            assert!(customer["notes"].is_null());
            let project = service
                .save_project(&principal, None, json!({"name":nonce,"customer":cid}))
                .await
                .unwrap();
            let pid = project["id"].as_str().unwrap();
            assert_eq!(project["customer"], cid);
            assert!(
                service
                    .save_project(&principal, Some(pid), json!({"name":nonce,"customer":"1"}))
                    .await
                    .is_err()
            );
            assert!(
                service
                    .archive(&principal, "customers", cid, true)
                    .await
                    .is_err()
            );
            let listed = service
                .list(
                    &principal,
                    json!({"collection":"projects","customer":cid,"limit":1,"search":nonce,"status":"active"}),
                )
                .await
                .unwrap();
            assert_eq!(listed["totalDocs"], 1);
            assert_eq!(listed["totalPages"], 1);
            assert_eq!(listed["docs"][0]["id"], pid);
            let related = service
                .query(
                    &principal,
                    "findByID",
                    json!({"collection":"projects","id":pid,"depth":1}),
                )
                .await
                .unwrap();
            assert_eq!(related["customer"]["id"], cid);
            for filter in [
                json!({"name":{"equals":true}}),
                json!({"name":{"equals":1}}),
            ] {
                assert_eq!(
                    service
                        .query(
                            &principal,
                            "count",
                            json!({"collection":"customers","where":filter})
                        )
                        .await
                        .unwrap()["totalDocs"],
                    0
                );
            }
            assert_eq!(service.query(&principal,"count",json!({"collection":"customers","where":{"and":[{"id":{"equals":cid}},{"createdAt":{"equals":customer["createdAt"]}}]}})).await.unwrap()["totalDocs"],1);
            let empty_or=service.query(&principal,"count",json!({"collection":"customers","where":{"and":[{"id":{"equals":cid}},{"or":[{}]}]}})).await.unwrap();
            assert_eq!(
                empty_or["totalDocs"], 0,
                "empty child filters do not make an OR match all records"
            );
            let selected=service.query(&principal,"find",json!({"collection":"customers","where":{"id":{"equals":cid}},"select":{"name":true}})).await.unwrap();
            assert_eq!(selected["docs"][0].as_object().unwrap().len(), 2);
            let safe = service
                .query(
                    &principal,
                    "findByID",
                    json!({"collection":"users","id":mapped}),
                )
                .await
                .unwrap();
            assert!(safe.get("hash").is_none());
            assert!(service.get(&principal, "users", &mapped).await.is_err());
            let audit = service
                .list(
                    &principal,
                    json!({"collection":"audit-events","targetID":cid}),
                )
                .await
                .unwrap();
            assert_eq!(audit["totalDocs"], 3);
            assert_eq!(audit["docs"][0]["actor"], mapped);
            let failed = RegistryService::new(runtime.clone(), signer()).with_allocator(Arc::new(
                FailAudit {
                    count: AtomicUsize::new(0),
                },
            ));
            assert!(
                failed
                    .save_customer(
                        &principal,
                        None,
                        json!({"name":format!("{nonce}-rollback")})
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                scalar(
                    &owner,
                    "SELECT count(*) AS n FROM webdock_admin.customers WHERE name=$1",
                    vec![format!("{nonce}-rollback").into()]
                )
                .await,
                0
            );
            let revoked = RegistryService::new(runtime.clone(), signer()).with_allocator(Arc::new(
                RevokeOnAudit {
                    count: AtomicUsize::new(0),
                    owner: owner.clone(),
                    user_id: uid.clone(),
                },
            ));
            let failed_write = revoked
                .save_customer(
                    &principal,
                    None,
                    json!({"name":format!("{nonce}-rollback")}),
                )
                .await;
            sql(
                &owner,
                "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",
                vec![uid.clone().into()],
            )
            .await;
            assert!(
                failed_write.is_err(),
                "authority revoked after mutation must prevent commit"
            );
            assert_eq!(
                scalar(
                    &owner,
                    "SELECT count(*) AS n FROM webdock_admin.customers WHERE name=$1",
                    vec![format!("{nonce}-rollback").into()]
                )
                .await,
                0
            );
            sql(&owner,"INSERT INTO webdock_admin.cms_instances(id,label,project_id,admin_u_r_l,schema_name,provider_project_i_d,template,status) VALUES ($1,'Registry fixture',$2,'https://example.invalid',$3,$3,'custom','suspended')",vec![format!("{nonce}-cms").into(),pid.into(),nonce.clone().into()]).await;
            assert!(
                service
                    .archive(&principal, "projects", pid, true)
                    .await
                    .is_err()
            );
            sql(
                &owner,
                "UPDATE webdock_admin.cms_instances SET status='retired' WHERE project_id=$1",
                vec![pid.into()],
            )
            .await;
            assert_eq!(
                service
                    .archive(&principal, "projects", pid, true)
                    .await
                    .unwrap()["status"],
                "archived"
            );
            assert!(
                service
                    .patch_repository(&principal, pid, "https://example.invalid")
                    .await
                    .is_err()
            );
            service
                .archive(&principal, "customers", cid, true)
                .await
                .unwrap();
            assert!(
                matches!(
                    service.delete_customer(&principal, cid).await,
                    Err(RegistryError::Conflict(_))
                ),
                "an archived customer with a project is kept"
            );
            assert!(
                service
                    .archive(&principal, "projects", pid, false)
                    .await
                    .is_err()
            );
            service
                .archive(&principal, "customers", cid, false)
                .await
                .unwrap();
            service
                .archive(&principal, "projects", pid, false)
                .await
                .unwrap();
            assert_eq!(
                service
                    .patch_repository(&principal, pid, "https://example.invalid/repo")
                    .await
                    .unwrap()["repositoryURL"],
                "https://example.invalid/repo"
            );
            let spare = service
                .save_customer(&principal, None, json!({"name":format!("{nonce}-delete")}))
                .await
                .unwrap();
            let spare = spare["id"].as_str().unwrap();
            let tenant: String = owner
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
                    [spare.into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "organization_id")
                .unwrap();
            assert!(
                matches!(
                    service.delete_customer(&principal, spare).await,
                    Err(RegistryError::Conflict(_))
                ),
                "an active customer is kept"
            );
            // Every customer starts with a Default project; it follows the customer while empty.
            let default_project = |status: &'static str| {
                let owner = &owner;
                async move {
                    scalar(owner, "SELECT count(*) AS n FROM webdock_admin.projects WHERE customer_id=$1 AND created_default AND name='Default' AND status::text=$2", vec![spare.into(), status.into()]).await
                }
            };
            assert_eq!(default_project("active").await, 1);
            let default_id: String = owner
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM webdock_admin.projects WHERE customer_id=$1",
                    [spare.into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "id")
                .unwrap();
            sql(&owner, "INSERT INTO webdock_admin.vercel_project_link(project_id,team_id,vercel_project_id,updated_by) VALUES($1,'team','prj','fixture')", vec![default_id.clone().into()]).await;
            assert!(
                matches!(
                    service.archive(&principal, "customers", spare, true).await,
                    Err(RegistryError::Invalid(_))
                ),
                "a Default project in use keeps the customer active"
            );
            sql(
                &owner,
                "DELETE FROM webdock_admin.vercel_project_link WHERE project_id=$1",
                vec![default_id.into()],
            )
            .await;
            service
                .archive(&principal, "customers", spare, true)
                .await
                .unwrap();
            assert_eq!(default_project("archived").await, 1);
            service
                .archive(&principal, "customers", spare, false)
                .await
                .unwrap();
            assert_eq!(
                default_project("active").await,
                1,
                "restored with its customer"
            );
            service
                .archive(&principal, "customers", spare, true)
                .await
                .unwrap();
            assert_eq!(
                service.delete_customer(&principal, spare).await.unwrap()["deleted"],
                true
            );
            for (table, column, id) in [
                ("webdock_admin.customers", "id", spare),
                ("webdock_admin.projects", "customer_id", spare),
                ("webdock_auth.tenant_customer", "customer_id", spare),
                ("webdock_auth.organization", "id", tenant.as_str()),
            ] {
                assert_eq!(
                    scalar(
                        &owner,
                        &format!("SELECT count(*) AS n FROM {table} WHERE {column}=$1"),
                        vec![id.into()]
                    )
                    .await,
                    0,
                    "{table} row removed"
                );
            }
            assert_eq!(
                scalar(
                    &owner,
                    "SELECT count(*) AS n FROM webdock_admin.audit_events WHERE target_i_d=$1 AND action='delete'",
                    vec![spare.into()]
                )
                .await,
                1
            );
            let leaked = runtime
                .query_one_raw(Statement::from_string(
                    DbBackend::Postgres,
                    "SELECT nullif(current_setting('webdock.rls_context',true),'') AS scope",
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<Option<String>>("", "scope")
                .unwrap();
            assert!(leaked.is_none());
            sql(
                &owner,
                "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
                vec![uid.clone().into()],
            )
            .await;
            assert!(
                service
                    .list(&principal, json!({"collection":"customers"}))
                    .await
                    .is_err(),
                "stale Principal cannot retain revoked operator access"
            );
        }
    };
    // Keep cleanup independent of assertions so a failed test does not erase other fixtures.
    let outcome = tokio::spawn(result).await;
    sql(
        &owner,
        "DELETE FROM webdock_admin.cms_instances WHERE schema_name=$1",
        vec![nonce.clone().into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_admin.audit_events WHERE actor_id=$1",
        vec![mapped.clone().into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_admin.projects WHERE name=$1",
        vec![nonce.clone().into()],
    )
    .await;
    sql(&owner,"DELETE FROM webdock_auth.tenant_customer WHERE customer_id IN (SELECT id FROM webdock_admin.customers WHERE name IN ($1,$2,$3))",vec![nonce.clone().into(),format!("{nonce}-rollback").into(),format!("{nonce}-delete").into()]).await;
    sql(
        &owner,
        "DELETE FROM webdock_admin.customers WHERE name IN ($1,$2,$3)",
        vec![
            nonce.clone().into(),
            format!("{nonce}-rollback").into(),
            format!("{nonce}-delete").into(),
        ],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.organization WHERE name=$1",
        vec![nonce.clone().into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_admin.users WHERE id=$1",
        vec![mapped.into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.session WHERE id=$1",
        vec![sid.into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        vec![uid.into()],
    )
    .await;
    outcome.unwrap();
}
