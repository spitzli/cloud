//! Compiles the proposed unlinked module against actual native storage types.
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use webdock_agent::storage::{StorageGuard, StoragePacket, VolumeRecord, recovery};

fn packet(app: &str) -> StoragePacket {
    StoragePacket::new(
        app,
        "123",
        "789",
        1,
        67108864,
        webdock_supervisor::timestamp().unwrap() + 60,
    )
    .unwrap()
}
fn record(app: &str, uuid: &str) -> VolumeRecord {
    serde_json::from_value(serde_json::json!({"appID":app,"projectID":"123","clusterID":"789","bytes":67108864,"uuid":uuid,"state":"ready"})).unwrap()
}
#[test]
fn native_fresh_volume_retains_its_identity_and_checks_actual_owner_packet_and_peak() {
    let source = record("456", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let target = record("457", "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let guard = StorageGuard::new(
        chrono::Utc::now() + chrono::Duration::seconds(60),
        Arc::new(AtomicBool::new(false)),
    );
    recovery::validate_fresh_target(
        &source,
        &packet("456"),
        &target,
        &packet("457"),
        201326592,
        &guard,
    )
    .unwrap();
    assert_eq!(target.uuid, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert!(
        recovery::validate_fresh_target(
            &source,
            &packet("456"),
            &source,
            &packet("456"),
            201326592,
            &guard
        )
        .is_err()
    );
    assert!(
        recovery::validate_fresh_target(
            &source,
            &packet("456"),
            &target,
            &packet("458"),
            201326592,
            &guard
        )
        .is_err()
    );
    assert!(
        recovery::validate_fresh_target(
            &source,
            &packet("456"),
            &target,
            &packet("457"),
            67108864,
            &guard
        )
        .is_err()
    );
}
#[test]
fn only_concrete_ntfy_data_can_be_copied_never_source_pool_identities_or_keys() {
    assert_eq!(recovery::NTFY_PROFILE, "ntfy-sqlite-offline-v1");
    for name in ["cache.db", "auth.db", "attachments/abc123_-ABC"] {
        recovery::ntfy_data_path(Path::new(name)).unwrap();
    }
    for name in [
        "../cache.db",
        "/cache.db",
        "state.json",
        "volume.ext4",
        "service-kind.json",
        ".webdock-restored.json",
        "attachments/../state.json",
        "secret.key",
        "attachments/nested/a",
    ] {
        assert!(recovery::ntfy_data_path(Path::new(name)).is_err());
    }
}
#[test]
fn private_staging_hash_partial_transfer_and_revoked_guard_are_checked() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("part");
    std::fs::write(&file, b"abc").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let revoked = Arc::new(AtomicBool::new(false));
    let guard = StorageGuard::new(
        chrono::Utc::now() + chrono::Duration::seconds(60),
        revoked.clone(),
    );
    let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    recovery::verify_data_receipt(&file, 3, hash, &guard).unwrap();
    assert!(recovery::verify_data_receipt(&file, 4, hash, &guard).is_err());
    assert!(recovery::verify_data_receipt(&file, 3, &"a".repeat(64), &guard).is_err());
    revoked.store(true, Ordering::SeqCst);
    assert!(recovery::verify_data_receipt(&file, 3, hash, &guard).is_err());
}
