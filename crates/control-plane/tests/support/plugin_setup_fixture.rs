use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
const SECRET: &str = "owned-plugin-kernel-bootstrap-context-key-32";
pub struct Fixture {
    container: String,
    url: url::Url,
    passwords: BTreeMap<String, String>,
}
fn docker() -> Command {
    let mut c = Command::new("docker");
    c.args(["--host", "unix:///var/run/docker.sock"]);
    c
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = docker()
            .args(["rm", "--force", "--volumes", &self.container])
            .output();
    }
}
async fn connect(url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut options = ConnectOptions::new(url);
    options
        .max_connections(1)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(1))
        .sqlx_logging(false);
    Database::connect(options).await
}
pub fn private(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
impl Fixture {
    pub async fn start() -> Self {
        assert_eq!(
            std::env::var("WEBDOCK_PLUGIN_SETUP_DOCKER_TEST").as_deref(),
            Ok("1")
        );
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let password = format!("{nonce}{nonce}");
        let output = docker()
            .args([
                "run",
                "--detach",
                "--name",
                &format!("webdock-plugin-setup-{nonce}"),
                "--label",
                "webdock.owned-plugin-setup=true",
                "--env",
                "POSTGRES_PASSWORD",
                "--env",
                "POSTGRES_DB=webdock_plugin_setup_test",
                "--publish",
                "127.0.0.1::5432",
                "postgres:18-alpine",
            ])
            .env("POSTGRES_PASSWORD", &password)
            .output()
            .unwrap();
        assert!(output.status.success(), "owned native fixture startup");
        let container = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        assert!(container.len() == 64 && container.bytes().all(|b| b.is_ascii_hexdigit()));
        let output = docker()
            .args(["port", &container, "5432/tcp"])
            .output()
            .unwrap();
        let address = String::from_utf8(output.stdout).unwrap();
        let port = address
            .trim()
            .strip_prefix("127.0.0.1:")
            .unwrap()
            .parse()
            .unwrap();
        let mut url =
            url::Url::parse("postgresql://postgres@127.0.0.1/webdock_plugin_setup_test").unwrap();
        url.set_port(Some(port)).unwrap();
        url.set_password(Some(&password)).unwrap();
        let passwords = webdock_persistence::roles::LOGIN_ROLES
            .iter()
            .copied()
            .chain(["webdock_offline_migrate"])
            .map(|role| {
                (
                    role.into(),
                    format!(
                        "{}{}",
                        uuid::Uuid::new_v4().simple(),
                        uuid::Uuid::new_v4().simple()
                    ),
                )
            })
            .collect();
        let fixture = Self {
            container,
            url,
            passwords,
        };
        let owner = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(db) = connect(fixture.url.as_str()).await {
                    break db;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        webdock_persistence::roles::provision_roles(
            &owner,
            "webdock_plugin_setup_test",
            &fixture.passwords,
        )
        .await
        .unwrap();
        webdock_persistence::migrations::bootstrap(&owner, "webdock_plugin_setup_test", SECRET)
            .await
            .unwrap();
        owner.close().await.unwrap();
        fixture
    }
    pub async fn command(
        &self,
        bin: &str,
        input: PathBuf,
        output: PathBuf,
    ) -> std::process::Output {
        let mut url = self.url.clone();
        url.set_username("webdock_offline_migrate").unwrap();
        url.set_password(Some(&self.passwords["webdock_offline_migrate"]))
            .unwrap();
        let target = format!("127.0.0.1:{}{}", url.port().unwrap(), url.path());
        tokio::time::timeout(
            Duration::from_secs(45),
            tokio::process::Command::new(bin)
                .env_clear()
                .env("DATABASE_URL_UNPOOLED", url.as_str())
                .args(["--target", &target, "--config"])
                .arg(input)
                .arg("--output")
                .arg(output)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap()
    }
    pub async fn rejected_connection(
        &self,
        bin: &str,
        input: PathBuf,
        output: PathBuf,
        role: &str,
        options: Option<&str>,
        database: &str,
    ) -> std::process::Output {
        let mut url = self.url.clone();
        url.set_path(database);
        if role != "postgres" {
            url.set_username(role).unwrap();
            url.set_password(Some(&self.passwords[role])).unwrap();
        }
        if let Some(options) = options {
            url.query_pairs_mut().append_pair("options", options);
        }
        let target = format!("127.0.0.1:{}{}", url.port().unwrap(), url.path());
        tokio::process::Command::new(bin)
            .env_clear()
            .env("DATABASE_URL_UNPOOLED", url.as_str())
            .args(["--target", &target, "--config"])
            .arg(input)
            .arg("--output")
            .arg(output)
            .kill_on_drop(true)
            .output()
            .await
            .unwrap()
    }
    pub async fn assert_remote(&self, output: &Value, pin: &str) {
        let owner = connect(self.url.as_str()).await.unwrap();
        let row=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT transport,unix_uid,certificate_sha256 FROM webdock_auth.agent_transport_enrollment WHERE cluster_id=$1",[output["clusterID"].as_str().unwrap().into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("", "transport").unwrap(), "mtls");
        assert!(
            row.try_get::<Option<i64>>("", "unix_uid")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            row.try_get::<String>("", "certificate_sha256").unwrap(),
            pin
        );
        owner.close().await.unwrap();
    }
    pub async fn assert_setup(&self, secret: &Value) {
        let owner = connect(self.url.as_str()).await.unwrap();
        let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.hosting_cluster) AS clusters,(SELECT count(*) FROM webdock_auth.hosting_agent) AS agents,(SELECT count(*) FROM webdock_auth.agent_transport_enrollment) AS transports,(SELECT count(*) FROM webdock_admin.customers) AS customers,(SELECT count(*) FROM webdock_admin.projects) AS projects,(SELECT count(*) FROM webdock_auth.plugin_installation WHERE kind='external') AS plugins")).await.unwrap().unwrap();
        for field in ["clusters", "agents", "transports"] {
            assert_eq!(row.try_get::<i64>("", field).unwrap(), 1);
        }
        for field in ["customers", "projects", "plugins"] {
            assert_eq!(row.try_get::<i64>("", field).unwrap(), 0);
        }
        let value=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT c.verified,c.region,c.generation,c.runtime_installation,a.credential_hash,e.credential_hash AS transport_hash FROM webdock_auth.hosting_cluster c JOIN webdock_auth.hosting_agent a ON a.cluster_id=c.id JOIN webdock_auth.agent_transport_enrollment e ON e.cluster_id=c.id WHERE c.id=$1",[secret["clusterID"].as_str().unwrap().into()])).await.unwrap().unwrap();
        assert!(value.try_get::<bool>("", "verified").unwrap());
        assert_eq!(value.try_get::<String>("", "region").unwrap(), "eu");
        assert_eq!(value.try_get::<i32>("", "generation").unwrap(), 1);
        assert_eq!(
            value.try_get::<String>("", "runtime_installation").unwrap(),
            "owned-fixture"
        );
        use sha2::{Digest, Sha256};
        let expected = hex::encode(Sha256::digest(
            secret["credential"].as_str().unwrap().as_bytes(),
        ));
        assert!(value.try_get::<String>("", "credential_hash").unwrap() == expected);
        assert!(value.try_get::<String>("", "transport_hash").unwrap() == expected);
        owner.close().await.unwrap();
    }
}
