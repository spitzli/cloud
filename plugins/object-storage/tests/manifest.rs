use serde_json::{Value, json};
use webdock_object_storage_plugin::validate_manifest;
use webdock_plugin_protocol::Manifest;
#[test]
fn every_implemented_operation_has_rest_and_mcp_parity_and_purge_retains_core_index() {
    let m = Manifest::parse(include_bytes!("../manifest.json")).unwrap();
    validate_manifest(&m).unwrap();
    let v = m.as_json();
    for a in v["actions"].as_array().unwrap() {
        for k in ["api", "mcp"] {
            assert_eq!(
                v[k].as_array()
                    .unwrap()
                    .iter()
                    .filter(|b| b["action"] == a["name"])
                    .count(),
                1
            );
        }
    }
    assert_eq!(m.action("buckets.purge").unwrap()["resource"], "use");
    assert_eq!(m.action("buckets.delete").unwrap()["resource"], "delete");
    assert_eq!(m.action("credentials.reveal").unwrap()["intent"], "write");
    assert_eq!(v["jobs"], json!([]));
    let mut changed: Value = v.clone();
    changed["api"] = json!([]);
    assert!(
        validate_manifest(&Manifest::parse(&serde_json::to_vec(&changed).unwrap()).unwrap())
            .is_err()
    );
}

#[test]
fn storage_actions_require_native_service_and_describe_only_public_descriptors() {
    let m = Manifest::parse(include_bytes!("../manifest.json")).unwrap();
    let v = m.as_json();
    assert!(
        v["services"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!({"name":"storage","version":1}))
    );
    for direction in ["upload", "download"] {
        let a = m.action(&format!("objects.{direction}")).unwrap();
        assert_eq!(a["storage"], direction);
        assert_eq!(a["resource"], "use");
        assert_eq!(
            a["intent"],
            if direction == "upload" {
                "write"
            } else {
                "read"
            }
        );
        assert!(
            a["output"]["description"]
                .as_str()
                .unwrap()
                .contains("TransferDescriptor")
        );
        assert!(a["output"]["properties"]["url"].is_null());
        assert!(a["output"]["properties"]["headers"].is_null());
        assert!(a["output"]["properties"]["path"].is_object());
    }
    assert_eq!(
        m.action("credentials.reveal").unwrap()["feature"],
        "manage-buckets"
    );
}

#[test]
fn public_output_schema_accepts_the_exported_descriptor_and_rejects_the_private_plan() {
    use webdock_plugin_protocol::{
        storage::{StoragePlan, TransferDescriptor},
        validate_value,
    };
    let m = Manifest::parse(include_bytes!("../manifest.json")).unwrap();
    for direction in ["upload", "download"] {
        let private = json!({"version":1,"binding":"managed-objects","direction":direction,"resourceId":"owned","resourceEpoch":"epoch-1","url":"https://provider.invalid/private?signature=not-public","headers":{},"expiresAt":1100,"exactBytes":3,"expectedSha256":"a".repeat(64),"attachment":{"filename":"proof.txt","contentType":"text/plain"}});
        let mut plan: StoragePlan = serde_json::from_value(private.clone()).unwrap();
        let descriptor = serde_json::to_value(
            TransferDescriptor::new(&uuid::Uuid::new_v4().to_string(), &plan).unwrap(),
        )
        .unwrap();
        let action = m.action(&format!("objects.{direction}")).unwrap();
        validate_value(action.get("output"), &descriptor).unwrap();
        assert!(validate_value(action.get("output"), &private).is_err());
        assert!(!descriptor.to_string().contains("provider.invalid"));
        assert!(descriptor.get("headers").is_none());
        assert!(descriptor.get("resourceEpoch").is_none());
        for required in ["expiresAt", "exactBytes"] {
            let mut incomplete = descriptor.clone();
            incomplete.as_object_mut().unwrap().remove(required);
            assert!(validate_value(action.get("output"), &incomplete).is_err());
        }
        if direction == "download" {
            plan.expected_sha256 = None;
            let unknown = serde_json::to_value(
                TransferDescriptor::new(&uuid::Uuid::new_v4().to_string(), &plan).unwrap(),
            )
            .unwrap();
            assert!(unknown.get("expectedSha256").is_none());
            validate_value(action.get("output"), &unknown).unwrap();
        }
    }
}
