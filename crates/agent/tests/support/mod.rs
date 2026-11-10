use spitzli_core::id::RecordId;
use spitzli_supervisor::*;
pub fn spec() -> RuntimeSpec {
    RuntimeSpec {
        autoscaling: None,
        version: 1,
        installation: "test-install".into(),
        project_id: RecordId::parse("123").unwrap(),
        resource_id: RecordId::parse("456").unwrap(),
        revision: 1,
        image: format!("docker.io/library/busybox@sha256:{}", "a".repeat(64)),
        arguments: vec!["httpd".into(), "-f".into(), "-p".into(), "8080".into()],
        cpu_millicores: 100,
        memory_bytes: 32 * 1024 * 1024,
        ephemeral_bytes: 16 * 1024 * 1024,
        replicas: 1,
        port: 8080,
        health_path: Some("/".into()),
        health_host: None,
        volume: None,
        plugin: None,
        mounts: Vec::new(),
        hosting_binding: None,
        hosted_access: None,
    }
}
