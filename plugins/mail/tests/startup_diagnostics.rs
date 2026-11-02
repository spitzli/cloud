#[test]
fn startup_failure_does_not_echo_arguments_or_underlying_diagnostics() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_webdock-mail"))
        .env_remove("WEBDOCK_RECOVERY_COMPONENT_TEST")
        .arg("/fixture-private-configuration-marker")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        output.stderr == b"mail_plugin_start_failed\n",
        "unexpected startup diagnostics; contents withheld"
    );
}

#[test]
fn fixture_startup_markers_are_exact_and_never_echo_poison_or_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let poison = temporary.path().join("poison.json");
    std::fs::write(
        &poison,
        b"{\"password\":\"poison-secret\",\"url\":\"https://poison.invalid/private\"}",
    )
    .unwrap();
    std::fs::set_permissions(&poison, std::fs::Permissions::from_mode(0o600)).unwrap();
    for (path, stages) in [
        (
            temporary.path().join("missing-private.json"),
            vec!["args", "config_read"],
        ),
        (poison, vec!["args", "config_read", "config_decode"]),
    ] {
        for mode in [None, Some("0"), Some("true"), Some("01"), Some("1")] {
            let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_webdock-mail"));
            command
                .arg(&path)
                .env_remove("WEBDOCK_RECOVERY_COMPONENT_TEST");
            if let Some(value) = mode {
                command.env("WEBDOCK_RECOVERY_COMPONENT_TEST", value);
            }
            let output = command.output().unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            let expected = if mode == Some("1") {
                stages
                    .iter()
                    .map(|stage| format!("mail_startup_stage stage={stage}\n"))
                    .collect::<String>()
                    + "mail_plugin_start_failed\n"
            } else {
                "mail_plugin_start_failed\n".to_owned()
            };
            if mode == Some("1") && !path.exists() {
                let lines = output
                    .stderr
                    .split(|b| *b == b'\n')
                    .filter(|v| !v.is_empty())
                    .collect::<Vec<_>>();
                assert_eq!(
                    lines.len(),
                    4,
                    "bounded metadata line required on actual read failure"
                );
                let prefix = b"mail_startup_read_metadata ";
                assert!(lines[2].starts_with(prefix));
                let value: serde_json::Value =
                    serde_json::from_slice(&lines[2][prefix.len()..]).unwrap();
                assert_eq!(value["file_type"], "unavailable");
                assert_eq!(value["errno"], 2);
                assert_eq!(value["metadata_errno"], 2);
                assert_eq!(value["absolute"], true);
                assert_eq!(value["expected_fixed_path"], false);
            } else {
                assert!(
                    output.stderr == expected.as_bytes(),
                    "unexpected CLI startup output; raw bytes withheld"
                );
            }
            assert!(!output.stderr.windows(6).any(|v| v == b"poison"));
        }
    }
}

#[test]
fn read_failure_metadata_is_scalar_only_for_real_mode_and_owner_cases() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("canary-private-mode.json");
    std::fs::write(&input, b"poison-private-value").unwrap();
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o644)).unwrap();
    for path in [input, std::fs::canonicalize("/usr/bin/true").unwrap()] {
        let normal = std::process::Command::new(env!("CARGO_BIN_EXE_webdock-mail"))
            .arg(&path)
            .env_remove("WEBDOCK_RECOVERY_COMPONENT_TEST")
            .output()
            .unwrap();
        assert!(!normal.status.success());
        assert!(normal.stdout.is_empty());
        assert!(normal.stderr == b"mail_plugin_start_failed\n");
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_webdock-mail"))
            .arg(&path)
            .env("WEBDOCK_RECOVERY_COMPONENT_TEST", "1")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let lines = output
            .stderr
            .split(|b| *b == b'\n')
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 4);
        let prefix = b"mail_startup_read_metadata ";
        assert!(lines[2].starts_with(prefix));
        let value: serde_json::Value = serde_json::from_slice(&lines[2][prefix.len()..]).unwrap();
        let metadata = path.symlink_metadata().unwrap();
        assert_eq!(value["file_uid"], metadata.uid());
        assert_eq!(value["mode"], metadata.mode() & 0o7777);
        assert_eq!(value["nlink"], metadata.nlink());
        assert_eq!(value["len"], metadata.len());
        assert_eq!(
            value["current_euid"],
            temporary.path().metadata().unwrap().uid()
        );
        assert_eq!(value["error_kind"], "non_io");
        assert!(value["errno"].is_null());
        assert!(!output.stderr.windows(6).any(|v| v == b"poison"));
        assert!(!output.stderr.windows(6).any(|v| v == b"canary"));
    }
}
