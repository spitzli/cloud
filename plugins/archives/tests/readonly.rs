#![cfg(feature = "local-test")]
#[path = "../../../crates/agent/src/database/exports.rs"]
mod role;
use serde_json::Value;
#[tokio::test]
#[ignore = "uses only the owned PostgreSQL instance created by archives-storage.py"]
async fn dedicated_export_role_cannot_write_ddl_set_role_or_enter_another_database() {
    let f: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("WEBDOCK_ARCHIVES_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let meta = f["database"].as_str().unwrap();
    let (admin, task) = tokio_postgres::connect(meta, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = task.await;
    });
    let id = "998877661";
    let db = format!("db_{id}");
    admin
        .batch_execute(&format!("CREATE ROLE wd_migrate_{id} NOLOGIN"))
        .await
        .unwrap();
    admin
        .batch_execute(&format!("CREATE DATABASE {db}"))
        .await
        .unwrap();
    admin
        .batch_execute("REVOKE CONNECT ON DATABASE archive_source FROM PUBLIC")
        .await
        .unwrap();
    admin
        .batch_execute(&format!("REVOKE ALL ON DATABASE {db} FROM PUBLIC"))
        .await
        .unwrap();
    let role = role::ExportRole::new(id, &db, "dedicated")
        .unwrap()
        .unwrap();
    assert!(role::ExportRole::new(id, &db, "shared").unwrap().is_none());
    admin.batch_execute(&role.create).await.unwrap();
    let target = meta.replace("/archive_metadata", &format!("/{db}"));
    let (owner, task) = tokio_postgres::connect(&target, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = task.await;
    });
    owner.batch_execute(&format!("REVOKE ALL ON SCHEMA public FROM PUBLIC; CREATE SCHEMA app AUTHORIZATION wd_migrate_{id}; CREATE TABLE app.proof(id integer); INSERT INTO app.proof VALUES(42); ALTER TABLE app.proof OWNER TO wd_migrate_{id};")).await.unwrap();
    owner.batch_execute(&role.grants).await.unwrap();
    let password =
        role::ExportRole::password(id, "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8").unwrap();
    admin
        .batch_execute(&format!(
            "ALTER ROLE {} PASSWORD '{}'; {}",
            role.name, password, role.enable
        ))
        .await
        .unwrap();
    let mut url = url::Url::parse(&target).unwrap();
    url.set_username(&role.name).unwrap();
    url.set_password(Some(&password)).unwrap();
    let (reader, task) = tokio_postgres::connect(url.as_str(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = task.await;
    });
    assert_eq!(
        reader
            .query_one("SELECT id FROM app.proof", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        42
    );
    reader
        .batch_execute("SET default_transaction_read_only=off")
        .await
        .unwrap();
    for statement in [
        "INSERT INTO app.proof VALUES(99)",
        "CREATE TABLE app.unsafe(id integer)",
        "CREATE SCHEMA unsafe",
        "CREATE TEMP TABLE unsafe(id integer)",
        "SET ROLE wd_migrate_998877661",
    ] {
        assert!(
            reader.batch_execute(statement).await.is_err(),
            "readonly privilege failure"
        );
    }
    let mut foreign = url.clone();
    foreign.set_path("/archive_source");
    assert!(
        tokio_postgres::connect(foreign.as_str(), tokio_postgres::NoTls)
            .await
            .is_err()
    );
    owner.batch_execute(&format!("SET ROLE wd_migrate_{id}; CREATE TABLE app.future(id integer); INSERT INTO app.future VALUES(7); RESET ROLE;")).await.unwrap();
    assert_eq!(
        reader
            .query_one("SELECT id FROM app.future", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        7
    );
    admin.batch_execute(&role.disable).await.unwrap();
    assert!(reader.simple_query("SELECT 1").await.is_err());
    let next =
        role::ExportRole::password(id, "Hh0cGxoZGBcWFRQTEhEQDw4NDAsKCQgHBgUEAwIBAAA").unwrap();
    admin
        .batch_execute(&format!(
            "ALTER ROLE {} PASSWORD '{}'; {}",
            role.name, next, role.enable
        ))
        .await
        .unwrap();
    assert!(
        tokio_postgres::connect(url.as_str(), tokio_postgres::NoTls)
            .await
            .is_err()
    );
    url.set_password(Some(&next)).unwrap();
    let (rotated, task) = tokio_postgres::connect(url.as_str(), tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = task.await;
    });
    assert_eq!(
        rotated
            .query_one("SELECT count(*) FROM app.proof", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    admin.batch_execute(&role.disable).await.unwrap();
    drop(rotated);
    drop(owner);
    admin
        .batch_execute(&format!("DROP DATABASE {db} WITH (FORCE);"))
        .await
        .unwrap();
    admin.batch_execute(&role.drop_role).await.unwrap();
    assert_eq!(
        admin
            .query_one(
                "SELECT count(*) FROM pg_roles WHERE rolname=$1",
                &[&role.name]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}
