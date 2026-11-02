//! Self-service policy, gates and the resource index against the retained local fixture.
//! Each test creates uniquely named records and removes them; no schema reset and no provider I/O.
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use webdock_identity::{CookiePolicy, IdentityService, Principal, RlsSigner};

async fn db(key: &str) -> DatabaseConnection {
    let value = std::env::var(key).expect("explicit local fixture URL");
    let parsed = url::Url::parse(&value).unwrap();
    assert!(
        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"))
            && parsed.path() == "/webdock_rls_test"
    );
    Database::connect(value).await.unwrap()
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

/// A v2 product manifest: offering `note`, feature `create-notes`, create/use/delete actions.
pub fn product_manifest(name: &str) -> Value {
    let mut v: Value = serde_json::from_str(include_str!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    let resource = json!({"type":"object","additionalProperties":false,"required":["resource_id"],"properties":{"resource_id":{"type":"string","maxLength":128}}});
    v["name"] = json!(name);
    v["capabilities"] = json!([]);
    v["services"]["required"] = json!([]);
    v["jobs"] = json!([]);
    v["offerings"] =
        json!([{"id":"note","label":"Note","description":"A short note kept with the project."}]);
    v["selfService"] =
        json!({"features":[{"id":"create-notes","label":"Create notes","offering":"note"}]});
    v["actions"] = json!([
        {"name":"notes.create","intent":"write","audience":"customer","capabilities":[],"offering":"note","feature":"create-notes","resource":"create",
         "input":{"type":"object","additionalProperties":false,"properties":{"name":{"type":"string","maxLength":80}}}},
        {"name":"notes.view","intent":"read","audience":"customer","capabilities":[],"offering":"note","resource":"use","input":resource},
        {"name":"notes.rename","intent":"write","audience":"customer","capabilities":[],"offering":"note","feature":"create-notes","resource":"use","input":resource},
        {"name":"notes.delete","intent":"write","audience":"customer","capabilities":[],"offering":"note","feature":"create-notes","resource":"delete","input":resource},
        {"name":"notes.operator","intent":"read","audience":"operator","capabilities":[]}
    ]);
    v["areas"] = json!({
        "operator":{"pages":[{"id":"overview","label":"Overview","action":"notes.operator"}]},
        "selfService":{"pages":[{"id":"note","label":"Note","action":"notes.view","resource":true}]}
    });
    v
}

pub struct Fixture {
    pub owner: DatabaseConnection,
    pub nonce: String,
    pub plugin: String,
    pub customers: [String; 2],
    pub projects: [String; 2],
    users: Vec<String>,
    pub principals: std::collections::HashMap<&'static str, Principal>,
    pub bindings: tempfile::TempDir,
}
impl Fixture {
    /// Operator; owner, admin and member of customer A; owner of customer B.
    pub async fn new() -> Self {
        let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
        let nonce = format!(
            "self-service-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let plugin = nonce.clone();
        let cookies =
            CookiePolicy::new(false, "fixture-host-cookie-secret-at-least-32-bytes").unwrap();
        let identity = IdentityService::new(
            db("AUTH_DATABASE_URL").await,
            CookiePolicy::new(false, "fixture-host-cookie-secret-at-least-32-bytes").unwrap(),
        );
        let mut customers = vec![];
        let mut projects = vec![];
        let mut orgs = vec![];
        for label in ["a", "b"] {
            let cid = id(&owner).await;
            sql(
                &owner,
                "WITH hint AS MATERIALIZED (SELECT set_config('webdock.organization_allocation',jsonb_build_object('customer_id',$1::text,'organization_id',$3::text)::text,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,$2 FROM hint",
                vec![cid.clone().into(), format!("{nonce}-{label}").into(),id(&owner).await.into()],
            )
            .await;
            let pid = id(&owner).await;
            sql(
                &owner,
                "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,$2,$3)",
                vec![
                    pid.clone().into(),
                    format!("{nonce}-{label}").into(),
                    cid.clone().into(),
                ],
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
            projects.push(pid);
            orgs.push(org);
        }
        let mut users = vec![];
        let mut principals = std::collections::HashMap::new();
        for (label, role, org) in [
            ("operator", "operator", None),
            ("owner", "user", Some((0, "owner"))),
            ("admin", "user", Some((0, "admin"))),
            ("member", "user", Some((0, "member"))),
            ("foreign", "user", Some((1, "owner"))),
            ("preview", "operator", None),
            ("acting", "operator", None),
        ] {
            let uid = format!("{nonce}-{label}");
            let sid = format!("{uid}-session");
            let token = format!("{uid}-token");
            sql(&owner, "INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,$3,true,false)", vec![uid.clone().into(), format!("{uid}@example.invalid").into(), role.into()]).await;
            sql(&owner, "INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$2,$3,now()+interval '1 hour',now())", vec![sid.into(), token.clone().into(), uid.clone().into()]).await;
            if role == "operator" {
                sql(&owner, "INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$2)", vec![id(&owner).await.into(), uid.clone().into(), format!("{uid}@example.invalid").into()]).await;
            }
            if let Some((index, member_role)) = org {
                sql(&owner, "INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,$4,now())", vec![format!("{uid}-member").into(), orgs[index].clone().into(), uid.clone().into(), member_role.into()]).await;
            }
            if label == "preview" {
                sql(&owner, "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES($1,$2,$3,$4)", vec![format!("{uid}-session").into(), uid.clone().into(), customers[0].clone().into(), orgs[0].clone().into()]).await;
            }
            if label == "acting" {
                sql(&owner, "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES($1,$2,$3,$4,'owner')", vec![format!("{uid}-session").into(), uid.clone().into(), customers[0].clone().into(), orgs[0].clone().into()]).await;
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
        let manifest = product_manifest(&plugin);
        let raw = manifest.to_string();
        let digest = hex::encode(Sha256::digest(raw.as_bytes()));
        let credential = hex::encode(Sha256::digest(b"fixture-credential"));
        let image = manifest["image"].as_str().unwrap().to_owned();
        sql(&owner, "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','1.0.0','ready',1)", vec![plugin.clone().into()]).await;
        sql(&owner, "INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,'1.0.0',$2,$3,$4,'https://registry.invalid','fixture')", vec![plugin.clone().into(), raw.into(), digest.into(), image.clone().into()]).await;
        sql(&owner, "INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES($1,'[]','{}')", vec![plugin.clone().into()]).await;
        sql(&owner, "INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES($1,1,'1.0.0',$2,'ready',0,$3,'fixtures','fixture-plugin')", vec![plugin.clone().into(), image.into(), credential.clone().into()]).await;
        let bindings = tempfile::tempdir().unwrap();
        let path = bindings.path().join("bindings.json");
        let config = json!({"runtimes":{plugin.clone():{"generation":1,"namespace":"fixtures","service":"fixture-plugin","credentialSha256":credential,"peers":[{"kind":"unix","uid":rustix::process::geteuid().as_raw()}],"projects":{}}}});
        std::fs::write(&path, config.to_string()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            owner,
            nonce,
            plugin,
            customers: [customers[0].clone(), customers[1].clone()],
            projects: [projects[0].clone(), projects[1].clone()],
            users,
            principals,
            bindings,
        }
    }
    pub fn principal(&self, label: &str) -> &Principal {
        &self.principals[label]
    }
    /// Restricted Core session role with the signed actor, as request handlers use it.
    pub async fn session(&self, label: &str) -> sea_orm::DatabaseTransaction {
        RlsSigner::new(
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            "webdock_auth_runtime",
        )
        .unwrap()
        .begin(&db("DATABASE_URL").await, self.principal(label))
        .await
        .unwrap()
    }
    pub async fn cleanup(self) {
        let o = &self.owner;
        for uid in &self.users {
            sql(
                o,
                "DELETE FROM webdock_auth.studio_tenant_preview WHERE actor_id=$1",
                vec![uid.clone().into()],
            )
            .await;
        }
        for table in [
            "plugin_workload",
            "plugin_resource",
            "plugin_feature_customer",
            "plugin_feature_policy",
            "plugin_invocation",
            "plugin_grant",
            "plugin_runtime",
            "plugin_configuration",
            "plugin_release",
            "plugin_operation",
        ] {
            sql(
                o,
                &format!("DELETE FROM webdock_auth.{table} WHERE plugin_id=$1"),
                vec![self.plugin.clone().into()],
            )
            .await;
        }
        sql(
            o,
            "DELETE FROM webdock_auth.plugin_installation WHERE id=$1",
            vec![self.plugin.clone().into()],
        )
        .await;
        for pid in &self.projects {
            sql(
                o,
                "DELETE FROM webdock_admin.projects WHERE id=$1",
                vec![pid.clone().into()],
            )
            .await;
        }
        for cid in &self.customers {
            sql(
                o,
                "DELETE FROM webdock_admin.customers WHERE id=$1",
                vec![cid.clone().into()],
            )
            .await;
        }
        for uid in &self.users {
            sql(
                o,
                "DELETE FROM webdock_auth.access_event WHERE actor_id=$1",
                vec![uid.clone().into()],
            )
            .await;
            sql(
                o,
                "DELETE FROM webdock_auth.member WHERE \"userId\"=$1",
                vec![uid.clone().into()],
            )
            .await;
            sql(
                o,
                "DELETE FROM webdock_admin.users WHERE name=$1",
                vec![uid.clone().into()],
            )
            .await;
            sql(
                o,
                "DELETE FROM webdock_auth.session WHERE \"userId\"=$1",
                vec![uid.clone().into()],
            )
            .await;
            sql(
                o,
                "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
                vec![uid.clone().into()],
            )
            .await;
        }
        let _ = &self.nonce;
    }
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn self_service_tables_enforce_rls() {
    let f = Fixture::new().await;
    // Owner rows for both customers exist; the customer owner of A reads only A's allow-list row.
    for cid in &f.customers {
        sql(&f.owner, "INSERT INTO webdock_auth.plugin_feature_customer(plugin_id,feature_id,customer_id,added_by) VALUES($1,'create-notes',$2,'fixture')", vec![f.plugin.clone().into(), cid.clone().into()]).await;
    }
    let tx = f.session("owner").await;
    let rows = tx
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT customer_id FROM webdock_auth.plugin_feature_customer WHERE plugin_id=$1",
            [f.plugin.clone().into()],
        ))
        .await
        .unwrap();
    let seen: Vec<String> = rows
        .iter()
        .map(|r| r.try_get("", "customer_id").unwrap())
        .collect();
    assert_eq!(seen, vec![f.customers[0].clone()]);
    let denied = tx
        .execute_raw(Statement::from_sql_and_values(DbBackend::Postgres, "INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES($1,'create-notes','everyone','owner')", [f.plugin.clone().into()]))
        .await;
    assert!(
        denied.is_err(),
        "customers cannot change self-service policy"
    );
    tx.rollback().await.unwrap();
    let tx = f.session("owner").await;
    assert!(
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.bump_plugin_policy($1)",
            [f.plugin.clone().into()]
        ))
        .await
        .is_err(),
        "customers cannot bump the policy revision"
    );
    tx.rollback().await.unwrap();
    let tx = f.session("operator").await;
    tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres, "INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES($1,'create-notes','everyone','operator')", [f.plugin.clone().into()]))
        .await
        .unwrap();
    let revision: i64 = tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.bump_plugin_policy($1) AS r",
            [f.plugin.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "r")
        .unwrap();
    assert_eq!(revision, 1);
    assert!(
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres, "INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by) VALUES($1,'x','note',$2,$3,'x','operator','operator')", [f.plugin.clone().into(), f.customers[0].clone().into(), f.projects[0].clone().into()]))
            .await
            .is_err(),
        "the resource index is written only by the plugin controller"
    );
    tx.rollback().await.unwrap();
    f.cleanup().await;
}

