//! Only supported native setup APIs/CLIs establish these fixture records.
use serde_json::json;
use std::{collections::HashMap, os::unix::fs::PermissionsExt};

#[tokio::test]
async fn setup_rejects_unreviewed_targets_before_creating_private_output() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = dir.path().join("input.json");
    std::fs::write(&config, b"{}").unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = dir.path().join("kernel");
    for raw in [
        "postgresql://postgres:private@localhost/db",
        "postgresql://webdock_auth_runtime:private@localhost/db",
        "postgresql://webdock_offline_migrate:private@localhost/db?host=elsewhere.invalid",
    ] {
        let env = HashMap::from([("DATABASE_URL_UNPOOLED".into(), raw.into())]);
        assert!(
            webdock_control_plane::plugin_bootstrap::bootstrap_plugins(
                &env,
                "localhost:5432/db",
                &config,
                &output
            )
            .await
            .is_err()
        );
        assert!(!output.exists());
        assert!(
            webdock_control_plane::plugin_bootstrap::enroll_agent(
                &env,
                "localhost:5432/db",
                &config,
                &output
            )
            .await
            .is_err()
        );
        assert!(!output.exists());
    }
}

#[path = "support/plugin_setup_fixture.rs"]
mod fixture;
#[tokio::test]
#[ignore = "requires WEBDOCK_PLUGIN_SETUP_DOCKER_TEST=1; exclusively owned fresh native PostgreSQL"]
async fn fresh_native_clis_create_private_replayable_kernel_and_agent_without_seed_rows() {
    use std::os::unix::fs::MetadataExt;
    let f = fixture::Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let catalog = dir.path().join("catalog.json");
    fixture::private(
        &catalog,
        &json!({"version":1,"platformID":"owned-fixture","keys":[],"targets":[],"releases":[]}),
    );
    let input = dir.path().join("kernel-input.json");
    fixture::private(
        &input,
        &json!({"platformID":"owned-fixture","installationCatalog":catalog,"callbackOrigin":"https://core.example.test:8443","listenAddress":"127.0.0.1:18443"}),
    );
    for (role, options, database) in [
        ("webdock_auth_runtime", None, "webdock_plugin_setup_test"),
        (
            "postgres",
            Some("-c role=webdock_offline_migrate"),
            "webdock_plugin_setup_test",
        ),
        (
            "webdock_offline_migrate",
            Some("-c role=webdock_platform_owner"),
            "webdock_plugin_setup_test",
        ),
        ("webdock_offline_migrate", None, "postgres"),
    ] {
        let refused = dir.path().join("refused-kernel");
        let result = f
            .rejected_connection(
                env!("CARGO_BIN_EXE_webdock-bootstrap-plugins"),
                input.clone(),
                refused.clone(),
                role,
                options,
                database,
            )
            .await;
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert_eq!(result.stderr, b"plugin_bootstrap_failed\n");
        assert!(!refused.exists());
    }
    let output = dir.path().join("kernel");
    let run = |bin: &'static str, config: std::path::PathBuf, out: std::path::PathBuf| {
        f.command(bin, config, out)
    };
    let result = run(
        env!("CARGO_BIN_EXE_webdock-bootstrap-plugins"),
        input.clone(),
        output.clone(),
    )
    .await;
    assert!(
        result.status.success(),
        "supported kernel CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    let config = webdock_control_plane::plugin_config::PluginConfig::load(
        &output.join("plugin-config.json"),
    )
    .unwrap();
    assert!(config.host_bindings.is_absolute());
    assert!(config.connections.is_absolute());
    for file in [
        "plugin-config.json",
        "host-bindings.json",
        "connections.json",
        "invocation.key",
        "ca.crt",
        "ca.key",
        "core.crt",
        "core.key",
        "bootstrap.json",
    ] {
        assert_eq!(output.join(file).metadata().unwrap().mode() & 0o777, 0o600);
    }
    let key = std::fs::read(output.join("invocation.key")).unwrap();
    let repeated = run(
        env!("CARGO_BIN_EXE_webdock-bootstrap-plugins"),
        input.clone(),
        output.clone(),
    )
    .await;
    assert!(repeated.status.success());
    assert!(repeated.stdout.is_empty());
    assert!(
        std::fs::read(output.join("invocation.key")).unwrap() == key,
        "exact replay never rotates kernel signing identity"
    );
    let copied = dir.path().join("copied-kernel");
    std::fs::create_dir(&copied).unwrap();
    std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o700)).unwrap();
    for entry in std::fs::read_dir(&output).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), copied.join(entry.file_name())).unwrap();
    }
    assert!(
        !run(
            env!("CARGO_BIN_EXE_webdock-bootstrap-plugins"),
            input.clone(),
            copied
        )
        .await
        .status
        .success(),
        "copied configuration with paths to another output directory is not exact replay"
    );
    let agent_input = dir.path().join("agent-input.json");
    fixture::private(
        &agent_input,
        &json!({"platformID":"owned-fixture","agentID":"owned-agent","name":"Reviewed EU cluster","country":"de","region":"eu","locationEvidence":"Owned local Kubernetes fixture reviewed in Germany","transport":{"kind":"unix","uid":rustix::process::geteuid().as_raw()}}),
    );
    let enrollment = dir.path().join("agent.json");
    let enrolled = run(
        env!("CARGO_BIN_EXE_webdock-enroll-agent"),
        agent_input.clone(),
        enrollment.clone(),
    )
    .await;
    assert!(
        enrolled.status.success(),
        "supported agent CLI failed: {}",
        String::from_utf8_lossy(&enrolled.stderr)
    );
    assert!(enrolled.stdout.is_empty());
    let secret: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&enrollment).unwrap()).unwrap();
    assert_eq!(secret["identity"]["installation"], "owned-fixture");
    assert_eq!(secret["identity"]["generation"], 1);
    assert_eq!(secret["credential"].as_str().unwrap().len(), 43);
    let before = std::fs::read(&enrollment).unwrap();
    let repeated = run(
        env!("CARGO_BIN_EXE_webdock-enroll-agent"),
        agent_input.clone(),
        enrollment.clone(),
    )
    .await;
    assert!(repeated.status.success());
    assert!(std::fs::read(&enrollment).unwrap() == before);
    f.assert_setup(&secret).await;
    // Remote enrollment pins the actual reviewed leaf and writes the matching mTLS record.
    let ca = std::fs::read_to_string(output.join("ca.crt")).unwrap();
    let ca_key = std::fs::read_to_string(output.join("ca.key")).unwrap();
    let leaf = webdock_plugin_host::provisioning::tls::issue_runtime_identity(
        &ca,
        &ca_key,
        "agent.example.test",
    )
    .unwrap();
    let certificate = dir.path().join("agent.crt");
    std::fs::write(&certificate, &leaf.certificate_pem).unwrap();
    std::fs::set_permissions(&certificate, std::fs::Permissions::from_mode(0o600)).unwrap();
    let remote_input = dir.path().join("remote-input.json");
    fixture::private(
        &remote_input,
        &json!({"platformID":"owned-fixture","agentID":"remote-agent","name":"Reviewed remote EU cluster","country":"de","region":"eu","locationEvidence":"Reviewed remote fixture certificate","transport":{"kind":"mtls","certificate":certificate}}),
    );
    let remote_output = dir.path().join("remote-agent.json");
    let result = run(
        env!("CARGO_BIN_EXE_webdock-enroll-agent"),
        remote_input.clone(),
        remote_output.clone(),
    )
    .await;
    assert!(result.status.success());
    assert!(result.stdout.is_empty());
    let remote: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&remote_output).unwrap()).unwrap();
    f.assert_remote(&remote, &leaf.certificate_sha256).await;
    assert!(
        run(
            env!("CARGO_BIN_EXE_webdock-enroll-agent"),
            remote_input,
            remote_output
        )
        .await
        .status
        .success()
    );
    // Lost output cannot authorize replacement or mint another credential.
    let absent = run(
        env!("CARGO_BIN_EXE_webdock-enroll-agent"),
        agent_input.clone(),
        dir.path().join("replacement.json"),
    )
    .await;
    assert!(!absent.status.success());
    assert!(!dir.path().join("replacement.json").exists());
    let mut changed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_input).unwrap()).unwrap();
    changed["name"] = "Different cluster".into();
    fixture::private(&agent_input, &changed);
    assert!(
        !run(
            env!("CARGO_BIN_EXE_webdock-enroll-agent"),
            agent_input,
            enrollment.clone()
        )
        .await
        .status
        .success()
    );
    assert!(std::fs::read(&enrollment).unwrap() == before);
    std::fs::remove_file(output.join("core.key")).unwrap();
    assert!(
        !run(
            env!("CARGO_BIN_EXE_webdock-bootstrap-plugins"),
            input,
            output.clone()
        )
        .await
        .status
        .success()
    );
    assert!(
        !output.join("core.key").exists(),
        "partial output is never repaired with replacement identity"
    );
}
