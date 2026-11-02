//! A fresh supported native bootstrap, never the live acceptance database.
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use webdock_plugin_host::provisioning::storage::MetadataProvisioner;
#[expect(
    dead_code,
    reason = "Shared TLS fixture also supports other runtime startup tests."
)]
#[path = "../../plugin-host/tests/support/provisioning_fixture.rs"]
mod fixture;
#[tokio::test]
#[ignore = "requires WEBDOCK_PLUGIN_PROVISIONING_DOCKER_TEST=1; owned fresh dual-interface PostgreSQL fixture"]
async fn one_cluster_on_different_tcp_interfaces_is_never_separate_storage() {
    let f = fixture::Fixture::start().await;
    let interfaces = interfaces(&f);
    let passwords = webdock_persistence::roles::LOGIN_ROLES
        .iter()
        .copied()
        .chain(["webdock_offline_migrate"])
        .map(|role| (role.to_owned(), "c".repeat(64)))
        .collect();
    let owner = connect(&f.admin_url, &f.ca).await;
    webdock_persistence::roles::provision_roles(&owner, "postgres", &passwords)
        .await
        .unwrap();
    webdock_persistence::migrations::bootstrap(
        &owner,
        "postgres",
        "owned-cluster-identity-context-key-32bytes",
    )
    .await
    .unwrap();
    let mut platform_url = url::Url::parse(&f.admin_url).unwrap();
    platform_url.set_username("webdock_auth_runtime").unwrap();
    platform_url.set_password(Some(&"c".repeat(64))).unwrap();
    let platform = connect(platform_url.as_str(), &f.ca).await;
    let (first, second) = (&interfaces.urls[0], &interfaces.urls[1]);
    let a = MetadataProvisioner::connect(first, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    let b = MetadataProvisioner::connect(second, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    let old_a = a.server_identity().await.unwrap();
    let old_b = b.server_identity().await.unwrap();
    assert_ne!(
        old_a, old_b,
        "actual distinct server listen interfaces reproduce the previous bypass"
    );
    let identity =
        webdock_control_plane::plugins::installation::worker::platform_storage_identity(&platform)
            .await
            .unwrap();
    assert!(a.require_separate_cluster(&identity).await.is_err());
    assert!(b.require_separate_cluster(&identity).await.is_err());
    let different = fixture::Fixture::start().await;
    let separate =
        MetadataProvisioner::connect(&different.admin_url, Some(different.ca.as_bytes()))
            .await
            .unwrap();
    assert!(
        separate.require_separate_cluster(&identity).await.is_ok(),
        "independently initialized storage remains usable"
    );
    let row=platform.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT NOT (rolsuper OR rolbypassrls OR rolcreatedb OR rolcreaterole OR rolreplication) AND NOT pg_has_role(current_user,'pg_monitor','MEMBER') AS restricted FROM pg_roles WHERE rolname=current_user")).await.unwrap().unwrap();
    assert!(
        row.try_get::<bool>("", "restricted").unwrap(),
        "observed identity requires no new privileged role"
    );
    let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM pg_roles WHERE rolname LIKE 'wd_pm_r_%') AS roles,(SELECT count(*) FROM pg_database WHERE datname LIKE 'wd_pm_d_%') AS databases")).await.unwrap().unwrap();
    assert_eq!(row.try_get::<i64>("", "roles").unwrap(), 0);
    assert_eq!(row.try_get::<i64>("", "databases").unwrap(), 0);
    // Harden only this disposable test cluster after proving the stock restricted-role path.
    owner
        .execute_unprepared("REVOKE EXECUTE ON FUNCTION pg_catalog.pg_control_system() FROM PUBLIC")
        .await
        .unwrap();
    assert!(
        webdock_control_plane::plugins::installation::worker::platform_storage_identity(&platform)
            .await
            .is_err(),
        "missing control-data access fails closed"
    );
    owner.close().await.unwrap();
    platform.close().await.unwrap();
}

async fn connect(raw: &str, ca: &str) -> sea_orm::DatabaseConnection {
    let mut options = sea_orm::ConnectOptions::new(raw);
    options
        .max_connections(1)
        .min_connections(1)
        .sqlx_logging(false);
    let ca = ca.as_bytes().to_vec();
    options.map_sqlx_postgres_opts(move |options| options.ssl_root_cert_from_pem(ca.clone()));
    sea_orm::Database::connect(options).await.unwrap()
}
struct Interfaces {
    name: String,
    container: String,
    urls: Vec<String>,
}
fn docker(args: &[&str]) -> std::process::Output {
    std::process::Command::new("docker")
        .args(["--host", "unix:///var/run/docker.sock"])
        .args(args)
        .output()
        .unwrap()
}
impl Drop for Interfaces {
    fn drop(&mut self) {
        let _ = docker(&[
            "network",
            "disconnect",
            "--force",
            &self.name,
            &self.container,
        ]);
        let _ = docker(&["network", "rm", &self.name]);
    }
}
fn interfaces(f: &fixture::Fixture) -> Interfaces {
    let name = format!("webdock-id-test-{}", uuid::Uuid::new_v4().simple());
    assert!(
        docker(&[
            "network",
            "create",
            "--label",
            "webdock.owned-cluster-identity=true",
            &name
        ])
        .status
        .success()
    );
    let mut interfaces = Interfaces {
        name,
        container: f.container.clone(),
        urls: Vec::new(),
    };
    assert!(
        docker(&[
            "network",
            "connect",
            &interfaces.name,
            &interfaces.container
        ])
        .status
        .success()
    );
    let output = docker(&[
        "inspect",
        "--format",
        "{{json .NetworkSettings.Networks}}",
        &interfaces.container,
    ]);
    assert!(output.status.success());
    let networks: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for network in networks.as_object().unwrap().values() {
        let ip = network["IPAddress"].as_str().unwrap();
        let mut url = url::Url::parse(&f.admin_url).unwrap();
        url.set_port(Some(5432)).unwrap();
        url.query_pairs_mut().append_pair("hostaddr", ip);
        interfaces.urls.push(url.into());
    }
    assert_eq!(interfaces.urls.len(), 2);
    interfaces
}
