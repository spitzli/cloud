use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};
use webdock_mail_runtime::payload::PrivatePayloadStore;

#[test]
fn retained_volume_is_private_immutable_and_bound_to_key_customer_and_reference() {
    let dir = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let key = STANDARD.encode([19u8; 32]);
    let store = PrivatePayloadStore::open(dir.path(), &key).unwrap();
    let body = b"Private mail body and attachment";
    let saved = store.put("123", body, None).unwrap();
    assert_eq!(saved.bytes, body.len());
    assert_eq!(store.get("123", &saved.reference).unwrap(), body);
    assert!(
        !fs::read(dir.path().join(&saved.reference))
            .unwrap()
            .windows(body.len())
            .any(|w| w == body)
    );
    assert!(
        store
            .put("123", b"replacement", Some(&saved.reference))
            .is_err()
    );
    assert!(store.get("124", &saved.reference).is_err());
    assert!(store.get("123", "../../etc/passwd").is_err());
    assert!(PrivatePayloadStore::open(dir.path(), &STANDARD.encode([20u8; 32])).is_err());
    assert_eq!(store.get("123", &saved.reference).unwrap(), body);
    let link = uuid::Uuid::new_v4().to_string();
    symlink(&saved.reference, dir.path().join(&link)).unwrap();
    assert!(store.get("123", &link).is_err());
    fs::set_permissions(
        dir.path().join(&saved.reference),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(store.get("123", &saved.reference).is_err());
    fs::set_permissions(
        dir.path().join(&saved.reference),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    store.remove(&saved.reference).unwrap();
    store.remove(&saved.reference).unwrap();
    assert!(store.get("123", &saved.reference).is_err());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(PrivatePayloadStore::open(dir.path(), &key).is_err());
}

#[test]
fn reads_preexisting_node_payload_without_reencrypting() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("node-compatibility.json")).unwrap();
    let dir = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = PrivatePayloadStore::open(dir.path(), fixture["key"].as_str().unwrap()).unwrap();
    let reference = "12345678-1234-4123-8123-123456789012";
    let path = dir.path().join(reference);
    fs::write(&path, fixture["sealed"].as_str().unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        store.get("123", reference).unwrap(),
        b"Private mail body and attachment"
    );
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        fixture["sealed"].as_str().unwrap()
    );
}
