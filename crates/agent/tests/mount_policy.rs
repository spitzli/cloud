mod support;
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::Path};
use webdock_agent::policy::{GrantKind, MountGrant, RuntimePolicy, RuntimePolicyConfig};
use webdock_supervisor::{MountReference, PluginBinding};

#[test]
fn socket_directory_grants_preserve_the_wire_shape_and_reject_secret_fields() {
    let wire = serde_json::json!({"kind": "socket_directory"});
    let grant: GrantKind = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(grant).unwrap(), wire);
    assert!(
        serde_json::from_value::<GrantKind>(
            serde_json::json!({"kind": "socket_directory", "sha256": "untrusted"})
        )
        .is_err()
    );
}

fn private() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn file(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
#[test]
fn secret_mount_requires_reviewed_descriptor_scope_revision_and_immutable_file() {
    let dir = private();
    let uid = rustix::process::geteuid().as_raw();
    let artifact = dir.path().join("artifacts");
    std::fs::create_dir(&artifact).unwrap();
    std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    manifest["jobs"] = serde_json::json!([]);
    manifest["secrets"] = serde_json::json!(["provider"]);
    manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .push("plugin.secrets.read".into());
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    file(&artifact.join(format!("{hash}.json")), &bytes);
    let mount_root = dir.path().join("mounts");
    let source = mount_root.join("test-install/123/456/secrets/provider");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    for path in [
        mount_root.clone(),
        mount_root.join("test-install"),
        mount_root.join("test-install/123"),
        mount_root.join("test-install/123/456"),
        source.parent().unwrap().to_path_buf(),
    ] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    file(&source, b"scoped-test-secret");
    let config = RuntimePolicyConfig {
        registry_config: None,
        artifact_directory: Some(artifact),
        artifacts: vec![hash.clone()],
        mount_root: Some(mount_root),
        runtime_uid: Some(uid),
        grants: vec![MountGrant {
            installation: "test-install".into(),
            name: "provider".into(),
            revision: 1,
            project_id: "123".into(),
            resource_id: "456".into(),
            kind: GrantKind::Secret {
                sha256: format!("{:x}", Sha256::digest(b"scoped-test-secret")),
            },
        }],
    };
    let policy = RuntimePolicy::load(config).unwrap();
    let mut spec = support::spec();
    spec.image = manifest["image"].as_str().unwrap().into();
    spec.plugin = Some(PluginBinding {
        manifest_sha256: hash,
        plugin_id: "project-notes".into(),
        version: "1.0.0".into(),
        runtime_generation: 1,
    });
    spec.mounts = vec![MountReference {
        name: "provider".into(),
        revision: 1,
    }];
    let mounts = policy.local_mounts(&spec).unwrap();
    assert_eq!(mounts[0].target, "/run/secrets/provider");
    assert!(mounts[0].read_only);
    assert_eq!(mounts[0].source, source);
    let mut wrong = spec.clone();
    wrong.project_id = webdock_core::id::RecordId::parse("999").unwrap();
    assert!(policy.local_mounts(&wrong).is_err());
    let mut wrong = spec.clone();
    wrong.mounts[0].revision = 2;
    assert!(policy.local_mounts(&wrong).is_err());
    let mut wrong = spec.clone();
    wrong.image = support::spec().image;
    assert!(policy.local_mounts(&wrong).is_err());
    file(&source, b"replaced-without-review");
    assert!(policy.local_mounts(&spec).is_err());
}
#[test]
fn undeclared_mounts_and_plugin_bindings_fail_closed() {
    let policy = RuntimePolicy::load(Default::default()).unwrap();
    let mut spec = support::spec();
    spec.mounts = vec![MountReference {
        name: "docker-socket".into(),
        revision: 1,
    }];
    assert!(policy.local_mounts(&spec).is_err());
}
