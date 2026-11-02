use std::collections::BTreeMap;
use webdock_persistence::migrations::verify_legacy_history;

#[test]
fn retained_history_is_adopted_only_with_exact_names_and_checksums() {
    let expected = BTreeMap::from([
        ("0001_initial.sql".into(), "abc".into()),
        ("0002_upgrade.sql".into(), "def".into()),
    ]);
    assert!(
        verify_legacy_history(
            &expected,
            &[
                ("0001_initial.sql".into(), "abc".into()),
                ("0002_upgrade.sql".into(), "def".into())
            ]
        )
        .is_ok()
    );
    assert!(
        verify_legacy_history(&expected, &[("0001_initial.sql".into(), "changed".into())]).is_err()
    );
    assert!(
        verify_legacy_history(&expected, &[("9999_missing.sql".into(), "abc".into())]).is_err()
    );
    assert!(
        verify_legacy_history(&expected, &[("0001_initial.sql".into(), "abc".into())]).is_err()
    );
}

#[tokio::test]
#[ignore = "requires the owned retained-schema local fixture"]
async fn offline_upgrade_uses_retained_journal_and_denies_runtime_roles() {
    use webdock_persistence::migrations::{MigrationError, upgrade};
    let owner = std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap();
    let url = url::Url::parse(&owner).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let db = sea_orm::Database::connect(&owner).await.unwrap();
    assert!(matches!(
        upgrade(&db, "wrong_target").await,
        Err(MigrationError::Target)
    ));
    upgrade(&db, "webdock_rls_test").await.unwrap();
    upgrade(&db, "webdock_rls_test").await.unwrap();
    let runtime = std::env::var("AUTH_DATABASE_URL").unwrap();
    let parsed = url::Url::parse(&runtime).unwrap();
    assert_eq!(parsed.host_str(), url.host_str());
    assert_eq!(parsed.port(), url.port());
    assert_eq!(parsed.path(), url.path());
    let runtime = sea_orm::Database::connect(&runtime).await.unwrap();
    assert!(matches!(
        upgrade(&runtime, "webdock_rls_test").await,
        Err(MigrationError::RuntimeRole)
    ));
}

#[tokio::test]
#[ignore = "creates and removes a uniquely owned local fixture database"]
async fn fresh_native_bootstrap_has_complete_schema_and_rejects_reinitialization() {
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
    use webdock_persistence::migrations::{bootstrap, upgrade, verify_bootstrap_baseline};
    let raw = std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap();
    let mut url = url::Url::parse(&raw).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let control = Database::connect(&raw).await.unwrap();
    let name = format!(
        "webdock_bootstrap_{}_{}_test",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    control
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let db = Database::connect(url.as_str()).await.unwrap();
    let result=async{
        bootstrap(&db,&name,"native-bootstrap-context-key-at-least-32bytes").await?;
        upgrade(&db,&name).await?;
        verify_bootstrap_baseline(&db).await?;
        assert!(bootstrap(&db,&name,"another-context-key-at-least-32bytes").await.is_err());
        let row=db.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT to_regclass('webdock_auth.user') IS NOT NULL AND to_regclass('webdock_admin.projects') IS NOT NULL AND to_regclass('webdock_auth.identity_passkey_ceremony') IS NOT NULL AS ready".to_owned())).await.unwrap().unwrap();
        assert!(row.try_get::<bool>("","ready").unwrap());
        let mut runtime_url=url::Url::parse(&std::env::var("AUTH_DATABASE_URL").unwrap()).unwrap();
        assert_eq!(runtime_url.host_str(),url.host_str());assert_eq!(runtime_url.port(),url.port());
        runtime_url.set_path(&format!("/{name}"));
        let runtime=Database::connect(runtime_url.as_str()).await.unwrap();
        assert!(webdock_persistence::migrations::native_schema_ready(&runtime).await);
        use sha2::{Digest,Sha256};
        let native_checksum=hex::encode(Sha256::digest(include_bytes!("../schema/baseline.sql")));
        let row=db.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT checksum FROM webdock_api_migrations.baseline WHERE id")).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("","checksum").unwrap(),native_checksum);
        db.execute_unprepared("UPDATE webdock_api_migrations.baseline SET checksum=repeat('0',64)").await.unwrap();
        assert!(verify_bootstrap_baseline(&db).await.is_err());
        upgrade(&db,&name).await?;
        assert!(verify_bootstrap_baseline(&db).await.is_err(),"upgrade must not repair provenance");
        db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_api_migrations.baseline SET checksum=$1",[include_str!("../../../tools/migrations/baseline.sha256").trim().into()])).await.unwrap();
        verify_bootstrap_baseline(&db).await?;
        db.execute_unprepared("ALTER TABLE webdock_api_migrations.journal OWNER TO postgres").await.unwrap();
        assert!(verify_bootstrap_baseline(&db).await.is_err());
        db.execute_unprepared("ALTER TABLE webdock_api_migrations.journal OWNER TO webdock_platform_owner").await.unwrap();
        let tx=db.begin().await.unwrap();
        tx.execute_unprepared("DELETE FROM webdock_api_migrations.journal WHERE id=(SELECT min(id) FROM webdock_api_migrations.journal)").await.unwrap();
        assert!(verify_bootstrap_baseline(&tx).await.is_err(),"retained checksum alone is not sufficient");
        tx.rollback().await.unwrap();
        db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_api_migrations.baseline SET checksum=$1",[native_checksum.into()])).await.unwrap();
        let tx=db.begin().await.unwrap();
        tx.execute_unprepared("DELETE FROM webdock_api_migrations.baseline").await.unwrap();
        assert!(verify_bootstrap_baseline(&tx).await.is_err());
        tx.rollback().await.unwrap();
        db.execute_unprepared("UPDATE webdock_api_migrations.rust_journal SET checksum='\\x00'::bytea WHERE version=1").await.unwrap();
        assert!(!webdock_persistence::migrations::native_schema_ready(&runtime).await);
        assert!(verify_bootstrap_baseline(&db).await.is_err(),"native marker requires exact native journal");
        runtime.close().await.unwrap();
        Ok::<_,webdock_persistence::migrations::MigrationError>(())
    }.await;
    db.close().await.unwrap();
    control
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}
