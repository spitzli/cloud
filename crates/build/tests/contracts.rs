use serde_json::json;
use spitzli_build::contracts::BuildCommand;
#[test]
fn retained_command_names_ids_selectors_and_null_semantics() {
    assert!(BuildCommand::parse(json!({"action":"git.builds.request","projectID":"123","targetID":"456","idempotencyKey":"build-request-1234"})).is_ok());
    for id in ["0", "+1", "01", "-1", "9223372036854775808"] {
        assert!(BuildCommand::parse(json!({"action":"git.sources.list","projectID":id})).is_err());
    }
    assert!(BuildCommand::parse(json!({"action":"git.builds.request","projectID":"123","sourceID":"456","idempotencyKey":"build-request-1234"})).is_err());
    assert!(BuildCommand::parse(json!({"action":"git.builds.request","projectID":"123","targetID":null,"idempotencyKey":"build-request-1234"})).is_err());
    assert!(
        BuildCommand::parse(json!({"action":"git.builds.list","projectID":"123","page":0}))
            .is_err()
    );
    assert!(
        BuildCommand::parse(json!({"action":"git.builds.list","projectID":"123","limit":101}))
            .is_err()
    );
}
#[test]
fn customer_environment_never_changes_host_or_guest_control_configuration() {
    use spitzli_build::execution::{GuestRequest, Limits};
    use std::collections::BTreeMap;
    let mut request = GuestRequest {
        recipe: "dockerfile".into(),
        root_directory: ".".into(),
        build_environment: BTreeMap::new(),
        limits: Limits {
            cpu: 2,
            memory_mi_b: 4096,
            disk_bytes: 10 * 1024 * 1024 * 1024,
            duration_seconds: 1500,
        },
    };
    for key in [
        "PATH",
        "LD_PRELOAD",
        "GIT_CONFIG_GLOBAL",
        "WEBDOCK_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
    ] {
        request.build_environment = BTreeMap::from([(key.into(), "secret".into())]);
        assert!(request.validate().is_err());
    }
    request.build_environment = BTreeMap::from([("PUBLIC_API".into(), "safe-value".into())]);
    assert!(request.validate().is_ok());
    assert_eq!(request.redact("x=safe-value"), "x=[REDACTED]");
}
#[test]
fn readonly_ceiling_is_an_explicit_allowlist() {
    let ceiling = spitzli_build::BuildCeiling {
        read: true,
        write: false,
    };
    for action in [
        "git.connections.list",
        "git.repositories.list",
        "git.sources.list",
        "git.source.get",
        "git.builds.list",
        "git.builds.get",
        "git.releases.list",
    ] {
        assert!(ceiling.require(action).is_ok());
    }
    for action in [
        "git.connections.disconnect",
        "git.source.configure",
        "git.builds.request",
        "git.builds.cancel",
        "git.releases.approve",
        "git.releases.rollback",
        "git.future.get",
        "",
    ] {
        assert_eq!(
            ceiling.require(action).unwrap_err().code,
            "permission_denied"
        );
    }
    assert!(
        spitzli_build::BuildCeiling {
            read: false,
            write: true
        }
        .require("git.sources.list")
        .is_err()
    );
}

#[test]
fn kubernetes_backend_selection_is_explicit_and_bounded() {
    let base = json!({"action":"git.source.configure","projectID":"123","targetID":"456","connectionID":"789","repositoryID":"999","branch":"main","rootDirectory":".","recipe":"dockerfile","revision":0});
    assert!(BuildCommand::parse(base.clone()).is_ok());
    let mut explicit = base.clone();
    explicit["executionBackend"] = json!("kubernetes");
    assert!(BuildCommand::parse(explicit).is_ok());
    for backend in [
        json!("privileged"),
        json!("docker"),
        json!(""),
        serde_json::Value::Null,
    ] {
        let mut invalid = base.clone();
        invalid["executionBackend"] = backend;
        assert!(BuildCommand::parse(invalid).is_err());
    }
}

#[test]
fn log_cursors_are_read_only_and_finite() {
    let base = json!({"action":"git.builds.logs","projectID":"123","buildID":"456","after":"0","limit":20});
    assert!(BuildCommand::parse(base.clone()).is_ok());
    assert!(
        spitzli_build::BuildCeiling {
            read: true,
            write: false
        }
        .require("git.builds.logs")
        .is_ok()
    );
    for (key, value) in [
        ("after", json!("-1")),
        ("after", json!("9223372036854775808")),
        ("after", json!(3)),
        ("limit", json!(0)),
        ("limit", json!(101)),
    ] {
        let mut bad = base.clone();
        bad[key] = value;
        assert!(BuildCommand::parse(bad).is_err());
    }
}

#[test]
fn container_strategy_overrides_are_explicit_and_never_change_legacy_recipes() {
    let base = json!({"action":"git.source.configure","projectID":"123","targetID":"456","connectionID":"789","repositoryID":"999","branch":"main","rootDirectory":".","recipe":"dockerfile","revision":0,"executionBackend":"kubernetes","containerBuild":{"strategy":"railpack","contextDirectory":".","dockerfile":"Dockerfile","buildArgs":{"PUBLIC_NAME":"demo"}}});
    assert!(BuildCommand::parse(base.clone()).is_ok());
    for (field, value) in [
        ("strategy", json!("shell")),
        ("dockerfile", json!("../Dockerfile")),
        ("contextDirectory", json!("/host")),
        ("buildArgs", json!({"BUILDKIT_HOST":"other"})),
        ("buildArgs", json!({"HTTP_PROXY":"http://private"})),
    ] {
        let mut bad = base.clone();
        bad["containerBuild"][field] = value;
        assert!(BuildCommand::parse(bad).is_err());
    }
    let mut legacy = base;
    legacy["executionBackend"] = json!("microvm");
    assert!(BuildCommand::parse(legacy).is_err());
}

#[test]
fn mvp_environment_is_explicitly_production_and_never_creates_a_preview() {
    let base = json!({"action":"git.builds.request","projectID":"123","targetID":"456","idempotencyKey":"owned-production-build","environmentName":"production"});
    assert!(BuildCommand::parse(base.clone()).is_ok());
    for value in [
        json!("preview"),
        json!("preview-pr-1"),
        json!(""),
        serde_json::Value::Null,
    ] {
        let mut bad = base.clone();
        bad["environmentName"] = value;
        assert!(BuildCommand::parse(bad).is_err());
    }
}

#[test]
fn workflow_run_trigger_is_opt_in_native_only_and_omission_stays_distinct() {
    let value = json!({"action":"git.source.configure","projectID":"123","targetID":"456","connectionID":"789","repositoryID":"55","branch":"release","rootDirectory":".","recipe":"dockerfile","revision":0,"executionBackend":"kubernetes","webhookTrigger":"workflow_run","workflowPath":".github/workflows/quality.yml"});
    assert!(BuildCommand::parse(value.clone()).is_ok());
    let mut wrong = value.clone();
    wrong["executionBackend"] = json!("microvm");
    assert!(BuildCommand::parse(wrong).is_err());
    let mut wrong = value.clone();
    wrong["webhookTrigger"] = json!("check_suite");
    assert!(BuildCommand::parse(wrong).is_err());
    let mut omitted = value;
    omitted.as_object_mut().unwrap().remove("webhookTrigger");
    omitted.as_object_mut().unwrap().remove("workflowPath");
    assert!(BuildCommand::parse(omitted).is_ok());
}
