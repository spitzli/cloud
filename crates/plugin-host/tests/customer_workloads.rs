//! Customer infrastructure has no project anchor; all calls use the real restricted host boundary.
#[path = "self_service.rs"]
mod shared;
use hmac::{Hmac, Mac};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shared::Fixture;
use std::sync::Arc;
use webdock_plugin_host::HostService;
use webdock_plugin_runtime::PeerIdentity;

async fn sql(db: &impl ConnectionTrait, query: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await
    .unwrap();
}
async fn value(db: &impl ConnectionTrait, query: &str, values: Vec<sea_orm::Value>) -> Value {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "value")
    .unwrap()
}
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
fn objects(memory: u64) -> Value {
    json!([{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"service"},"spec":{"replicas":1,"template":{"metadata":{"labels":{"app":"service"}},"spec":{"containers":[{"name":"service","image":"fixture.invalid/service:1","resources":{"limits":{"memory":format!("{memory}Mi")},"requests":{"cpu":"50m"}}}]}}}}])
}
async fn agent(db: &sea_orm::DatabaseConnection, cluster: &str) -> sea_orm::DatabaseTransaction {
    let tx = db.begin().await.unwrap();
    let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":"agent","clusterID":cluster,"authProvider":"better-auth"}}).to_string();
    let mut mac =
        Hmac::<Sha256>::new_from_slice(std::env::var("DATABASE_RLS_SECRET").unwrap().as_bytes())
            .unwrap();
    mac.update(raw.as_bytes());
    sql(&tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await;
    tx
}
async fn setup() -> (Fixture, Arc<HostService>, String) {
    // The shared fixture supports the retained overlap allocator; a fresh disposable schema has
    // no authentication allocator row until its first explicit fixture setup.
    let raw = std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap();
    let target = url::Url::parse(&raw).unwrap();
    assert!(
        matches!(target.host_str(), Some("localhost" | "127.0.0.1"))
            && target.path() == "/webdock_rls_test"
    );
    let owner = sea_orm::Database::connect(raw).await.unwrap();
    sql(&owner,"INSERT INTO webdock_auth.snowflake_state(singleton) SELECT true WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.snowflake_state)",vec![]).await;
    owner.close().await.unwrap();
    let f = Fixture::new().await;
    let cluster: String = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_admin.next_snowflake() AS id",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "id")
        .unwrap();
    sql(&f.owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified) VALUES($1,$2,'k3s','eu-central','fixture',true)",vec![cluster.clone().into(),f.nonce.clone().into()]).await;
    let mut manifest = shared::product_manifest(&f.plugin);
    manifest["capabilities"] = json!(["plugin.workloads"]);
    manifest["workloads"] =
        json!({"kinds":["apps/v1/Deployment","v1/Service","traefik.io/v1alpha1/IngressRoute"]});
    let raw = manifest.to_string();
    sql(&f.owner,"UPDATE webdock_auth.plugin_release SET manifest_bytes=$2,manifest_sha256=$3 WHERE plugin_id=$1",vec![f.plugin.clone().into(),raw.clone().into(),hex::encode(Sha256::digest(raw.as_bytes())).into()]).await;
    sql(&f.owner,"UPDATE webdock_auth.plugin_configuration SET approved_capabilities='[\"plugin.workloads\"]' WHERE plugin_id=$1",vec![f.plugin.clone().into()]).await;
    binding(&f, Some(&cluster));
    let host = f.host().await;
    (f, host, cluster)
}
fn binding(f: &Fixture, cluster: Option<&str>) {
    let path = f.bindings.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"][&f.plugin]["workloadCluster"] = json!(cluster);
    std::fs::write(path, bindings.to_string()).unwrap();
}
async fn apply(
    host: &Arc<HostService>,
    f: &Fixture,
    owner: Value,
    unit: &str,
    objects: Value,
) -> Result<Value, webdock_plugin_protocol::PluginError> {
    let mut input = json!({"unit":unit,"objects":objects});
    input
        .as_object_mut()
        .unwrap()
        .extend(owner.as_object().unwrap().clone());
    host.workloads(&peer(), &f.plugin, 1, "apply", input).await
}
async fn cleanup(f: Fixture, cluster: String) {
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.workload_log WHERE plugin_id=$1",
        vec![f.plugin.clone().into()],
    )
    .await;
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
        vec![cluster.into()],
    )
    .await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL restricted roles and signed RLS fixture"]
