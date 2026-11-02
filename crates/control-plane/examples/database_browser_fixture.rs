//! Explicit owned local browser fixture; no production configuration is accepted.
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use serde_json::{Value, json};
fn guard(key: &str) -> String {
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
    assert!(
        !std::path::Path::new(".runtime-test/database-browser.json").exists(),
        "Reuse or explicitly clean the existing owned fixture first"
    );
    let account: Value =
        serde_json::from_slice(&std::fs::read(".runtime-test/account-browser.json").unwrap())
            .unwrap();
    let user = account["userID"].as_str().unwrap();
    let owner = Database::connect(guard("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    let row=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND email=$2 AND name='Account browser fixture' AND role='operator' AND \"twoFactorEnabled\" AND \"emailVerified\" AND NOT \"mustChangePassword\"",[user.into(),account["email"].as_str().unwrap().into()])).await.unwrap();
    assert!(
        row.is_some(),
        "Complete the owned account browser fixture first"
    );
    let ids = webdock_persistence::IdAllocator::connect(
        &guard("AUTH_DATABASE_URL"),
        webdock_ids::Identity::platform(0, 2).unwrap(),
    )
    .await
    .unwrap();
    let customer = ids.allocate().await.unwrap().to_string();
    let project = ids.allocate().await.unwrap().to_string();
    let binding = ids.allocate().await.unwrap().to_string();
    let grant = ids.allocate().await.unwrap().to_string();
    let runtime = "https://localhost:3152";
    let proxy_secret = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let gateway_secret = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let cipher =
        webdock_secrets::SecretCipher::new(&std::env::var("BETTER_AUTH_SECRET").unwrap()).unwrap();
    let sealed = cipher
        .seal(&proxy_secret, &format!("database:{binding}:{user}"))
        .unwrap();
    let module_version = format!("database-browser-{binding}");
    let module_added=owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('databases','builtin','enabled','enabled',$1) ON CONFLICT DO NOTHING",[module_version.clone().into()])).await.unwrap().rows_affected()==1;
    for (sql, args) in [
        (
            "INSERT INTO webdock_admin.customers(id,name) VALUES($1,'Database browser fixture')",
            vec![customer.clone().into()],
        ),
        (
            "INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES($1,$2,'Database browser fixture')",
            vec![project.clone().into(), customer.clone().into()],
        ),
        (
            "INSERT INTO webdock_auth.database_binding(id,project_id,name,environment,engine) VALUES($1,$2,'Orders','development','sqlite')",
            vec![binding.clone().into(), project.clone().into()],
        ),
        (
            "INSERT INTO webdock_auth.database_grant(id,binding_id,subject,profile,runtime_origin,connection_id,secret_sealed) VALUES($1,$2,$3,'schema',$4,'fixture-orders',$5)",
            vec![
                grant.clone().into(),
                binding.clone().into(),
                user.into(),
                runtime.into(),
                sealed.into(),
            ],
        ),
        (
            "INSERT INTO webdock_auth.database_runtime_owner(runtime_origin,binding_id,subject,profile) VALUES($1,$2,$3,'schema')",
            vec![runtime.into(), binding.clone().into(), user.into()],
        ),
    ] {
        owner
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                args,
            ))
            .await
            .unwrap();
    }
    let fixture = json!({"userID":user,"customerID":customer,"projectID":project,"bindingID":binding,"grantID":grant,"runtimeOrigin":runtime,"gatewayOrigin":"http://127.0.0.1:3150","gatewaySecret":gateway_secret,"proxySecret":proxy_secret,"moduleVersion":module_version,"moduleAdded":module_added});
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(".runtime-test/database-browser.json")
        .unwrap();
    file.write_all(fixture.to_string().as_bytes()).unwrap();
    println!("Owned database browser fixture prepared; credentials remain private.");
}
