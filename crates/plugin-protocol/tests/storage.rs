use serde_json::json;
use webdock_plugin_protocol::storage::{StoragePlan, TransferDescriptor, validate_storage_result};
fn plan() -> serde_json::Value {
    json!({"version":1,"binding":"objects","direction":"download","resourceEpoch":"active-1","resourceId":"owned","url":"https://storage.invalid/bucket/key?X-Amz-Signature=private","headers":{},"expiresAt":1300,"exactBytes":3,"expectedSha256":"a".repeat(64),"attachment":{"filename":"report.bin","contentType":"application/octet-stream"}})
}
#[test]
fn private_plan_contract_binds_direction_resource_bytes_hash_and_expiry() {
    let p: StoragePlan = serde_json::from_value(plan()).unwrap();
    p.validate("download", "owned", 1000).unwrap();
    for (direction, resource, now) in [
        ("upload", "owned", 1000),
        ("download", "foreign", 1000),
        ("download", "owned", 1300),
        ("download", "owned", 999),
    ] {
        assert!(p.validate(direction, resource, now).is_err());
    }
    let mut v = plan();
    v["expectedSha256"] = json!(null);
    v["direction"] = json!("upload");
    assert!(
        serde_json::from_value::<StoragePlan>(v)
            .unwrap()
            .validate("upload", "owned", 1000)
            .is_err()
    );
    assert!(!format!("{p:?}").contains("Signature"));
}
#[test]
fn runtime_validates_private_contract_instead_of_public_manifest_schema() {
    let action = json!({"storage":"download","output":{"type":"object","required":["id","path"],"additionalProperties":false,"properties":{"id":{"type":"string"},"path":{"type":"string"}}}});
    validate_storage_result(&action, &json!({"resource_id":"owned"}), &plan(), 1000).unwrap();
    assert!(
        validate_storage_result(&action, &json!({"resource_id":"foreign"}), &plan(), 1000).is_err()
    );
    let p: StoragePlan = serde_json::from_value(plan()).unwrap();
    let descriptor = TransferDescriptor::new("123e4567-e89b-42d3-a456-426614174000", &p).unwrap();
    let public = serde_json::to_string(&descriptor).unwrap();
    assert!(!public.contains("storage.invalid"));
    assert!(!public.contains("Signature"));
}