async fn customer_scope_retains_ownership_cluster_and_storage_without_a_project() {
    let (f, host, cluster) = setup().await;
    let customer = json!({"customer":f.customers[0]});
    let project = json!({"project":f.projects[0]});
    let first = apply(&host, &f, customer.clone(), "service-1", objects(128))
        .await
        .unwrap();
    let project_unit = apply(&host, &f, project.clone(), "project-1", objects(64))
        .await
        .unwrap();
    assert_ne!(first["namespace"], project_unit["namespace"]);
    let moved = f
        .owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_admin.projects SET customer_id=$2 WHERE id=$1",
            [f.projects[0].clone().into(), f.customers[1].clone().into()],
        ))
        .await
        .unwrap_err();
    assert!(
        moved
            .to_string()
            .contains("retained workloads cannot change customer")
    );
    let stored = value(&f.owner,"SELECT jsonb_build_object('project',project_id,'customer',customer_id,'objects',objects,'cluster',cluster_id) AS value FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit='service-1'",vec![f.plugin.clone().into()]).await;
    assert!(stored["project"].is_null());
    assert_eq!(stored["customer"], f.customers[0]);
    assert_eq!(
        stored["objects"][0]["metadata"]["labels"]["webdock.dev/customer-id"],
        f.customers[0]
    );
    assert!(
        stored["objects"][0]["metadata"]["labels"]
            .get("webdock.dev/project-id")
            .is_none()
    );
    for owner in [
        project.clone(),
        json!({"project":f.projects[1]}),
        json!({"customer":f.customers[1]}),
    ] {
        assert!(
            apply(&host, &f, owner, "service-1", objects(128))
                .await
                .is_err()
        );
    }
    assert!(
        apply(&host, &f, customer.clone(), "project-1", objects(64))
            .await
            .is_err()
    );
    for owner in [
        json!({}),
        json!({"project":f.projects[0],"customer":f.customers[0]}),
        json!({"customer":"9999999999999999999"}),
        json!({"customer":f.customers[0],"resource":"unowned"}),
        json!({"customer":f.customers[0],"extra":true}),
    ] {
        assert!(
            apply(&host, &f, owner, "invalid", objects(64))
                .await
                .is_err()
        );
    }
    binding(&f, None);
    assert_eq!(
        apply(&host, &f, customer.clone(), "service-1", objects(128))
            .await
            .unwrap()["revision"],
        2
    );
    assert!(
        apply(&host, &f, customer.clone(), "needs-cluster", objects(64))
            .await
            .is_err()
    );
    assert_eq!(value(&f.owner,"SELECT to_jsonb(cluster_id) AS value FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit='service-1'",vec![f.plugin.clone().into()]).await,json!(cluster));
    binding(&f, Some(&cluster));
    sql(
        &f.owner,
        "UPDATE webdock_auth.hosting_cluster SET dedicated_customer_id=$2 WHERE id=$1",
        vec![cluster.clone().into(), f.customers[1].clone().into()],
    )
    .await;
    assert!(
        apply(&host, &f, customer.clone(), "foreign-cluster", objects(64))
            .await
            .is_err()
    );
    sql(&f.owner,"UPDATE webdock_auth.hosting_cluster SET dedicated_customer_id=NULL,verified=false WHERE id=$1",vec![cluster.clone().into()]).await;
    assert!(
        apply(
            &host,
            &f,
            customer.clone(),
            "unverified-cluster",
            objects(64)
        )
        .await
        .is_err()
    );
    sql(
        &f.owner,
        "UPDATE webdock_auth.hosting_cluster SET verified=true WHERE id=$1",
        vec![cluster.clone().into()],
    )
    .await;
    sql(
        &f.owner,
        "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
        vec![f.customers[0].clone().into()],
    )
    .await;
    assert!(
        apply(&host, &f, customer.clone(), "archived", objects(64))
            .await
            .is_err()
    );
    sql(
        &f.owner,
        "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
        vec![f.customers[0].clone().into()],
    )
    .await;
    sql(
        &f.owner,
        "UPDATE webdock_admin.projects SET status='archived' WHERE id=$1",
        vec![f.projects[0].clone().into()],
    )
    .await;
    assert!(
        apply(&host, &f, project.clone(), "archived-project", objects(64))
            .await
            .is_err()
    );
    sql(
        &f.owner,
        "UPDATE webdock_admin.projects SET status='active' WHERE id=$1",
        vec![f.projects[0].clone().into()],
    )
    .await;
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND project_id IS NOT NULL",
        vec![f.plugin.clone().into()],
    )
    .await;
    // Customer infrastructure does not pin an unrelated, empty project's ownership.
    sql(
        &f.owner,
        "UPDATE webdock_admin.projects SET customer_id=$2 WHERE id=$1",
        vec![f.projects[0].clone().into(), f.customers[1].clone().into()],
    )
    .await;
    sql(
        &f.owner,
        "UPDATE webdock_admin.projects SET customer_id=$2 WHERE id=$1",
        vec![f.projects[0].clone().into(), f.customers[0].clone().into()],
    )
    .await;
    // Delete every project, including Default: the unit has no first/default/last-project anchor.
    sql(
        &f.owner,
        "DELETE FROM webdock_admin.projects WHERE customer_id=$1",
        vec![f.customers[0].clone().into()],
    )
    .await;
    assert_eq!(
        apply(&host, &f, customer, "service-1", objects(128))
            .await
            .unwrap()["revision"],
        3
    );
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE customer_id=$1",
        vec![f.customers[0].clone().into()],
    )
    .await;
    let deleted = f
        .owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_admin.customers WHERE id=$1",
            [f.customers[0].clone().into()],
        ))
        .await
        .unwrap_err();
    assert!(
        deleted
            .to_string()
            .contains("plugin_workload_customer_id_fkey"),
        "{deleted}"
    );
    cleanup(f, cluster).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL restricted roles and signed RLS fixture"]
