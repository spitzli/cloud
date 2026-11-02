use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};
use std::{os::unix::fs::PermissionsExt, process::Command, time::Duration};
use webdock_plugin_host::provisioning::tls::issue_runtime_identity;

pub struct Fixture {
    pub directory: tempfile::TempDir,
    pub ca: String,
    pub ca_key: String,
    pub admin_url: String,
    pub port: u16,
    pub container: String,
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
impl Fixture {
    pub async fn start() -> Self {
        assert_eq!(
            std::env::var("WEBDOCK_PLUGIN_PROVISIONING_DOCKER_TEST").as_deref(),
            Ok("1")
        );
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = params.self_signed(&key).unwrap().pem();
        let ca_key = key.serialize_pem();
        let leaf = issue_runtime_identity(&ca, &ca_key, "localhost").unwrap();
        std::fs::write(directory.path().join("server.crt"), leaf.certificate_pem).unwrap();
        std::fs::write(directory.path().join("server.key"), leaf.private_key_pem).unwrap();
        std::fs::set_permissions(
            directory.path().join("server.key"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let password = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let name = format!(
            "webdock-plugin-provisioning-{}",
            uuid::Uuid::new_v4().simple()
        );
        let output = docker().args(["run", "--detach", "--name", &name, "--label", "webdock.owned-plugin-provisioning=true", "--env", "POSTGRES_PASSWORD", "--publish", "127.0.0.1::5432", "--mount", &format!("type=bind,source={},target=/test-tls,readonly", directory.path().display()), "--entrypoint", "sh", "postgres:18-alpine", "-c", "mkdir /tmp/webdock-tls; cp /test-tls/server.* /tmp/webdock-tls/; chown -R postgres:postgres /tmp/webdock-tls; chmod 0600 /tmp/webdock-tls/server.key; exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/webdock-tls/server.crt -c ssl_key_file=/tmp/webdock-tls/server.key"]).env("POSTGRES_PASSWORD", &password).output().unwrap();
        assert!(
            output.status.success(),
            "owned TLS PostgreSQL fixture startup failed"
        );
        let container = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        assert!(container.len() == 64 && container.bytes().all(|b| b.is_ascii_hexdigit()));
        let port_output = docker()
            .args(["port", &container, "5432/tcp"])
            .output()
            .unwrap();
        let address = String::from_utf8(port_output.stdout).unwrap();
        let port = address
            .trim()
            .strip_prefix("127.0.0.1:")
            .unwrap()
            .parse()
            .unwrap();
        let mut url = url::Url::parse("postgresql://postgres@localhost/postgres").unwrap();
        url.set_port(Some(port)).unwrap();
        url.set_password(Some(&password)).unwrap();
        let fixture = Self {
            directory,
            ca,
            ca_key,
            admin_url: url.into(),
            port,
            container,
        };
        for attempt in 0..100 {
            if webdock_plugin_kit::postgres::connect(
                &fixture.admin_url,
                Some(fixture.ca.as_bytes()),
            )
            .await
            .is_ok()
            {
                return fixture;
            }
            assert!(
                attempt < 99,
                "owned TLS PostgreSQL fixture did not become ready"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        unreachable!()
    }
    pub async fn admin(&self) -> tokio_postgres::Client {
        webdock_plugin_kit::postgres::connect(&self.admin_url, Some(self.ca.as_bytes()))
            .await
            .unwrap()
    }
}
