use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use std::{collections::BTreeMap, process::Command, time::Duration};

fn docker() -> Command {
    let mut command = Command::new("docker");
    command.args(["--host", "unix:///var/run/docker.sock"]);
    command
}
struct Container {
    name: String,
}
impl Drop for Container {
    fn drop(&mut self) {
        let _ = docker().args(["rm", "-f", &self.name]).output();
    }
}
pub struct Fixture {
    _container: Container,
    pub owner: DatabaseConnection,
    pub runtime_url: String,
    #[allow(
        dead_code,
        reason = "Shared owned fixture includes owner-role refusal coverage"
    )]
    pub owner_url: String,
}
impl Fixture {
    pub async fn start() -> Self {
        let name = format!("wd-native-pg-{}", uuid::Uuid::new_v4().simple());
        let password = format!("owned-{}:+/?@", uuid::Uuid::new_v4().simple());
        assert!(
            docker()
                .args([
                    "run",
                    "-d",
                    "--rm",
                    "--name",
                    &name,
                    "-e",
                    "POSTGRES_PASSWORD",
                    "-e",
                    "POSTGRES_DB=webdock_native_test",
                    "-p",
                    "127.0.0.1::5432",
                    "postgres:18-alpine"
                ])
                .env("POSTGRES_PASSWORD", &password)
                .output()
                .unwrap()
                .status
                .success()
        );
        let container = Container { name: name.clone() };
        let endpoint = docker().args(["port", &name, "5432/tcp"]).output().unwrap();
        assert!(endpoint.status.success());
        let endpoint = String::from_utf8(endpoint.stdout).unwrap();
        let address: std::net::SocketAddr = endpoint.trim().parse().unwrap();
        assert!(address.ip().is_loopback());
        let mut url = url::Url::parse(&format!(
            "postgres://postgres@{address}/webdock_native_test?sslmode=disable"
        ))
        .unwrap();
        url.set_password(Some(&password)).unwrap();
        let owner = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let mut options = sea_orm::ConnectOptions::new(url.as_str());
                options.max_connections(1).sqlx_logging(false);
                if let Ok(db) = Database::connect(options).await {
                    break db;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        let mut fixture = Self {
            _container: container,
            owner,
            runtime_url: String::new(),
            owner_url: url.to_string(),
        };
        let role_password = format!("private-{}", uuid::Uuid::new_v4().simple());
        let passwords = BTreeMap::from_iter(
            spitzli_persistence::roles::LOGIN_ROLES
                .iter()
                .map(|role| (role.to_string(), role_password.clone())),
        );
        spitzli_persistence::roles::provision_roles(
            &fixture.owner,
            "webdock_native_test",
            &passwords,
        )
        .await
        .unwrap();
        fixture
            .owner
            .execute_unprepared(include_str!("../../migrations/0001_ids.sql"))
            .await
            .unwrap();
        // Delimiters exercise native URI/SCRAM handling; only this owned test role is changed.
        assert!(!password.contains('\''));
        fixture
            .owner
            .execute_unprepared(&format!(
                "ALTER ROLE webdock_auth_runtime PASSWORD '{password}'"
            ))
            .await
            .unwrap();
        url.set_username("webdock_auth_runtime").unwrap();
        fixture.runtime_url = url.to_string();
        fixture
    }
}
