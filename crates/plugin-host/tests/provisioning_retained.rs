//! Retained storage is inspected, never repaired or provisioned by lifecycle operations.
use webdock_plugin_host::provisioning::{
    ProvisioningError,
    storage::{MetadataProvisioner, StoragePlan},
};

#[path = "support/provisioning_fixture.rs"]
#[allow(
    dead_code,
    reason = "The shared fixture also carries files and keys for runtime TLS tests."
)]
mod fixture;

fn plan(f: &fixture::Fixture, password: &str) -> StoragePlan {
    StoragePlan::new(
        "retained-platform",
        "postgres",
        password.into(),
        "localhost".into(),
        f.port,
    )
    .unwrap()
}

async fn reader(f: &fixture::Fixture) -> MetadataProvisioner {
    let mut url = url::Url::parse(&f.admin_url).unwrap();
    // tokio-postgres percent-decodes options but does not form-decode '+' as a space.
    url.set_query(Some("options=-c%20default_transaction_read_only%3Don"));
    assert_eq!(
        url.as_str()
            .parse::<tokio_postgres::Config>()
            .unwrap()
            .get_options(),
        Some("-c default_transaction_read_only=on")
    );
    MetadataProvisioner::connect(url.as_str(), Some(f.ca.as_bytes()))
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires WEBDOCK_PLUGIN_PROVISIONING_DOCKER_TEST=1; owned disposable PostgreSQL only"]
async fn retained_returns_existing_login_under_enforced_read_only_session() {
    let f = fixture::Fixture::start().await;
    let provisioner = MetadataProvisioner::connect(&f.admin_url, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    let plan = plan(&f, &"a".repeat(64));
    let initial = provisioner.provision(&plan).await.unwrap();
    let app = webdock_plugin_kit::postgres::connect(&initial.url, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    app.batch_execute("CREATE TABLE retained(value integer); INSERT INTO retained VALUES(73)")
        .await
        .unwrap();

    let read_only = reader(&f).await;
    assert!(
        read_only.provision(&plan).await.is_err(),
        "fixture forbids provisioning writes"
    );
    let retained = read_only.retained(&plan).await.unwrap();
    assert!(
        initial.url == retained.url,
        "private login material stays byte-identical"
    );
    assert_eq!(retained.database(), plan.database());
    assert_eq!(retained.role(), plan.role());
    let reconnected = webdock_plugin_kit::postgres::connect(&retained.url, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    assert_eq!(
        reconnected
            .query_one("SELECT value FROM retained", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        73
    );
}

#[tokio::test]
#[ignore = "requires WEBDOCK_PLUGIN_PROVISIONING_DOCKER_TEST=1; owned disposable PostgreSQL only"]
async fn retained_refuses_missing_storage_without_creating_it() {
    let f = fixture::Fixture::start().await;
    let read_only = reader(&f).await;
    let plan = plan(&f, &"b".repeat(64));
    assert_eq!(
        read_only.retained(&plan).await.err(),
        Some(ProvisioningError::Ownership)
    );
    let admin = f.admin().await;
    let row = admin.query_one("SELECT EXISTS(SELECT FROM pg_catalog.pg_roles WHERE rolname=$1),EXISTS(SELECT FROM pg_catalog.pg_database WHERE datname=$2)", &[&plan.role(), &plan.database()]).await.unwrap();
    assert!(!row.get::<_, bool>(0) && !row.get::<_, bool>(1));
    let provisioner = MetadataProvisioner::connect(&f.admin_url, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    provisioner.provision(&plan).await.unwrap();
    admin
        .batch_execute(&format!("DROP DATABASE {}", plan.database()))
        .await
        .unwrap();
    assert_eq!(
        read_only.retained(&plan).await.err(),
        Some(ProvisioningError::Ownership)
    );
    assert!(
        !admin
            .query_one(
                "SELECT EXISTS(SELECT FROM pg_catalog.pg_database WHERE datname=$1)",
                &[&plan.database()]
            )
            .await
            .unwrap()
            .get::<_, bool>(0)
    );
}

#[tokio::test]
#[ignore = "requires WEBDOCK_PLUGIN_PROVISIONING_DOCKER_TEST=1; owned disposable PostgreSQL only"]
async fn retained_rejects_ownership_and_role_drift_without_repair() {
    let f = fixture::Fixture::start().await;
    let provisioner = MetadataProvisioner::connect(&f.admin_url, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    let plan = plan(&f, &"c".repeat(64));
    provisioner.provision(&plan).await.unwrap();
    let read_only = reader(&f).await;
    let admin = f.admin().await;
    let markers = admin.query_one("SELECT pg_catalog.shobj_description(r.oid,'pg_authid'),pg_catalog.shobj_description(d.oid,'pg_database') FROM pg_catalog.pg_roles r,pg_catalog.pg_database d WHERE r.rolname=$1 AND d.datname=$2", &[&plan.role(), &plan.database()]).await.unwrap();
    let role_marker: String = markers.get(0);
    let database_marker: String = markers.get(1);

    let changed = StoragePlan::new(
        "retained-platform",
        "postgres",
        "d".repeat(64),
        "localhost".into(),
        f.port,
    )
    .unwrap();
    assert_eq!(
        read_only.retained(&changed).await.err(),
        Some(ProvisioningError::Ownership)
    );
    for (kind, name, marker) in [
        ("ROLE", plan.role(), role_marker),
        ("DATABASE", plan.database(), database_marker),
    ] {
        for value in ["NULL", "'foreign'"] {
            admin
                .batch_execute(&format!("COMMENT ON {kind} {name} IS {value}"))
                .await
                .unwrap();
            assert_eq!(
                read_only.retained(&plan).await.err(),
                Some(ProvisioningError::Ownership)
            );
        }
        admin
            .batch_execute(&format!("COMMENT ON {kind} {name} IS '{marker}'"))
            .await
            .unwrap();
    }
    for flag in [
        "SUPERUSER",
        "CREATEDB",
        "CREATEROLE",
        "INHERIT",
        "REPLICATION",
        "BYPASSRLS",
        "NOLOGIN",
    ] {
        admin
            .batch_execute(&format!("ALTER ROLE {} {flag}", plan.role()))
            .await
            .unwrap();
        assert_eq!(
            read_only.retained(&plan).await.err(),
            Some(ProvisioningError::Ownership),
            "reject {flag}"
        );
        admin.batch_execute(&format!("ALTER ROLE {} NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS LOGIN", plan.role())).await.unwrap();
    }
    admin
        .batch_execute(&format!("GRANT pg_read_all_data TO {}", plan.role()))
        .await
        .unwrap();
    assert_eq!(
        read_only.retained(&plan).await.err(),
        Some(ProvisioningError::Ownership)
    );
    admin
        .batch_execute(&format!(
            "REVOKE pg_read_all_data FROM {}; ALTER DATABASE {} OWNER TO postgres",
            plan.role(),
            plan.database()
        ))
        .await
        .unwrap();
    assert_eq!(
        read_only.retained(&plan).await.err(),
        Some(ProvisioningError::Ownership)
    );
    admin
        .batch_execute(&format!(
            "ALTER DATABASE {} OWNER TO {}",
            plan.database(),
            plan.role()
        ))
        .await
        .unwrap();
    assert!(read_only.retained(&plan).await.is_ok());
}
