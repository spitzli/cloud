use webdock_agent::registry::{RegistryConfig, RegistryCredential};
fn configuration() -> serde_json::Value {
    serde_json::json!({"version":1,"euStorageEvidence":"Precise country/EU placement unverified; best-effort geography accepted.","projects":{"2":{"repository":"registry.example/tenant-1/project-2","pullSecret":"registry-project-2","readOnly":true,"credentialScopeEvidence":"read-only repository-scoped robot"}}})
}
#[test]
fn registry_policy_preserves_exact_project_repository_and_truthful_evidence() {
    let config = RegistryConfig::parse(&serde_json::to_vec(&configuration()).unwrap()).unwrap();
    let image = format!(
        "registry.example/tenant-1/project-2@sha256:{}",
        "a".repeat(64)
    );
    assert_eq!(
        config
            .select("2", &image)
            .unwrap()
            .unwrap()
            .pull_secret
            .as_deref(),
        Some("registry-project-2")
    );
    assert!(config.select("3", &image).is_err());
    assert!(
        config
            .select("2", &image.replace("tenant-1", "tenant-2"))
            .is_err()
    );
    assert!(
        config
            .select("2", "registry.example/tenant-1/project-2:latest")
            .is_err()
    );
    assert!(
        config
            .select(
                "2",
                &format!("docker.io/library/busybox@sha256:{}", "b".repeat(64))
            )
            .unwrap()
            .is_none()
    );
    let mut config = configuration();
    config["projects"]["2"]["readOnly"] = false.into();
    assert!(RegistryConfig::parse(&serde_json::to_vec(&config).unwrap()).is_err());
}
#[test]
fn registry_credentials_are_single_scope_and_never_execute_helpers() {
    let bytes = br#"{"auths":{"registry.example":{"auth":"cmVhZGVyOnRlc3Qtc2VjcmV0"}}}"#;
    let credentials = RegistryCredential::parse(bytes, "registry.example").unwrap();
    assert_eq!(credentials.username.as_deref(), Some("reader"));
    assert!(RegistryCredential::parse(bytes, "foreign.example").is_err());
    assert!(
        RegistryCredential::parse(
            br#"{"credsStore":"arbitrary-command","auths":{}}"#,
            "registry.example"
        )
        .is_err()
    );
    assert!(RegistryCredential::parse(br#"{"auths":{"registry.example":{"username":"reader","password":"test"},"foreign.example":{"username":"other","password":"test"}}}"#,"registry.example").is_err());
}
