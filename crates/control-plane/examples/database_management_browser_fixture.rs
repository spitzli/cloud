//! Owned local browser fixture. Creates a synthetic admission observation, never a live provider.
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use serde_json::{Value, json};
fn guarded(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let u = url::Url::parse(&raw).unwrap();
    assert!(["postgres", "postgresql"].contains(&u.scheme()));
    assert_eq!(u.host_str(), Some("127.0.0.1"));
    assert_eq!(u.path(), "/webdock_rls_test");
    assert!(u.query().is_none() && u.fragment().is_none());
    raw
}
#[tokio::main]
async fn main() {
    let path = ".runtime-test/database-management-browser.json";
    assert!(
        !std::path::Path::new(path).exists(),
        "Reuse the exact owned fixture"
    );
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(".runtime-test/database-browser.json").unwrap())
            .unwrap();
    let owner = Database::connect(guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    let valid=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT p.id FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1 AND c.id=$2 AND p.name='Database browser fixture' AND c.name='Database browser fixture'",[fixture["projectID"].as_str().unwrap().into(),fixture["customerID"].as_str().unwrap().into()])).await.unwrap();
    assert!(valid.is_some());
    let ids = webdock_persistence::IdAllocator::connect(
        &guarded("AUTH_DATABASE_URL"),
        webdock_ids::Identity::platform(0, 2).unwrap(),
    )
    .await
    .unwrap();
    let cluster = ids.allocate().await.unwrap().to_string();
    let pool = uuid::Uuid::new_v4().to_string();
    let capacity = json!({"apps":100,"cpuMillicores":16000,"memoryBytes":68719476736_u64,"volumeBytes":214748364800_u64,"ephemeralBytes":17179869184_u64,"replicasPerApp":10,"concurrentDeployments":10});
    let observation = json!({"capacity":{"volumeBytes":107374182400_u64},"nodes":[{"id":"browser-fixture-node","cpuMillicores":16000,"memoryBytes":68719476736_u64}],"capabilities":{"databaseVersion":1,"databaseNodePorts":[],"databasePools":[{"node":"browser-fixture-node","poolID":pool,"ready":true,"capacityBytes":107374182400_u64,"availableBytes":107374182400_u64,"allocatedBytes":0,"requestedCpuMillicores":0,"requestedMemoryBytes":0,"allocations":[],"observedDatabases":[]}]}});
    use sha2::{Digest, Sha256};
    let credential = hex::encode(Sha256::digest(uuid::Uuid::new_v4().as_bytes()));
    let hash = hex::encode(Sha256::digest(credential.as_bytes()));
    for (sql, values) in [
        (
            "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity) VALUES($1,'Database management browser fixture','k3s','local','owned-synthetic-admission',true,$2)",
            vec![cluster.clone().into(), capacity.into()],
        ),
        (
            "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES($1,$2,1,now(),$3)",
            vec![cluster.clone().into(), hash.into(), observation.into()],
        ),
        (
            "INSERT INTO webdock_auth.hosting_database_pool(cluster_id,node,pool_id,capacity_bytes) VALUES($1,'browser-fixture-node',$2::uuid,107374182400)",
            vec![cluster.clone().into(), pool.clone().into()],
        ),
    ] {
        owner
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap();
    }
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(json!({"clusterID":cluster,"poolID":pool,"projectID":fixture["projectID"],"customerID":fixture["customerID"],"userID":fixture["userID"]}).to_string().as_bytes()).unwrap();
    println!("Owned managed admission fixture created; no provider or execution agent enrolled.");
}