use webdock_plugin_host::{AdmissionInput, FileBindings, HostService};
use webdock_plugin_runtime::PeerIdentity;

impl Fixture {
    pub async fn host(&self) -> std::sync::Arc<HostService> {
        std::sync::Arc::new(
            HostService::new(
                db("DATABASE_URL").await,
                db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
                std::env::var("DATABASE_RLS_SECRET").unwrap(),
                std::sync::Arc::new(webdock_persistence::IdAllocator::new(
                    db("DATABASE_URL").await,
                    webdock_ids::Identity::platform(0, 2).unwrap(),
                )),
                std::sync::Arc::new(FileBindings::new(
                    self.bindings.path().join("bindings.json"),
                )),
                ed25519_dalek::SigningKey::from_bytes(&[23; 32]),
            )
            .await
            .unwrap(),
        )
    }
    /// Operators change policy through the API (Task 4); tests write rows and bump the revision directly.
    pub async fn set_mode(&self, mode: &str, listed: &[usize]) {
        let o = &self.owner;
        sql(o, "INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES($1,'create-notes',$2,'fixture') ON CONFLICT (plugin_id,feature_id) DO UPDATE SET mode=EXCLUDED.mode", vec![self.plugin.clone().into(), mode.into()]).await;
        sql(
            o,
            "DELETE FROM webdock_auth.plugin_feature_customer WHERE plugin_id=$1",
            vec![self.plugin.clone().into()],
        )
        .await;
        for i in listed {
            sql(o, "INSERT INTO webdock_auth.plugin_feature_customer(plugin_id,feature_id,customer_id,added_by) VALUES($1,'create-notes',$2,'fixture')", vec![self.plugin.clone().into(), self.customers[*i].clone().into()]).await;
        }
        sql(o, "UPDATE webdock_auth.plugin_installation SET policy_revision=policy_revision+1 WHERE id=$1", vec![self.plugin.clone().into()]).await;
        sql(o, "UPDATE webdock_auth.plugin_grant SET revision=(SELECT policy_revision FROM webdock_auth.plugin_installation WHERE id=$1) WHERE plugin_id=$1", vec![self.plugin.clone().into()]).await;
    }
    pub fn input(&self, project: usize, action: &str, input: Value) -> AdmissionInput {
        serde_json::from_value(json!({"pluginID":self.plugin,"projectID":self.projects[project],"action":action,"input":input,"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap()
    }
    pub async fn resources(&self) -> Vec<(String, String, String)> {
        self.owner
            .query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT resource_id,project_id,created_via FROM webdock_auth.plugin_resource WHERE plugin_id=$1 ORDER BY resource_id", [self.plugin.clone().into()]))
            .await
            .unwrap()
            .iter()
            .map(|r| (r.try_get("", "resource_id").unwrap(), r.try_get("", "project_id").unwrap(), r.try_get("", "created_via").unwrap()))
            .collect()
    }
}
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
/// Admit, authenticate and record a create; returns the new resource id.
async fn create(f: &Fixture, host: &HostService, who: &str, project: usize, id: &str) -> String {
    let admitted = host
        .admit(
            f.principal(who),
            f.input(project, "notes.create", json!({"name":id})),
        )
        .await
        .unwrap();
    let handle = host.authenticate(peer(), &admitted.token).await.unwrap();
    host.record_resource(&handle, &json!({"resource":{"id":id,"name":id}}))
        .await
        .unwrap();
    host.revoke(&handle).await.unwrap();
    id.into()
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn policy_matrix_for_create() {
    let f = Fixture::new().await;
    let host = f.host().await;
    // (mode, listed customers) → which actors may create on customer A's project.
    let cases: [(&str, &[usize], &[&str]); 4] = [
        ("everyone", &[], &["operator", "owner", "admin"]),
        ("selected", &[0], &["operator", "owner", "admin"]),
        ("selected", &[1], &["operator"]),
        ("assigned", &[], &["operator"]),
    ];
    for (mode, listed, allowed) in cases {
        f.set_mode(mode, listed).await;
        for who in ["operator", "owner", "admin", "member", "foreign", "preview"] {
            let result = host
                .admit(
                    f.principal(who),
                    f.input(0, "notes.create", json!({"name":"x"})),
                )
                .await;
            assert_eq!(
                result.is_ok(),
                allowed.contains(&who),
                "{mode} {listed:?} {who}"
            );
        }
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn audience_and_resource_gates() {
    let f = Fixture::new().await;
    let host = f.host().await;
    f.set_mode("everyone", &[]).await;
    // operator_assigns_resource: operator creates on A, the A owner then uses it.
    let assigned = create(&f, &host, "operator", 0, "assigned-1").await;
    let foreign = create(&f, &host, "foreign", 1, "foreign-1").await;
    let mine = create(&f, &host, "owner", 0, "mine-1").await;
    assert_eq!(
        f.resources().await,
        vec![
            (assigned.clone(), f.projects[0].clone(), "operator".into()),
            (
                foreign.clone(),
                f.projects[1].clone(),
                "self-service".into()
            ),
            (mine.clone(), f.projects[0].clone(), "self-service".into()),
        ]
    );
    let view = |r: &str| json!({"resource_id": r});
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(0, "notes.view", view(&assigned))
        )
        .await
        .is_ok()
    );
    // member_cannot_create but may read resources of its customer.
    assert!(
        host.admit(f.principal("member"), f.input(0, "notes.view", view(&mine)))
            .await
            .is_ok()
    );
    assert!(
        host.admit(
            f.principal("member"),
            f.input(0, "notes.create", json!({"name":"m"}))
        )
        .await
        .is_err()
    );
    // resource_gate_rejects_foreign_resource: never by project mix-up, not even for operators.
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(0, "notes.view", view(&foreign))
        )
        .await
        .is_err()
    );
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(1, "notes.view", view(&foreign))
        )
        .await
        .is_err()
    );
    assert!(
        host.admit(
            f.principal("operator"),
            f.input(0, "notes.view", view(&foreign))
        )
        .await
        .is_err()
    );
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(0, "notes.view", view("missing"))
        )
        .await
        .is_err()
    );
    // Operator-only actions.
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(0, "notes.operator", json!({}))
        )
        .await
        .is_err()
    );
    assert!(
        host.admit(
            f.principal("operator"),
            f.input(0, "notes.operator", json!({}))
        )
        .await
        .is_ok()
    );
    // removed_customer_keeps_resources: use stays, create and feature-gated changes stop.
    f.set_mode("selected", &[1]).await;
    assert!(
        host.admit(f.principal("owner"), f.input(0, "notes.view", view(&mine)))
            .await
            .is_ok()
    );
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(0, "notes.rename", view(&mine))
        )
        .await
        .is_err()
    );
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(0, "notes.create", json!({"name":"y"}))
        )
        .await
        .is_err()
    );
    assert!(
        host.admit(
            f.principal("operator"),
            f.input(0, "notes.rename", view(&mine))
        )
        .await
        .is_ok()
    );
    // Deleting through the feature removes the index row.
    f.set_mode("everyone", &[]).await;
    let admitted = host
        .admit(
            f.principal("owner"),
            f.input(0, "notes.delete", view(&mine)),
        )
        .await
        .unwrap();
    let handle = host.authenticate(peer(), &admitted.token).await.unwrap();
    host.record_resource(&handle, &json!({})).await.unwrap();
    assert!(f.resources().await.iter().all(|(id, _, _)| id != &mine));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn policy_change_revokes_inflight_invocation() {
    let f = Fixture::new().await;
    let host = f.host().await;
    f.set_mode("everyone", &[]).await;
    let admitted = host
        .admit(
            f.principal("owner"),
            f.input(0, "notes.create", json!({"name":"z"})),
        )
        .await
        .unwrap();
    f.set_mode("assigned", &[]).await;
    assert!(host.authenticate(peer(), &admitted.token).await.is_err());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn create_output_requires_valid_resource() {
    let f = Fixture::new().await;
    let host = f.host().await;
    f.set_mode("everyone", &[]).await;
    for output in [
        json!({}),
        json!({"resource":{"id":"a/b","name":"x"}}),
        json!({"resource":{"id":"ok","name":""}}),
        json!({"resource":{"id":"ok"}}),
    ] {
        let admitted = host
            .admit(
                f.principal("owner"),
                f.input(0, "notes.create", json!({"name":"x"})),
            )
            .await
            .unwrap();
        let handle = host.authenticate(peer(), &admitted.token).await.unwrap();
        assert!(
            host.record_resource(&handle, &output).await.is_err(),
            "{output}"
        );
    }
    // A create action must not take a resource id from the caller.
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(0, "notes.create", json!({"name":"x","resource_id":"r"}))
        )
        .await
        .is_err()
    );
    assert!(f.resources().await.is_empty());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn operator_policy_api() {
    let f = Fixture::new().await;
    let host = f.host().await;
    let op = f.principal("operator");
    // operator_sets_mode_and_list
    let view = host.self_service(op, None, &f.plugin).await.unwrap();
    assert_eq!(view["revision"], "0");
    assert_eq!(view["features"][0]["id"], "create-notes");
    assert_eq!(view["features"][0]["mode"], "assigned");
    assert_eq!(view["features"][0]["offering"], "note");
    let changed = host
        .set_feature_mode(op, None, &f.plugin, "create-notes", "selected", "0")
        .await
        .unwrap();
    assert_eq!(changed["revision"], "1");
    let changed = host
        .set_feature_customer(
            op,
            None,
            &f.plugin,
            "create-notes",
            &f.customers[0],
            true,
            "1",
        )
        .await
        .unwrap();
    assert_eq!(changed["revision"], "2");
    let view = host.self_service(op, None, &f.plugin).await.unwrap();
    assert_eq!(view["features"][0]["mode"], "selected");
    assert_eq!(
        view["features"][0]["customers"][0]["id"],
        f.customers[0].as_str()
    );
    assert!(
        host.admit(
            f.principal("owner"),
            f.input(0, "notes.create", json!({"name":"a"}))
        )
        .await
        .is_ok()
    );
    // stale_revision_conflicts
    let stale = host
        .set_feature_mode(op, None, &f.plugin, "create-notes", "everyone", "1")
        .await
        .unwrap_err();
    assert_eq!(stale.code, webdock_plugin_protocol::ErrorCode::Conflict);
    // unknown_feature_rejected
    assert!(
        host.set_feature_mode(op, None, &f.plugin, "missing", "everyone", "2")
            .await
            .is_err()
    );
    assert!(
        host.set_feature_mode(op, None, &f.plugin, "create-notes", "public", "2")
            .await
            .is_err()
    );
    assert!(
        host.set_feature_customer(op, None, &f.plugin, "create-notes", "999999999", true, "2")
            .await
            .is_err()
    );
    // customer_cannot_change_policy
    for who in ["owner", "admin", "preview"] {
        assert!(
            host.set_feature_mode(
                f.principal(who),
                None,
                &f.plugin,
                "create-notes",
                "everyone",
                "2"
            )
            .await
            .is_err(),
            "{who}"
        );
        assert!(
            host.self_service(f.principal(who), None, &f.plugin)
                .await
                .is_err(),
            "{who}"
        );
        assert!(
            host.customer_self_service(f.principal(who), None, &f.customers[0])
                .await
                .is_err(),
            "{who}"
        );
    }
    // customer_view_lists_selected_features
    let listed = host
        .customer_self_service(op, None, &f.customers[0])
        .await
        .unwrap();
    let mine: Vec<&Value> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["plugin"] == f.plugin.as_str())
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["feature"], "create-notes");
    assert_eq!(mine[0]["mode"], "selected");
    assert_eq!(mine[0]["allowed"], true);
    let other = host
        .customer_self_service(op, None, &f.customers[1])
        .await
        .unwrap();
    assert!(
        other
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["plugin"] == f.plugin.as_str() && v["allowed"] == false)
    );
    // Removing the customer revokes its in-flight create.
    let admitted = host
        .admit(
            f.principal("owner"),
            f.input(0, "notes.create", json!({"name":"b"})),
        )
        .await
        .unwrap();
    host.set_feature_customer(
        op,
        None,
        &f.plugin,
        "create-notes",
        &f.customers[0],
        false,
        "2",
    )
    .await
    .unwrap();
    assert!(host.authenticate(peer(), &admitted.token).await.is_err());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn index_follows_what_the_plugin_did_even_if_policy_changed_meanwhile() {
    let f = Fixture::new().await;
    let host = f.host().await;
    f.set_mode("everyone", &[]).await;
    let admitted = host
        .admit(
            f.principal("owner"),
            f.input(0, "notes.create", json!({"name":"late"})),
        )
        .await
        .unwrap();
    let handle = host.authenticate(peer(), &admitted.token).await.unwrap();
    // The plugin has created the note; the operator takes creation back before Core records it.
    f.set_mode("assigned", &[]).await;
    host.record_resource(&handle, &json!({"resource":{"id":"late-1","name":"late"}}))
        .await
        .unwrap();
    assert_eq!(
        f.resources().await.len(),
        1,
        "the created note stays visible and deletable"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn operator_lists_resources_across_customers() {
    let f = Fixture::new().await;
    let host = f.host().await;
    f.set_mode("everyone", &[]).await;
    let a = create(&f, &host, "owner", 0, &format!("a-{}", f.nonce)).await;
    let b = create(&f, &host, "foreign", 1, &format!("b-{}", f.nonce)).await;
    let list = host
        .plugin_resources(f.principal("operator"), None, &f.plugin)
        .await
        .unwrap();
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let row = rows.iter().find(|r| r["id"] == a.as_str()).unwrap();
    assert_eq!(row["customer"]["id"], f.customers[0].as_str());
    assert_eq!(row["project"]["id"], f.projects[0].as_str());
    assert!(row["project"]["name"].is_string() && row["customer"]["name"].is_string());
    assert_eq!(row["offering"], "note");
    assert_eq!(row["offeringLabel"], "Note");
    assert_eq!(row["createdVia"], "self-service");
    assert!(rows.iter().any(|r| r["id"] == b.as_str()));
    for who in ["owner", "admin", "member", "foreign", "preview"] {
        assert!(
            host.plugin_resources(f.principal(who), None, &f.plugin)
                .await
                .is_err(),
            "{who}"
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn workloads_are_pinned_and_stored_for_the_runtime() {
    let f = Fixture::new().await;
    let o = &f.owner;
    let mut manifest = product_manifest(&f.plugin);
    manifest["capabilities"] = json!(["plugin.workloads"]);
    manifest["workloads"] = json!({"kinds": ["apps/v1/Deployment", "v1/Service"]});
    let raw = manifest.to_string();
    sql(o, "UPDATE webdock_auth.plugin_release SET manifest_bytes=$2,manifest_sha256=$3 WHERE plugin_id=$1", vec![f.plugin.clone().into(), raw.clone().into(), hex::encode(Sha256::digest(raw.as_bytes())).into()]).await;
    let cluster = id(o).await;
    sql(o, "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence) VALUES($1,$2,'k3s','eu-central','fixture')", vec![cluster.clone().into(), format!("{}-cluster", f.nonce).into()]).await;
    let host = f.host().await;
    let service = json!([{"apiVersion":"v1","kind":"Service","metadata":{"name":"web"},"spec":{"ports":[{"port":80}]}}]);
    let apply = |project: usize| json!({"project": f.projects[project], "unit": "web-1", "objects": service});
    // Not approved yet: the capability is the operator's ceiling.
    assert!(
        host.workloads(&peer(), &f.plugin, 1, "apply", apply(0))
            .await
            .is_err()
    );
    sql(o, "UPDATE webdock_auth.plugin_configuration SET approved_capabilities='[\"plugin.workloads\"]' WHERE plugin_id=$1", vec![f.plugin.clone().into()]).await;
    // Approved, but no cluster assigned in the runtime binding.
    assert!(
        host.workloads(&peer(), &f.plugin, 1, "apply", apply(0))
            .await
            .is_err()
    );
    let path = f.bindings.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"][&f.plugin]["workloadCluster"] = json!(cluster);
    std::fs::write(&path, bindings.to_string()).unwrap();

    let first = host
        .workloads(&peer(), &f.plugin, 1, "apply", apply(0))
        .await
        .unwrap();
    assert_eq!(first["revision"], 1);
    let namespace = first["namespace"].as_str().unwrap().to_owned();
    assert!(namespace.starts_with("wd-") && namespace.ends_with(&f.projects[0]));
    assert_eq!(
        host.workloads(&peer(), &f.plugin, 1, "apply", apply(0))
            .await
            .unwrap()["revision"],
        2
    );
    let stored: Value = o.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT objects,cluster_id FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit='web-1'", [f.plugin.clone().into()])).await.unwrap().unwrap().try_get("", "objects").unwrap();
    assert_eq!(stored[0]["metadata"]["namespace"], namespace.as_str());
    assert_eq!(stored[0]["metadata"]["labels"]["webdock.dev/unit"], "web-1");
    // A unit stays in its project; other runtimes and other peers are refused.
    assert!(
        host.workloads(&peer(), &f.plugin, 1, "apply", apply(1))
            .await
            .is_err()
    );
    assert!(
        host.workloads(
            &PeerIdentity::Unix { uid: 0 },
            &f.plugin,
            1,
            "apply",
            apply(0)
        )
        .await
        .is_err()
    );
    assert!(
        host.workloads(&peer(), &f.plugin, 2, "observe", json!({"unit":"web-1"}))
            .await
            .is_err()
    );
    let observed = host
        .workloads(&peer(), &f.plugin, 1, "observe", json!({"unit":"web-1"}))
        .await
        .unwrap();
    assert_eq!(
        (
            observed["revision"].as_i64(),
            observed["appliedRevision"].as_i64()
        ),
        (Some(2), Some(0))
    );
    host.workloads(&peer(), &f.plugin, 1, "remove", json!({"unit":"web-1"}))
        .await
        .unwrap();
    let removing = host
        .workloads(&peer(), &f.plugin, 1, "observe", json!({"unit":"web-1"}))
        .await
        .unwrap();
    assert_eq!(
        (
            removing["removing"].as_bool(),
            removing["revision"].as_i64()
        ),
        (Some(true), Some(3))
    );
    // The agent reports only through its own scope; the owner session cannot fake it.
    let reported: bool = o
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.report_plugin_workload($1,'web-1',3,NULL,NULL,true) AS ok",
            [f.plugin.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "ok")
        .unwrap();
    assert!(!reported);
    sql(
        o,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    sql(
        o,
        "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
        vec![cluster.into()],
    )
    .await;
    f.cleanup().await;
}

/// Approve workloads for the fixture plugin on a fresh fixture cluster; returns the cluster id.
async fn enable_workloads(f: &Fixture) -> String {
    let o = &f.owner;
    let mut manifest = product_manifest(&f.plugin);
    manifest["capabilities"] = json!(["plugin.workloads"]);
    manifest["workloads"] = json!({"kinds": ["apps/v1/Deployment", "v1/Service"]});
    let raw = manifest.to_string();
    sql(o, "UPDATE webdock_auth.plugin_release SET manifest_bytes=$2,manifest_sha256=$3 WHERE plugin_id=$1", vec![f.plugin.clone().into(), raw.clone().into(), hex::encode(Sha256::digest(raw.as_bytes())).into()]).await;
    sql(o, "UPDATE webdock_auth.plugin_configuration SET approved_capabilities='[\"plugin.workloads\"]' WHERE plugin_id=$1", vec![f.plugin.clone().into()]).await;
    let cluster = id(o).await;
    sql(o, "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence) VALUES($1,$2,'k3s','eu-central','fixture')", vec![cluster.clone().into(), format!("{}-cluster", f.nonce).into()]).await;
    let path = f.bindings.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"][&f.plugin]["workloadCluster"] = json!(cluster);
    std::fs::write(&path, bindings.to_string()).unwrap();
    cluster
}
fn sized(memory: &str, replicas: u64) -> Value {
    json!([{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"web"},
        "spec":{"replicas":replicas,"selector":{"matchLabels":{"app":"web"}},"template":{"metadata":{"labels":{"app":"web"}},
        "spec":{"containers":[{"name":"web","image":"nginx:stable-alpine","resources":{"limits":{"memory":memory},"requests":{"cpu":"50m"}}}]}}}}])
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn plans_limit_creates_workloads_and_projects() {
    let f = Fixture::new().await;
    let o = &f.owner;
    f.set_mode("everyone", &[]).await;
    let cluster = enable_workloads(&f).await;
    let host = f.host().await;
    let meter = format!("{}.note", f.plugin);
    let input = |limits: Value| {
        serde_json::from_value::<webdock_plugin_host::PlanInput>(
            json!({"name": format!("{}-plan", f.nonce), "limits": limits}),
        )
        .unwrap()
    };
    // Only operators manage plans; limits are whole numbers.
    assert!(
        host.save_plan(f.principal("owner"), None, None, input(json!({})))
            .await
            .is_err()
    );
    assert_eq!(
        host.save_plan(
            f.principal("operator"),
            None,
            None,
            input(json!({"memory": -1}))
        )
        .await
        .unwrap_err()
        .message,
        "Limits are whole numbers of zero or more"
    );
    let plan = host
        .save_plan(
            f.principal("operator"),
            None,
            None,
            input(json!({meter.clone(): 1, "memory": 512, "projects": 2})),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed = host.plans(f.principal("operator"), None).await.unwrap();
    assert!(
        listed["plans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == plan.as_str())
    );
    assert!(
        listed["meters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == meter.as_str() && m["label"] == "Note")
    );
    let assign = |overrides: Value| {
        serde_json::from_value::<webdock_plugin_host::CustomerPlanInput>(
            json!({"planId": plan, "overrides": overrides}),
        )
        .unwrap()
    };
    assert!(
        host.set_customer_plan(
            f.principal("owner"),
            None,
            &f.customers[0],
            assign(json!({}))
        )
        .await
        .is_err()
    );
    host.set_customer_plan(
        f.principal("operator"),
        None,
        &f.customers[0],
        assign(json!({})),
    )
    .await
    .unwrap();
    // A plan with customers stays.
    assert_eq!(
        host.delete_plan(f.principal("operator"), None, &plan)
            .await
            .unwrap_err()
            .message,
        "Customers are still on this plan"
    );

    // Counts: one note is allowed, the second is refused at admission, for operators too.
    create(&f, &host, "owner", 0, "note-1").await;
    for who in ["owner", "operator"] {
        let refused = host
            .admit(
                f.principal(who),
                f.input(0, "notes.create", json!({"name":"x"})),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(
            refused.message, "Your plan allows 1 of these and 1 are in use.",
            "{who}"
        );
    }
    // Customer B has no plan and no default plan exists: unlimited.
    create(&f, &host, "foreign", 1, "note-b1").await;
    create(&f, &host, "foreign", 1, "note-b2").await;

    // Memory: 300 MiB fits, a second 300 MiB unit does not; resizing the first within 512 does.
    let apply = |unit: &str, objects: Value| json!({"project": f.projects[0], "unit": unit, "objects": objects, "resource": "note-1"});
    host.workloads(
        &peer(),
        &f.plugin,
        1,
        "apply",
        apply("web-1", sized("300Mi", 1)),
    )
    .await
    .unwrap();
    let refused = host
        .workloads(
            &peer(),
            &f.plugin,
            1,
            "apply",
            apply("web-2", sized("300Mi", 1)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.message,
        "Your plan allows 512 MiB memory; 300 MiB are in use and this needs 300 MiB."
    );
    host.workloads(
        &peer(),
        &f.plugin,
        1,
        "apply",
        apply("web-1", sized("512Mi", 1)),
    )
    .await
    .unwrap();
    // A plan downgrade leaves the customer over; stopping still works, growing does not.
    sql(
        o,
        "UPDATE webdock_auth.plan SET limits=limits||'{\"memory\":256}' WHERE id=$1",
        vec![plan.clone().into()],
    )
    .await;
    host.workloads(
        &peer(),
        &f.plugin,
        1,
        "apply",
        apply("web-1", sized("512Mi", 0)),
    )
    .await
    .unwrap();
    assert!(
        host.workloads(
            &peer(),
            &f.plugin,
            1,
            "apply",
            apply("web-1", sized("512Mi", 1))
        )
        .await
        .is_err()
    );
    // An override lifts the limit for this customer only.
    host.set_customer_plan(
        f.principal("operator"),
        None,
        &f.customers[0],
        assign(json!({"memory": null})),
    )
    .await
    .unwrap();
    host.workloads(
        &peer(),
        &f.plugin,
        1,
        "apply",
        apply("web-1", sized("512Mi", 2)),
    )
    .await
    .unwrap();

    // Projects: the Default project and the fixture project already make two.
    let third = id(o).await;
    let error = o
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,'Third',$2)",
            [third.into(), f.customers[0].clone().into()],
        ))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("The plan allows 2 projects."), "{error}");

    // Members read their own usage: limits, what is allocated and each unit with its resource.
    let usage = host
        .usage(f.principal("member"), &f.customers[0])
        .await
        .unwrap();
    assert_eq!(usage["plan"]["id"], plan.as_str());
    // The override lifted memory; 256 stays on the plan.
    assert_eq!(usage["limits"], json!({meter.clone(): 1, "projects": 2}));
    assert_eq!(usage["overrides"], json!({"memory": null}));
    assert_eq!(usage["allocated"]["memory"], 1024);
    assert_eq!(usage["allocated"][&meter], 1);
    assert_eq!(usage["units"][0]["resource"]["name"], "note-1");
    assert!(
        host.usage(f.principal("foreign"), &f.customers[0])
            .await
            .is_err()
    );
    // Nobody else does.
    let foreign = f.session("foreign").await;
    assert!(
        foreign
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.customer_usage($1) AS u",
                [f.customers[0].clone().into()]
            ))
            .await
            .is_err()
    );
    foreign.rollback().await.unwrap();

    sql(
        o,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    sql(
        o,
        "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
        vec![cluster.into()],
    )
    .await;
    sql(
        o,
        "DELETE FROM webdock_auth.customer_plan WHERE plan_id=$1",
        vec![plan.clone().into()],
    )
    .await;
    host.delete_plan(f.principal("operator"), None, &plan)
        .await
        .unwrap();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn units_route_only_their_domains_and_verified_ones() {
    let f = Fixture::new().await;
    let o = &f.owner;
    let cluster = enable_workloads(&f).await;
    let mut manifest = product_manifest(&f.plugin);
    manifest["capabilities"] = json!(["plugin.workloads"]);
    manifest["workloads"] =
        json!({"kinds": ["traefik.io/v1alpha1/IngressRoute", "cert-manager.io/v1/Certificate"]});
    let raw = manifest.to_string();
    sql(o, "UPDATE webdock_auth.plugin_release SET manifest_bytes=$2,manifest_sha256=$3 WHERE plugin_id=$1", vec![f.plugin.clone().into(), raw.clone().into(), hex::encode(Sha256::digest(raw.as_bytes())).into()]).await;
    let path = f.bindings.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"][&f.plugin]["routeDomains"] = json!(["apps.localhost"]);
    std::fs::write(&path, bindings.to_string()).unwrap();
    let host = f.host().await;
    let route = |project: usize, hosts: &[&str]| {
        let rule = hosts
            .iter()
            .map(|h| format!("Host(`{h}`)"))
            .collect::<Vec<_>>()
            .join(" || ");
        json!({"project": f.projects[project], "unit": format!("web-{project}"), "objects": [
            {"apiVersion":"traefik.io/v1alpha1","kind":"IngressRoute","metadata":{"name":"web"},"spec":{"routes":[{"kind":"Rule","match":rule,"services":[{"name":"web","port":80}]}]}},
            {"apiVersion":"cert-manager.io/v1","kind":"Certificate","metadata":{"name":"web"},"spec":{"secretName":"web","dnsNames":hosts,"issuerRef":{"name":"ca","kind":"ClusterIssuer"}}}]})
    };
    // The plugin's own route domain needs nobody's verification.
    host.workloads(
        &peer(),
        &f.plugin,
        1,
        "apply",
        route(0, &["shop-1.apps.localhost"]),
    )
    .await
    .unwrap();
    // A customer's domain needs that customer's verification first.
    let refused = host
        .workloads(
            &peer(),
            &f.plugin,
            1,
            "apply",
            route(0, &["shop-1.apps.localhost", "www.shop.example.test"]),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.message,
        "Verify the domain of www.shop.example.test before using it."
    );
    let claim = |customer: usize, verified: bool| {
        let customer = f.customers[customer].clone();
        async move {
            let domain_id = id(o).await;
            sql(o, &format!("INSERT INTO webdock_auth.customer_domain(id,customer_id,domain,token,created_by,verified_at) VALUES($1,$2,'shop.example.test',$3,'fixture',{})", if verified { "now()" } else { "NULL" }), vec![domain_id.into(), customer.into(), "0".repeat(32).into()]).await;
        }
    };
    claim(0, false).await;
    assert!(
        host.workloads(
            &peer(),
            &f.plugin,
            1,
            "apply",
            route(0, &["www.shop.example.test"])
        )
        .await
        .is_err(),
        "claimed is not verified"
    );
    // Verified by customer B: still not customer A's.
    claim(1, true).await;
    assert!(
        host.workloads(
            &peer(),
            &f.plugin,
            1,
            "apply",
            route(0, &["www.shop.example.test"])
        )
        .await
        .is_err()
    );
    host.workloads(
        &peer(),
        &f.plugin,
        1,
        "apply",
        route(1, &["www.shop.example.test", "shop.example.test"]),
    )
    .await
    .unwrap();
    let stored: Vec<String> = o
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT hosts FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit='web-1'",
            [f.plugin.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "hosts")
        .unwrap();
    assert_eq!(stored, ["shop.example.test", "www.shop.example.test"]);
    // Customer B's members see which unit uses the domain.
    let session = f.session("foreign").await;
    let users: Value = session
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.domain_users($1,'shop.example.test') AS u",
            [f.customers[1].clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "u")
        .unwrap();
    assert_eq!(users[0]["unit"], "web-1");
    session.rollback().await.unwrap();
    // A lookalike domain is not a subdomain.
    assert!(
        host.workloads(
            &peer(),
            &f.plugin,
            1,
            "apply",
            route(1, &["evilshop.example.test"])
        )
        .await
        .is_err()
    );

    sql(o, "DELETE FROM webdock_auth.customer_domain WHERE domain='shop.example.test' AND customer_id = ANY($1)", vec![vec![f.customers[0].clone(), f.customers[1].clone()].into()]).await;
    sql(
        o,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    sql(
        o,
        "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
        vec![cluster.into()],
    )
    .await;
    f.cleanup().await;
}

struct Mails(std::sync::Mutex<Vec<webdock_identity::DeliveryRequest>>);
#[async_trait::async_trait]
impl webdock_identity::Delivery for Mails {
    async fn deliver(
        &self,
        request: &webdock_identity::DeliveryRequest,
    ) -> Result<(), webdock_identity::DeliveryError> {
        // Account mail accepts record ids only (they become its idempotency keys).
        if !(1..=19).contains(&request.id.len()) || !request.id.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(webdock_identity::DeliveryError::Unavailable);
        }
        self.0.lock().unwrap().push(request.clone());
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn reports_open_and_resolve_notices_mailed_once() {
    let f = Fixture::new().await;
    let o = &f.owner;
    let cluster = enable_workloads(&f).await;
    let host = f.host().await;
    host.workloads(
        &peer(),
        &f.plugin,
        1,
        "apply",
        json!({"project": f.projects[0], "unit": "web-1", "objects": sized("128Mi", 1)}),
    )
    .await
    .unwrap();
    // The agent's report, as the database sees it (the agent role is tested with the agent transport).
    let report = |state: Value, failure: Option<&str>| {
        let (plugin, failure) = (f.plugin.clone(), failure.map(str::to_owned));
        async move {
            sql(o, "SELECT webdock_auth.notice(w, webdock_auth.attention($2,$3)) FROM webdock_auth.plugin_workload w WHERE w.plugin_id=$1 AND w.unit='web-1'", vec![plugin.into(), state.into(), failure.into()]).await;
        }
    };
    let open = || async {
        o.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT kind FROM webdock_auth.notification WHERE plugin_id=$1 AND resolved_at IS NULL ORDER BY kind", [f.plugin.clone().into()]))
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String>("", "kind").unwrap())
            .collect::<Vec<_>>()
    };
    let crash = json!({"ready": false, "problems": [{"reason": "CrashLoopBackOff"}],
        "objects": [{"kind": "cert-manager.io/v1/Certificate", "ready": false}]});
    report(crash.clone(), None).await;
    report(crash, None).await;
    assert_eq!(
        open().await,
        ["certificate", "crashing"],
        "one notice per kind, however often reported"
    );
    report(
        json!({"ready": false, "problems": [{"reason": "ImagePullBackOff"}]}),
        Some("denied"),
    )
    .await;
    assert_eq!(
        open().await,
        ["image", "refused"],
        "what the report no longer shows resolves"
    );

    assert_eq!(
        host.notifications(f.principal("member"), &f.customers[0])
            .await
            .unwrap(),
        json!([]),
        "fresh notices stay quiet"
    );
    // Due after two minutes: owners and admins get one fixed mail each, members do not.
    let mails = Mails(Default::default());
    // Notices other fixtures left behind go out first.
    while host
        .dispatch_notifications(&mails, "http://127.0.0.1:1/")
        .await
        .unwrap()
        > 0
    {}
    mails.0.lock().unwrap().clear();
    assert_eq!(
        host.dispatch_notifications(&mails, "http://127.0.0.1:1/")
            .await
            .unwrap(),
        0,
        "not yet due"
    );
    sql(o, "UPDATE webdock_auth.notification SET opened_at=now()-interval '3 minutes' WHERE plugin_id=$1", vec![f.plugin.clone().into()]).await;
    assert_eq!(
        host.dispatch_notifications(&mails, "http://127.0.0.1:1/")
            .await
            .unwrap(),
        2
    );
    let mut to: Vec<String> = mails
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|m| m.email.clone())
        .collect();
    to.sort();
    to.dedup();
    assert_eq!(
        to,
        [
            format!("{}-admin@example.invalid", f.nonce),
            format!("{}-owner@example.invalid", f.nonce)
        ]
    );
    assert!(
        mails
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|m| m.kind == webdock_identity::DeliveryKind::Notice)
    );
    assert_eq!(
        host.dispatch_notifications(&mails, "http://127.0.0.1:1/")
            .await
            .unwrap(),
        0,
        "mailed once"
    );

    // Members read them once they lasted long enough, newest open first; other customers do not.
    let listed = host
        .notifications(f.principal("member"), &f.customers[0])
        .await
        .unwrap();
    let kinds: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["resolvedAt"].is_null())
        .map(|n| n["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.len(), 2);
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["kind"] == "crashing" && !n["resolvedAt"].is_null())
    );
    assert!(
        host.notifications(f.principal("foreign"), &f.customers[0])
            .await
            .is_err()
    );
    // Healthy again: everything resolves.
    report(json!({"ready": true, "problems": [], "objects": [{"kind": "cert-manager.io/v1/Certificate", "ready": true}]}), None).await;
    assert!(open().await.is_empty());

    sql(
        o,
        "DELETE FROM webdock_auth.notification WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    sql(
        o,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    sql(
        o,
        "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
        vec![cluster.into()],
    )
    .await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn log_history_is_for_owners_and_admins_and_searchable() {
    let f = Fixture::new().await;
    let o = &f.owner;
    for (at, line) in [
        ("2 seconds", "GET / 200"),
        ("1 second", "ERROR database unreachable"),
        ("0 seconds", "GET /health 200"),
    ] {
        sql(o, "INSERT INTO webdock_auth.workload_log(plugin_id,unit,project_id,resource_id,at,line) VALUES($1,'web-1',$2,'note-1',date_trunc('minute',now())-$3::interval,$4)", vec![f.plugin.clone().into(), f.projects[0].clone().into(), at.into(), line.into()]).await;
    }
    let host = f.host().await;
    let read = |who: &'static str, search: &'static str, before: Option<String>| {
        let host = host.clone();
        let (f, project) = (&f, f.projects[0].clone());
        async move {
            host.logs(
                f.principal(who),
                &project,
                &f.plugin,
                "note-1",
                search,
                before.as_deref(),
            )
            .await
        }
    };
    let lines = |v: Value| {
        v.as_array()
            .unwrap()
            .iter()
            .map(|l| l["line"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        lines(read("owner", "", None).await.unwrap()),
        ["GET /health 200", "ERROR database unreachable", "GET / 200"]
    );
    assert_eq!(
        lines(read("admin", "error", None).await.unwrap()),
        ["ERROR database unreachable"]
    );
    let first = read("owner", "", None).await.unwrap();
    assert_eq!(
        lines(
            read(
                "operator",
                "",
                Some(first[1]["id"].as_str().unwrap().to_owned())
            )
            .await
            .unwrap()
        ),
        ["GET / 200"]
    );
    // A search is text, not a pattern.
    assert!(lines(read("owner", "%", None).await.unwrap()).is_empty());
    for who in ["member", "foreign"] {
        assert!(read(who, "", None).await.is_err(), "{who}");
    }
    assert!(
        read("owner", "", Some("yesterday".to_owned()))
            .await
            .is_err()
    );
    // Expired logs of a quiet or removed unit must not remain visible until another log arrives.
    sql(o, "INSERT INTO webdock_auth.workload_log(plugin_id,unit,project_id,resource_id,at,line) VALUES($1,'removed-unit',$2,'note-1',now()-interval '4 days','expired')", vec![f.plugin.clone().into(), f.projects[0].clone().into()]).await;
    assert_eq!(
        lines(read("owner", "expired", None).await.unwrap()),
        Vec::<String>::new()
    );
    host.prune_history().await.unwrap();
    let expired: i64 = o.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT count(*) AS n FROM webdock_auth.workload_log WHERE plugin_id=$1 AND line='expired'", [f.plugin.clone().into()]))
        .await.unwrap().unwrap().try_get("", "n").unwrap();
    assert_eq!(
        expired, 0,
        "retention removes stored logs without a new report"
    );
    // Several units can produce more than one page at the same timestamp.
    sql(o, "INSERT INTO webdock_auth.workload_log(plugin_id,unit,project_id,resource_id,at,line) SELECT $1,'burst-unit',$2,'note-1',date_trunc('minute',now()),'burst '||n FROM generate_series(1,501) n", vec![f.plugin.clone().into(), f.projects[0].clone().into()]).await;
    let page = read("owner", "burst", None).await.unwrap();
    assert_eq!(page.as_array().unwrap().len(), 500);
    let before = page[499]["id"].as_str().unwrap().to_owned();
    let older = read("owner", "burst", Some(before)).await.unwrap();
    assert_eq!(
        older.as_array().unwrap().len(),
        1,
        "paging must not drop tied timestamps"
    );
    sql(
        o,
        "DELETE FROM webdock_auth.workload_log WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn projects_choose_offered_clusters_while_empty() {
    let f = Fixture::new().await;
    let o = &f.owner;
    let default = enable_workloads(&f).await;
    let host = f.host().await;
    let mut clusters = Vec::new();
    for (label, offered, dedicated) in [
        ("offered", true, None),
        ("dedicated", false, Some(1)),
        ("hidden", false, None),
    ] {
        let cluster = id(o).await;
        let dedicated: Option<String> = dedicated.map(|i: usize| f.customers[i].clone());
        sql(o, "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,offered,dedicated_customer_id) VALUES($1,$2,'k3s','eu-central','fixture',true,$3,$4)", vec![cluster.clone().into(), format!("{}-{label}", f.nonce).into(), offered.into(), dedicated.into()]).await;
        clusters.push(cluster);
    }
    let names = |v: &Value| {
        v["options"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["name"].as_str().unwrap().starts_with(&f.nonce))
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    // Customer A sees the offered cluster, B also the one dedicated to it; nobody sees the hidden one.
    let a = host
        .placement(f.principal("member"), &f.projects[0])
        .await
        .unwrap();
    assert_eq!(names(&a), [clusters[0].clone()]);
    assert!(a["cluster"].is_null() && a["movable"] == true);
    // A first apply that overlaps an uncommitted placement change must use the new cluster.
    let choosing = f.session("owner").await;
    let pid: i32 = choosing
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    sql(
        &choosing,
        "SELECT webdock_rls.set_project_cluster($1,$2)",
        vec![f.projects[0].clone().into(), clusters[0].clone().into()],
    )
    .await;
    let apply_host = host.clone();
    let (apply_plugin, apply_project) = (f.plugin.clone(), f.projects[0].clone());
    let applying =
        tokio::spawn(async move {
            apply_host.workloads(&peer(), &apply_plugin, 1, "apply",
            json!({"project":apply_project,"unit":"overlapping","objects":sized("128Mi",1)})).await
        });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = o.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))) AS waiting", [pid.into()]))
                .await.unwrap().unwrap().try_get("", "waiting").unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("apply reached the placement transaction's lock");
    choosing.commit().await.unwrap();
    applying.await.unwrap().unwrap();
    let placed: String = o.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT cluster_id FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit='overlapping'", [f.plugin.clone().into()]))
        .await.unwrap().unwrap().try_get("", "cluster_id").unwrap();
    sql(
        o,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit='overlapping'",
        vec![f.plugin.clone().into()],
    )
    .await;
    host.place(f.principal("owner"), &f.projects[0], None)
        .await
        .unwrap();
    assert_eq!(
        placed, clusters[0],
        "first apply must read the committed placement under the project lock"
    );
    assert_eq!(
        names(
            &host
                .placement(f.principal("foreign"), &f.projects[1])
                .await
                .unwrap()
        )
        .len(),
        2
    );
    assert!(
        host.placement(f.principal("foreign"), &f.projects[0])
            .await
            .is_err()
    );
    // Owners choose; members cannot; unavailable clusters are refused in words.
    assert!(
        host.place(f.principal("member"), &f.projects[0], Some(&clusters[0]))
            .await
            .is_err()
    );
    for refused in [&clusters[1], &clusters[2]] {
        assert_eq!(
            host.place(f.principal("owner"), &f.projects[0], Some(refused))
                .await
                .unwrap_err()
                .message,
            "This cluster is not available to this workspace."
        );
    }
    assert_eq!(
        host.place(f.principal("owner"), &f.projects[0], Some(&clusters[0]))
            .await
            .unwrap()["cluster"],
        clusters[0].as_str()
    );
    // New units follow the project, not the plugin's default.
    host.workloads(
        &peer(),
        &f.plugin,
        1,
        "apply",
        json!({"project": f.projects[0], "unit": "web-1", "objects": sized("128Mi", 1)}),
    )
    .await
    .unwrap();
    let placed: String = o.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT cluster_id FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit='web-1'", [f.plugin.clone().into()])).await.unwrap().unwrap().try_get("", "cluster_id").unwrap();
    assert_eq!(placed, clusters[0]);
    assert_ne!(placed, default);
    // With services running the project stays put.
    assert_eq!(
        host.place(f.principal("owner"), &f.projects[0], None)
            .await
            .unwrap_err()
            .message,
        "Services already run in this project; it stays on its cluster."
    );
    assert_eq!(
        host.placement(f.principal("owner"), &f.projects[0])
            .await
            .unwrap()["movable"],
        false
    );
    // Operators see every cluster and choose what they offer.
    let listed = host.clusters(f.principal("operator"), None).await.unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == clusters[0].as_str() && c["projects"] == 1)
    );
    assert!(host.clusters(f.principal("owner"), None).await.is_err());
    host.offer_cluster(f.principal("operator"), None, &clusters[2], true)
        .await
        .unwrap();
    assert_eq!(
        names(
            &host
                .placement(f.principal("member"), &f.projects[0])
                .await
                .unwrap()
        )
        .len(),
        2
    );
    assert!(
        host.offer_cluster(f.principal("owner"), None, &clusters[2], false)
            .await
            .is_err()
    );

    sql(
        o,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    sql(
        o,
        "UPDATE webdock_admin.projects SET cluster_id=NULL WHERE id=$1",
        vec![f.projects[0].clone().into()],
    )
    .await;
    for cluster in clusters.into_iter().chain([default]) {
        sql(
            o,
            "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
            vec![cluster.into()],
        )
        .await;
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires local retained-schema restricted roles and signed RLS fixture"]
async fn acting_owner_plugin_admission_stays_customer_scoped() {
    let f = Fixture::new().await;
    let host = f.host().await;
    f.set_mode("everyone", &[]).await;
    let admitted = host
        .admit(
            f.principal("acting"),
            f.input(0, "notes.create", json!({"name":"acting"})),
        )
        .await
        .unwrap();
    assert_eq!(admitted.claims.customer_id, f.customers[0]);
    assert!(
        host.admit(
            f.principal("acting"),
            f.input(1, "notes.create", json!({"name":"foreign"}))
        )
        .await
        .is_err()
    );
    assert!(
        host.admit(
            f.principal("acting"),
            f.input(0, "notes.operator", json!({}))
        )
        .await
        .is_err()
    );
    let handle = host.authenticate(peer(), &admitted.token).await.unwrap();
    host.record_resource(
        &handle,
        &json!({"resource":{"id":"acting","name":"Acting"}}),
    )
    .await
    .unwrap();
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
        vec![f.principal("acting").session_id.clone().into()],
    )
    .await;
    assert!(host.revalidate(&handle).await.is_err());
    f.cleanup().await;
}
