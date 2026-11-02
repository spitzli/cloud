pub struct LocalRegistry {
    pub address: std::net::SocketAddr,
    name: String,
}
impl LocalRegistry {
    pub async fn start() -> Self {
        assert_eq!(
            std::env::var("WEBDOCK_BUILD_REGISTRY_TEST").as_deref(),
            Ok("1")
        );
        let image =
            "registry@sha256:ddf754342cfc8acc51a56d5d0ab6af06826461864460636d8bd5c546dab2a7b8";
        assert!(
            std::process::Command::new("docker")
                .args(["image", "inspect", image])
                .output()
                .unwrap()
                .status
                .success()
        );
        let name = format!(
            "webdock-build-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let started = std::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &name,
                "--read-only",
                "--tmpfs",
                "/var/lib/registry:rw,nosuid,nodev,size=128m",
                "--tmpfs",
                "/tmp:rw,nosuid,nodev,size=16m",
                "-p",
                "127.0.0.1::5000",
                "-e",
                "REGISTRY_HTTP_SECRET=isolated-fixture-only",
                image,
            ])
            .output()
            .unwrap();
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        let port = std::process::Command::new("docker")
            .args(["port", &name, "5000/tcp"])
            .output()
            .unwrap();
        let address = String::from_utf8(port.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let result = Self { address, name };
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(address).await.is_ok() {
                return result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("local registry did not start")
    }
}
impl Drop for LocalRegistry {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}
