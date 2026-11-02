use std::collections::BTreeMap;
use webdock_agent_protocol::plugin::*;
fn fixture() -> PluginRuntimeClaim {
    let files = MATERIAL_FILES
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                if *name == "manifest.json" {
                    "{}".into()
                } else {
                    "fixture".into()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let material = PluginRuntimeMaterial { files };
    PluginRuntimeClaim {
        spec: PluginRuntimeSpec {
            version: 1,
            platform_id: "fixture".into(),
            plugin_id: "postgres".into(),
            runtime_generation: 1,
            configuration_revision: 1,
            cluster_id: "123".into(),
            manifest_sha256: sha256(b"{}"),
            material_sha256: material.fingerprint().unwrap(),
            image: format!("registry.example/postgres@sha256:{}", "a".repeat(64)),
            namespace: PluginRuntimeSpec::namespace_name("fixture", "postgres"),
            service: PluginRuntimeSpec::service_name().into(),
            runtime_host: "postgres.plugins.example".into(),
            port: 3443,
            entry_point: "plugins".into(),
            cpu_millicores: 100,
            memory_bytes: 134217728,
            egress: vec![PluginRuntimeEgress {
                cidr: "192.0.2.1/32".into(),
                port: 443,
            }],
            host_aliases: vec![],
        },
        lease: PluginRuntimeLease {
            operation_id: "456".into(),
            generation: 1,
            token: "b".repeat(64),
            lease_until: 200,
            agent_generation: 1,
            action: PluginRuntimeAction::Apply,
        },
        material,
        checkpoint: None,
    }
}
#[test]
fn installation_ownership_and_material_are_exact_and_lease_independent() {
    let mut claim = fixture();
    claim.validate(100).unwrap();
    let hash = claim.spec.fingerprint().unwrap();
    claim.lease.operation_id = "789".into();
    claim.lease.generation += 1;
    claim.lease.token = "c".repeat(64);
    assert_eq!(claim.spec.fingerprint().unwrap(), hash);
    claim.validate(100).unwrap();
    claim.spec.namespace = "wd-customer-123".into();
    assert!(claim.validate(100).is_err());
    let mut claim = fixture();
    claim
        .material
        .files
        .insert("../escape".into(), "secret".into());
    assert!(claim.validate(100).is_err());
    let mut claim = fixture();
    claim
        .material
        .files
        .get_mut("runtime.key")
        .unwrap()
        .push('x');
    assert!(claim.validate(100).is_err());
}
#[test]
fn rejects_unbounded_or_unsafe_contracts_without_customer_fields() {
    for (field, value) in [
        (
            "image",
            serde_json::json!(format!(
                "registry.example/postgres@sha256:{}",
                "0".repeat(64)
            )),
        ),
        ("namespace", serde_json::json!("kube-system")),
        ("cpuMillicores", serde_json::json!(0)),
        ("memoryBytes", serde_json::json!(1)),
        ("runtimeHost", serde_json::json!("bad/path")),
        ("projectID", serde_json::json!("123")),
        (
            "egress",
            serde_json::json!([{"cidr":"0.0.0.0/0","port":443}]),
        ),
    ] {
        let mut value_spec = serde_json::to_value(fixture().spec).unwrap();
        value_spec[field] = value;
        assert!(
            serde_json::from_value::<PluginRuntimeSpec>(value_spec)
                .map_or(true, |s| s.validate().is_err()),
            "{field}"
        );
    }
    assert!(fixture().validate(200).is_err());
    let mut claim = fixture();
    claim.lease.lease_until = 1000;
    assert!(claim.validate(100).is_err());
}
#[test]
fn material_bounds_and_callback_proofs_cannot_smuggle_private_fields() {
    let mut material = fixture().material;
    material
        .files
        .get_mut("runtime.json")
        .unwrap()
        .push_str(&"x".repeat(65536));
    assert!(material.validate().is_err());
    assert!(
        serde_json::from_value::<PluginRuntimeCheckpoint>(
            serde_json::json!({"runtimeKey":"private"})
        )
        .is_err()
    );
    let result = PluginRuntimeCompletion {
        outcome: PluginRuntimeOutcome::Succeeded,
        checkpoint: PluginRuntimeCheckpoint::default(),
        converged: true,
    };
    assert!(result.validate().is_err());
}
#[test]
fn large_signed_manifest_fits_wire_but_private_file_and_serialized_caps_hold() {
    let mut material = fixture().material;
    material
        .files
        .insert("manifest.json".into(), "x".repeat(262144));
    material.validate().unwrap();
    material
        .files
        .insert("runtime.json".into(), "x".repeat(65537));
    assert!(material.validate().is_err());
    let mut material = fixture().material;
    material.files.insert(
        "manifest.json".into(),
        char::from(34).to_string().repeat(262144),
    );
    assert!(material.validate().is_err());
}

#[test]
fn generation_namespaces_accept_v2_without_reinterpreting_v1_custody() {
    let mut spec = fixture().spec;
    spec.version = 2;
    spec.runtime_generation = 2;
    spec.namespace = "wd-plugin-03ef6c37c7cfdd6de366d541".into();
    spec.validate().unwrap();
    spec.runtime_generation = 3;
    assert!(
        spec.validate().is_err(),
        "another generation cannot adopt the namespace"
    );
    spec.version = 1;
    assert!(
        spec.validate().is_err(),
        "legacy custody keeps its original namespace"
    );
    spec.namespace = PluginRuntimeSpec::namespace_name("fixture", "postgres");
    spec.validate().unwrap();
}

#[test]
fn legacy_spec_keeps_its_existing_namespace_and_serialized_fingerprint() {
    let spec = fixture().spec;
    assert_eq!(spec.namespace, "wd-plugin-84f085a592d4a0c834cb9e85");
    assert_eq!(
        spec.fingerprint().unwrap(),
        "1369d793803e31dbe79865c68965763699890c7b7fba4ecb387f9858d3664446"
    );
}

#[test]
fn plugin_poll_request_preserves_old_wire_identity_and_bounds_versions() {
    use prost::Message;
    use webdock_agent_protocol::{AgentIdentity, PluginRuntimePollRequest, PollRequest};
    let identity = Some(AgentIdentity {
        installation: "fixture".into(),
        agent_id: "agent".into(),
        generation: 1,
    });
    let old = PollRequest {
        identity: identity.clone(),
    };
    let current = PluginRuntimePollRequest {
        identity: identity.clone(),
        max_spec_version: 2,
    };
    assert_eq!(
        PollRequest::decode(current.encode_to_vec().as_slice())
            .unwrap()
            .identity,
        identity
    );
    let decoded = PluginRuntimePollRequest::decode(old.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.identity, identity);
    assert_eq!(normalize_spec_version(decoded.max_spec_version).unwrap(), 1);
    assert_eq!(normalize_spec_version(2).unwrap(), 2);
    for invalid in [3, u32::MAX] {
        assert!(normalize_spec_version(invalid).is_err());
    }
}
