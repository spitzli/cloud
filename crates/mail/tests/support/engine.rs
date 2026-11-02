//! Real Community Edition engine, isolated local Docker network, only loopback published ports.
//! All identities, domains, passwords and volumes are generated for this disposable fixture.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use webdock_mail::jmap::ManagementClient;
pub const PASSWORD: &str = "Rust-Disposable-Mail-Only!123456";
pub fn docker(args: &[&str]) -> String {
    let output = Command::new("docker")
        .args(["--host", "unix:///var/run/docker.sock"])
        .args(args)
        .env_remove("DOCKER_CONTEXT")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owned Docker fixture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
pub fn docker_quiet(args: &[&str]) {
    let _ = Command::new("docker")
        .args(["--host", "unix:///var/run/docker.sock"])
        .args(args)
        .env_remove("DOCKER_CONTEXT")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
pub struct Fixture {
    pub name: String,
    network: String,
    config: String,
    data: String,
    label: String,
    proxies: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.proxies {
            task.abort();
        }
        docker_quiet(&["rm", "-f", &self.name]);
        docker_quiet(&["volume", "rm", &self.config, &self.data]);
        docker_quiet(&["network", "rm", &self.network]);
    }
}
impl Fixture {
    pub async fn create() -> (Self, u16, u16) {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("webdock-rust-mail-{suffix}");
        let mut fixture = Self {
            network: format!("{name}-network"),
            config: format!("{name}-config"),
            data: format!("{name}-data"),
            label: format!("webdock.rust-mail-fixture={suffix}"),
            name,
            proxies: vec![],
        };
        docker(&[
            "network",
            "create",
            "--internal",
            "--label",
            &fixture.label,
            &fixture.network,
        ]);
        docker(&[
            "volume",
            "create",
            "--label",
            &fixture.label,
            &fixture.config,
        ]);
        docker(&["volume", "create", "--label", &fixture.label, &fixture.data]);
        docker(&[
            "run",
            "-d",
            "--name",
            &fixture.name,
            "--label",
            &fixture.label,
            "--network",
            &fixture.network,
            "--read-only",
            "--cap-drop",
            "ALL",
            "--cap-add",
            "NET_BIND_SERVICE",
            "--security-opt",
            "no-new-privileges:true",
            "--memory",
            "1g",
            "--cpus",
            "1",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,size=64m",
            "--mount",
            &format!("type=volume,source={},target=/etc/stalwart", fixture.config),
            "--mount",
            &format!(
                "type=volume,source={},target=/var/lib/stalwart",
                fixture.data
            ),
            "--env",
            "STALWART_RECOVERY_ADMIN=admin:rust-mail-disposable-admin-only",
            "--env",
            "STALWART_PUBLIC_URL=http://mail.customer.test:8080",
            "stalwartlabs/stalwart:v0.16.25@sha256:74e5a7d55303ba525d939c6bf97ed4e010df7521f52d80afc22a815b66bd53f3",
        ]);
        assert_eq!(
            docker(&[
                "inspect",
                "--format",
                "{{ index .Config.Labels \"webdock.rust-mail-fixture\" }}",
                &fixture.name
            ]),
            suffix
        );
        assert_eq!(
            docker(&[
                "network",
                "inspect",
                "--format",
                "{{.Internal}}",
                &fixture.network
            ]),
            "true"
        );
        assert_eq!(
            docker(&["inspect", "--format", "{{.State.Status}}", &fixture.name]),
            "running"
        );
        let ip = docker(&[
            "inspect",
            "--format",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            &fixture.name,
        ])
        .parse::<std::net::Ipv4Addr>()
        .unwrap();
        assert!(ip.is_private());
        // Docker does not publish ports for an internal network. Owned loopback TCP forwarding
        // gives the native test client access without giving the engine internet egress.
        let mut ports = Vec::new();
        for port in [8080, 465] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            ports.push(listener.local_addr().unwrap().port());
            fixture.proxies.push(tokio::spawn(async move {
                while let Ok((incoming, _)) = listener.accept().await {
                    tokio::spawn(forward(incoming, ip, port));
                }
            }));
        }
        (fixture, ports[0], ports[1])
    }
}
pub async fn forward(mut incoming: tokio::net::TcpStream, ip: std::net::Ipv4Addr, port: u16) {
    let Ok(mut upstream) = tokio::net::TcpStream::connect((ip, port)).await else {
        return;
    };
    let _ = tokio::io::copy_bidirectional(&mut incoming, &mut upstream).await;
}
pub fn basic(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}
pub async fn ready(client: &ManagementClient) {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if client
            .call("x:Bootstrap/get", json!({"ids":["singleton"]}))
            .await
            .is_ok()
        {
            return;
        }
        assert!(
            Instant::now() < until,
            "owned Mail engine did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
/// One configured engine with a `customer.test` domain and the named password accounts.
pub struct Engine {
    pub fixture: Fixture,
    /// Loopback JMAP/management origin with a trailing slash.
    pub origin: String,
    pub smtp: u16,
    pub accounts: Vec<String>,
}
impl Engine {
    pub async fn provision(users: &[&str]) -> Self {
        let (fixture, http, smtp) = Fixture::create().await;
        let origin = format!("http://127.0.0.1:{http}/");
        let management = ManagementClient::local_fixture(
            &origin,
            &basic("admin", "rust-mail-disposable-admin-only"),
        )
        .unwrap();
        ready(&management).await;
        management.call("x:Bootstrap/set",json!({"update":{"singleton":{"serverHostname":"mail.customer.test","defaultDomain":"customer.test","requestTlsCertificate":false,"generateDkimKeys":false,"tracer":{"@type":"Stdout","level":"warn"}}}})).await.unwrap();
        docker(&["restart", &fixture.name]);
        ready(&management).await;
        let domains = management
            .call("x:Domain/get", json!({"ids":null}))
            .await
            .unwrap();
        let domain = domains["list"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "customer.test")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        let mut accounts = Vec::new();
        for user in users {
            let created=management.call("x:Account/set",json!({"create":{"mailbox":{"@type":"User","name":user,"domainId":domain,"credentials":{"0":{"@type":"Password","secret":PASSWORD}},"roles":{"@type":"User"},"permissions":{"@type":"Inherit"},"encryptionAtRest":{"@type":"Disabled"},"quotas":{"maxDiskQuota":10485760}}}})).await.unwrap();
            accounts.push(
                created["created"]["mailbox"]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        Self {
            fixture,
            origin,
            smtp,
            accounts,
        }
    }
}
