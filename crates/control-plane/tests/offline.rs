//! Reject ambiguous offline migration targets before connecting or inspecting a database.
use std::process::Command;

#[test]
fn migration_cli_rejects_overrides_and_never_prints_connection_credentials() {
    for query in [
        "host=elsewhere.invalid",
        "hostaddr=192.0.2.1",
        "port=5433",
        "dbname=other",
        "sslmode=verify-full&ssl-mode=disable",
        "unrecognized_credential=private-fixture-credential",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_spitzli-migrate"))
            .env_clear()
            .env(
                "SPITZLI_MIGRATION_DATABASE_URL",
                format!(
                    "postgres://runtime:private-fixture-credential@127.0.0.1:1/webdock_test?{query}"
                ),
            )
            .args(["--target", "127.0.0.1:1/webdock_test"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap().trim(),
            "Error: Target"
        );
    }
}

#[test]
fn agent_listener_rejects_unavailable_mail_configuration_before_connecting() {
    use std::io::Write;
    let mut listener = tempfile::NamedTempFile::new().unwrap();
    listener
        .write_all(
            br#"{"kind":"unix","path":"/unused-fixture.sock","installation":"owned-fixture","agent_id":"owned-agent","uid":0}"#,
        )
        .unwrap();
    for (features, agent, expected) in [
        (
            r#"{"hosting":"enabled","mail":"disabled"}"#,
            "true",
            "Mail configuration execution requires a Mail feature state permitting accepted work",
        ),
        (
            r#"{"hosting":"enabled","mail":"enabled"}"#,
            "false",
            "IncomingAgentDisabled",
        ),
        (
            r#"{"mail":"enabled"}"#,
            "true",
            "Missing SPITZLI_DATABASE_URL",
        ),
        (
            r#"{"mail":"draining"}"#,
            "true",
            "Missing SPITZLI_DATABASE_URL",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_spitzli-agent-control"))
            .env_clear()
            .env("SPITZLI_FEATURES", features)
            .env("SPITZLI_NATIVE_MAIL_ENABLED", "true")
            .env("SPITZLI_NATIVE_MAIL_AGENT_ENABLED", agent)
            .env("SPITZLI_NATIVE_MAILBOXES_ENABLED", "true")
            .env("SPITZLI_NATIVE_MAIL_CONFIGURATION_ENABLED", "true")
            .arg("--config")
            .arg(listener.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(expected), "{error}");
    }
}
