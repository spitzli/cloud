use webdock_plugin_protocol::Manifest;
#[test]
fn compiled_native_contract_keeps_bytes_on_core_storage_and_jobs_on_core() {
    let manifest = Manifest::parse(include_bytes!("../manifest.json")).unwrap();
    webdock_archives::validate_manifest(&manifest).unwrap();
    assert_eq!(
        manifest.action("archives.download").unwrap()["storage"],
        "download"
    );
    assert!(manifest.job("archives.produce").is_some());
}

#[cfg(not(feature = "local-native-fixture"))]
#[test]
fn normal_runtime_configuration_cannot_enable_native_minio_exceptions() {
    let value = serde_json::json!({"config":{"id":"fixture","endpoint":"https://123456abcdef.native-archives.test:19443","region":"us-east-1","bucket":"wd-archives-123456abcdef","namespace":"archives"},"credentials":{"accessKeyId":"test-only","secretAccessKey":"test-only"},"fixtureProfile":{"runId":"123456abcdef","expiresAt":2000,"caSha256":"a".repeat(64)}});
    assert!(serde_json::from_value::<webdock_archives::backend::BackendConfig>(value).is_err());
}

#[test]
fn buildlog_public_requests_select_only_core_ids_never_log_bytes() {
    let manifest = Manifest::parse(include_bytes!("../manifest.json")).unwrap();
    let action = manifest.action("archives.create").unwrap();
    let base = serde_json::json!({"request_id":"11111111-1111-4111-8111-111111111111","kind":"buildlog","source_id":"125","retention_seconds":3600});
    assert!(webdock_plugin_protocol::validate_value(action.get("input"), &base).is_ok());
    for field in ["logs", "log", "content", "bytes", "url", "path"] {
        let mut forged = base.clone();
        forged[field] = serde_json::json!("untrusted body");
        assert!(
            webdock_plugin_protocol::validate_value(action.get("input"), &forged).is_err(),
            "{field} must never become a buildlog source"
        );
    }
}