async fn customer_and_project_units_share_limits_and_verified_domain_ownership() {
    let (f, host, cluster) = setup().await;
    let customer = json!({"customer":f.customers[0]});
    sql(&f.owner,"INSERT INTO webdock_auth.customer_plan(customer_id,overrides,updated_by) VALUES($1,'{\"memory\":192}','fixture')",vec![f.customers[0].clone().into()]).await;
    apply(&host, &f, customer.clone(), "service-1", objects(128))
        .await
        .unwrap();
    apply(
        &host,
        &f,
        json!({"project":f.projects[0]}),
        "project-1",
        objects(64),
    )
    .await
    .unwrap();
    assert!(
        apply(&host, &f, customer.clone(), "service-2", objects(1))
            .await
            .is_err()
    );
    // Replacing one's own allocation must not count it twice.
    apply(&host, &f, customer.clone(), "service-1", objects(128))
        .await
        .unwrap();
    let usage = host
        .usage(f.principal("owner"), &f.customers[0])
        .await
        .unwrap();
    assert_eq!(usage["allocated"]["memory"], 192);
    assert_eq!(usage["units"].as_array().unwrap().len(), 2);
    assert!(
        usage["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["unit"] == "service-1")
            .unwrap()["project"]
            .is_null()
    );
    assert!(
        host.usage(f.principal("foreign"), &f.customers[0])
            .await
            .is_err()
    );
    let domain = format!("{}.example.test", f.nonce);
    sql(&f.owner,"INSERT INTO webdock_auth.customer_domain(id,customer_id,domain,token,verified_at,created_by) VALUES(webdock_admin.next_snowflake(),$1,$2,repeat('a',32),now(),'fixture')",vec![f.customers[0].clone().into(),domain.clone().into()]).await;
    let route = |domain: &str| json!([{"apiVersion":"traefik.io/v1alpha1","kind":"IngressRoute","metadata":{"name":"route"},"spec":{"routes":[{"match":format!("Host(`{domain}`)"),"services":[{"name":"service","port":80}]}]}}]);
    apply(&host, &f, customer.clone(), "route-1", route(&domain))
        .await
        .unwrap();
    assert!(
        apply(
            &host,
            &f,
            json!({"customer":f.customers[1]}),
            "foreign-domain",
            route(&domain)
        )
        .await
        .is_err()
    );
    assert!(
        apply(
            &host,
            &f,
            customer,
            "unknown-domain",
            route("unverified.example.test")
        )
        .await
        .is_err()
    );
    let session = f.session("owner").await;
    let users = value(
        &session,
        "SELECT webdock_rls.domain_users($1,$2) AS value",
        vec![f.customers[0].clone().into(), domain.into()],
    )
    .await;
    assert_eq!(users.as_array().unwrap().len(), 1);
    assert!(users[0]["project"].is_null());
    session.commit().await.unwrap();
    cleanup(f, cluster).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL restricted roles and signed RLS fixture"]
async fn customer_and_project_allocations_serialize_the_same_budget() {
    let (f, host, cluster) = setup().await;
    sql(&f.owner,"INSERT INTO webdock_auth.customer_plan(customer_id,overrides,updated_by) VALUES($1,'{\"memory\":128}','fixture')",vec![f.customers[0].clone().into()]).await;
    let (customer, project) = tokio::join!(
        apply(
            &host,
            &f,
            json!({"customer":f.customers[0]}),
            "customer-race",
            objects(128)
        ),
        apply(
            &host,
            &f,
            json!({"project":f.projects[0]}),
            "project-race",
            objects(128)
        ),
    );
    assert_ne!(customer.is_ok(), project.is_ok());
    assert_eq!(
        host.usage(f.principal("owner"), &f.customers[0])
            .await
            .unwrap()["allocated"]["memory"],
        128
    );
    cleanup(f, cluster).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL restricted roles and signed RLS fixture"]
async fn customer_reports_preserve_usage_notices_logs_and_current_revision_fences() {
    let (f, host, cluster) = setup().await;
    apply(
        &host,
        &f,
        json!({"customer":f.customers[0]}),
        "service-1",
        objects(128),
    )
    .await
    .unwrap();
    let worker_url = std::env::var("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").unwrap();
    let worker = sea_orm::Database::connect(worker_url).await.unwrap();
    let report = json!({"usage":{"cpu":12,"memory":41},"problems":[{"reason":"CrashLoopBackOff"}],"logs":[format!("{} first line",chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos,true))]});
    let tx = agent(&worker, &cluster).await;
    assert_eq!(value(&tx,"SELECT to_jsonb(webdock_rls.report_plugin_workload($1,'service-1',1,$2,NULL,false)) AS value",vec![f.plugin.clone().into(),report.clone().into()]).await,json!(true));
    tx.commit().await.unwrap();
    let stored=value(&f.owner,"SELECT jsonb_build_object('customer',customer_id,'project',project_id,'samples',samples,'cpu',cpu_sum) AS value FROM webdock_auth.workload_usage WHERE plugin_id=$1 AND unit='service-1'",vec![f.plugin.clone().into()]).await;
    assert_eq!(stored["customer"], f.customers[0]);
    assert!(stored["project"].is_null());
    assert_eq!(stored["cpu"], 12);
    assert_eq!(value(&f.owner,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.workload_log WHERE plugin_id=$1 AND project_id IS NULL",vec![f.plugin.clone().into()]).await,1);
    sql(&f.owner,"UPDATE webdock_auth.notification SET opened_at=now()-interval '3 minutes' WHERE plugin_id=$1",vec![f.plugin.clone().into()]).await;
    let notices = host
        .notifications(f.principal("owner"), &f.customers[0])
        .await
        .unwrap();
    assert_eq!(notices.as_array().unwrap().len(), 1);
    assert!(notices[0]["project"].is_null());
    assert!(notices[0]["resource"].is_null());
    apply(
        &host,
        &f,
        json!({"customer":f.customers[0]}),
        "service-1",
        objects(128),
    )
    .await
    .unwrap();
    let tx = agent(&worker, &cluster).await;
    assert_eq!(value(&tx,"SELECT to_jsonb(webdock_rls.report_plugin_workload($1,'service-1',1,'{\"usage\":{\"cpu\":999,\"memory\":999},\"logs\":[\"2026-10-23T20:01:00Z stale line\"]}',NULL,false)) AS value",vec![f.plugin.clone().into()]).await,json!(false));
    tx.commit().await.unwrap();
    assert_eq!(
        value(
            &f.owner,
            "SELECT to_jsonb(samples) AS value FROM webdock_auth.workload_usage WHERE plugin_id=$1",
            vec![f.plugin.clone().into()]
        )
        .await,
        1
    );
    let tx = agent(&worker, &cluster).await;
    assert_eq!(value(&tx,"SELECT to_jsonb(webdock_rls.report_plugin_workload($1,'service-1',2,'{}',NULL,false)) AS value",vec![f.plugin.clone().into()]).await,json!(true));
    tx.commit().await.unwrap();
    sql(
        &f.owner,
        "UPDATE webdock_auth.workload_log SET at=now()-interval '4 days' WHERE plugin_id=$1;",
        vec![f.plugin.clone().into()],
    )
    .await;
    host.prune_history().await.unwrap();
    assert_eq!(
        value(
            &f.owner,
            "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.workload_log WHERE plugin_id=$1",
            vec![f.plugin.clone().into()]
        )
        .await,
        0
    );
    worker.close().await.unwrap();
    cleanup(f, cluster).await;
}
