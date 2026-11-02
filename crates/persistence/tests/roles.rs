use std::collections::BTreeMap;
use webdock_persistence::roles::{LOGIN_ROLES, OFFLINE_ROLE, validate_passwords};
#[test]
fn role_setup_accepts_only_complete_fixed_names_and_private_password_values() {
    assert!(validate_passwords(&BTreeMap::new()).is_err());
    let mut passwords: BTreeMap<String, String> = LOGIN_ROLES
        .iter()
        .map(|name| ((*name).into(), "a".repeat(48)))
        .collect();
    assert!(validate_passwords(&passwords).is_ok());
    passwords.insert(OFFLINE_ROLE.into(), "c".repeat(48));
    assert!(validate_passwords(&passwords).is_ok());
    passwords.insert(OFFLINE_ROLE.into(), "short".into());
    assert!(validate_passwords(&passwords).is_err());
    passwords.remove(OFFLINE_ROLE);
    passwords.insert("postgres".into(), "b".repeat(48));
    assert!(validate_passwords(&passwords).is_err());
    passwords.remove("postgres");
    passwords.insert(LOGIN_ROLES[0].into(), "short".into());
    assert!(validate_passwords(&passwords).is_err());
}

#[tokio::test]
#[ignore = "requires explicit WEBDOCK_BOOTSTRAP_DOCKER_TEST=1 and local Docker Engine"]
async fn empty_postgres_bootstrap_creates_roles_without_changing_existing_credentials() {
    use sea_orm::{ConnectOptions, ConnectionTrait, Database, DbBackend, Statement};
    use std::process::Command;
    use webdock_persistence::{
        migrations::{bootstrap, native_schema_ready, verify_bootstrap_baseline},
        roles::provision_roles,
    };
    assert_eq!(
        std::env::var("WEBDOCK_BOOTSTRAP_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    fn docker() -> Command {
        let mut command = Command::new("docker");
        command.args(["--host", "unix:///var/run/docker.sock"]);
        command
    }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let secret = format!("{}{}", nonce, uuid::Uuid::new_v4().simple());
    let name = format!("webdock-bootstrap-{}", &nonce[..12]);
    let output = docker()
        .args([
            "run",
            "--detach",
            "--name",
            &name,
            "--label",
            &format!("webdock.rust-bootstrap={nonce}"),
            "--env",
            "POSTGRES_PASSWORD",
            "--env",
            "POSTGRES_DB=webdock_fresh_test",
            "--publish",
            "127.0.0.1::5432",
            "postgres:18-alpine",
        ])
        .env("POSTGRES_PASSWORD", &secret)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owned test database could not be created"
    );
    let container = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    assert!(container.len() == 64 && container.bytes().all(|v| v.is_ascii_hexdigit()));
    struct OwnedContainer(String);
    impl Drop for OwnedContainer {
        fn drop(&mut self) {
            let _ = docker()
                .args(["rm", "--force", "--volumes", &self.0])
                .output();
        }
    }
    let _cleanup = OwnedContainer(container.clone());
    let ports = docker()
        .args(["port", &container, "5432/tcp"])
        .output()
        .unwrap();
    assert!(ports.status.success());
    let address = String::from_utf8(ports.stdout).unwrap();
    let address = address.trim();
    assert!(address.starts_with("127.0.0.1:"));
    let mut owner_url = url::Url::parse(&format!(
        "postgresql://postgres@{address}/webdock_fresh_test"
    ))
    .unwrap();
    owner_url.set_password(Some(&secret)).unwrap();
    let mut connected = None;
    let mut connection_error = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let mut options = ConnectOptions::new(owner_url.as_str());
        options
            .sqlx_logging(false)
            .max_connections(1)
            .connect_timeout(std::time::Duration::from_secs(1));
        match Database::connect(options).await {
            Ok(db) => {
                connected = Some(db);
                break;
            }
            Err(error) => {
                connection_error = error.to_string().replace(&secret, "[redacted]");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if connected.is_none() {
        let logs = docker()
            .args(["logs", "--tail", "20", &container])
            .output()
            .unwrap();
        panic!(
            "owned PostgreSQL did not become ready: {}; {}",
            connection_error,
            String::from_utf8_lossy(&logs.stderr).replace(&secret, "[redacted]")
        );
    }
    let owner = connected.unwrap();
    let mut passwords: BTreeMap<String, String> = LOGIN_ROLES
        .iter()
        .map(|name| {
            (
                (*name).into(),
                format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ),
            )
        })
        .collect();
    provision_roles(&owner, "webdock_fresh_test", &passwords)
        .await
        .unwrap();
    // The existing eight-password contract remains valid; the offline login is explicit opt-in.
    let missing = owner.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT NOT EXISTS(SELECT FROM pg_catalog.pg_roles WHERE rolname='webdock_offline_migrate') AS absent"
    )).await.unwrap().unwrap();
    assert!(missing.try_get::<bool>("", "absent").unwrap());
    passwords.insert(
        OFFLINE_ROLE.into(),
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
    );
    provision_roles(&owner, "webdock_fresh_test", &passwords)
        .await
        .unwrap();
    let changed: BTreeMap<String, String> = passwords
        .keys()
        .map(|name| (name.clone(), "b".repeat(64)))
        .collect();
    provision_roles(&owner, "webdock_fresh_test", &changed)
        .await
        .unwrap();
    let mut auth_url = owner_url.clone();
    auth_url.set_username("webdock_auth_bootstrap").unwrap();
    auth_url
        .set_password(Some(&passwords["webdock_auth_bootstrap"]))
        .unwrap();
    let mut options = ConnectOptions::new(auth_url.as_str());
    options.sqlx_logging(false).max_connections(1);
    let auth = Database::connect(options)
        .await
        .expect("existing role password must be preserved");
    let mut offline_url = owner_url.clone();
    offline_url.set_username(OFFLINE_ROLE).unwrap();
    offline_url
        .set_password(Some(&passwords[OFFLINE_ROLE]))
        .unwrap();
    let mut options = ConnectOptions::new(offline_url.as_str());
    options.sqlx_logging(false).max_connections(1);
    let offline = Database::connect(options)
        .await
        .expect("existing offline password must be preserved");
    let policy=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT NOT(rolsuper OR rolbypassrls OR rolcreatedb OR rolcreaterole OR rolreplication OR rolinherit) AND rolcanlogin AS restricted FROM pg_catalog.pg_roles WHERE rolname='webdock_offline_migrate'"
    )).await.unwrap().unwrap();
    assert!(policy.try_get::<bool>("", "restricted").unwrap());
    owner
        .execute_unprepared("GRANT webdock_auth_runtime TO webdock_offline_migrate")
        .await
        .unwrap();
    assert!(
        provision_roles(&owner, "webdock_fresh_test", &passwords)
            .await
            .is_err(),
        "unexpected membership must never be silently revoked"
    );
    owner
        .execute_unprepared("REVOKE webdock_auth_runtime FROM webdock_offline_migrate")
        .await
        .unwrap();
    owner
        .execute_unprepared("ALTER ROLE webdock_offline_migrate CREATEROLE")
        .await
        .unwrap();
    assert!(
        provision_roles(&owner, "webdock_fresh_test", &passwords)
            .await
            .is_err(),
        "unexpected privilege must never be silently reset"
    );
    owner
        .execute_unprepared("ALTER ROLE webdock_offline_migrate NOCREATEROLE")
        .await
        .unwrap();
    assert!(
        provision_roles(&auth, "webdock_fresh_test", &passwords)
            .await
            .is_err()
    );
    owner
        .execute_unprepared("ALTER ROLE webdock_worker_runtime SUPERUSER")
        .await
        .unwrap();
    assert!(
        provision_roles(&owner, "webdock_fresh_test", &passwords)
            .await
            .is_err(),
        "unexpected privilege must be refused, not silently changed"
    );
    owner
        .execute_unprepared("ALTER ROLE webdock_worker_runtime NOSUPERUSER")
        .await
        .unwrap();
    bootstrap(&owner, "webdock_fresh_test", &format!("context-{secret}"))
        .await
        .unwrap();
    assert!(native_schema_ready(&auth).await);
    use sea_orm::TransactionTrait;
    let tx = offline.begin().await.unwrap();
    tx.execute_unprepared("SET LOCAL ROLE webdock_platform_owner")
        .await
        .unwrap();
    verify_bootstrap_baseline(&tx).await.unwrap();
    assert!(native_schema_ready(&tx).await);
    tx.rollback().await.unwrap();
    let row = owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT to_regclass('webdock.payload_kv') IS NULL AS absent",
        ))
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.try_get::<bool>("", "absent").unwrap(),
        "disposable fixture sentinel must never ship in fresh installs"
    );
    assert!(
        provision_roles(&owner, "webdock_fresh_test", &passwords)
            .await
            .is_err()
    );
    auth.close().await.unwrap();
    offline.close().await.unwrap();
    owner.close().await.unwrap();
}
