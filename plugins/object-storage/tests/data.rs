use serde_json::json;
use webdock_object_storage_plugin::data::DataRequest;
use webdock_plugin_protocol::storage::StorageDirection;
fn input() -> serde_json::Value {
    json!({"resource_id":uuid::Uuid::new_v4(),"key":"reports/proof.txt","exact_bytes":3,"expected_sha256":"a".repeat(64),"filename":"proof.txt","content_type":"text/plain","expires_at":1120})
}
#[test]
fn typed_input_binds_method_version_checksum_length_expiry_and_safe_attachment() {
    let v = input();
    let request: DataRequest = serde_json::from_value(v.clone()).unwrap();
    assert!(request.validate(StorageDirection::Upload, 1000).is_ok());
    assert!(request.validate(StorageDirection::Download, 1000).is_err());
    for (key, value) in [
        ("key", json!("../other")),
        ("filename", json!("bad\r\nname")),
        ("content_type", json!("text/html")),
        ("expected_sha256", json!("A".repeat(64))),
        ("exact_bytes", json!(536870913_u64)),
        ("expires_at", json!(1000)),
        ("expires_at", json!(1301)),
    ] {
        let mut bad = v.clone();
        bad[key] = value;
        let request: DataRequest = serde_json::from_value(bad).unwrap();
        assert!(
            request.validate(StorageDirection::Upload, 1000).is_err(),
            "{key}"
        );
    }
    let mut extra = v.clone();
    extra["method"] = json!("DELETE");
    assert!(serde_json::from_value::<DataRequest>(extra).is_err());
    let mut download = v;
    download["version_id"] = json!("version-1");
    download["if_match"] = json!("\"aabbcc\"");
    let request: DataRequest = serde_json::from_value(download.clone()).unwrap();
    assert!(request.validate(StorageDirection::Download, 1000).is_ok());
    assert!(request.validate(StorageDirection::Upload, 1000).is_err());
    let mut unknown = download.clone();
    unknown.as_object_mut().unwrap().remove("expected_sha256");
    let request: DataRequest = serde_json::from_value(unknown).unwrap();
    assert!(request.validate(StorageDirection::Download, 1000).is_ok());
    assert!(request.validate(StorageDirection::Upload, 1000).is_err());
    for value in [json!(""), json!("bad\r\nversion"), json!("x".repeat(513))] {
        download["version_id"] = value;
        assert!(
            serde_json::from_value::<DataRequest>(download.clone())
                .unwrap()
                .validate(StorageDirection::Download, 1000)
                .is_err()
        );
    }
}
