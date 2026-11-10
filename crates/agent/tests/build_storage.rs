use chrono::{Duration, Utc};
use ed25519_dalek::SigningKey;
use spitzli_agent::storage::build::{BuildStorageLedger, BuildStoragePhase};
use spitzli_agent_protocol::build_storage::{
    BuildStorageAction, BuildStorageReservation, BuildStorageTarget, SignedBuildStorageReservation,
};

fn request(action: BuildStorageAction, epoch: u64) -> BuildStorageReservation {
    BuildStorageReservation {
        version: 1,
        audience: "webdock-build-storage-v1".into(),
        volume_id: "1234".into(),
        customer_id: "12".into(),
        project_id: "34".into(),
        build_id: "56".into(),
        generation: 1,
        target: BuildStorageTarget {
            cluster_id: "78".into(),
            node: "build-node-1".into(),
            pool_id: "123e4567-e89b-42d3-a456-426614174000".into(),
        },
        volume_bytes: 64 * 1024 * 1024,
        action,
        epoch,
        expires_at: Utc::now() + Duration::seconds(120),
    }
}

#[test]
fn retirement_tombstones_survive_restart_and_defeat_delayed_or_resigned_reserves() {
    let key = SigningKey::from_bytes(&[32; 32]);
    let mut ledger = BuildStorageLedger::default();
    let reserve = request(BuildStorageAction::Reserve, 1);
    ledger.apply_verified(&reserve).unwrap();
    ledger.apply_verified(&reserve).unwrap();
    assert_eq!(ledger.phase(&reserve).unwrap(), BuildStoragePhase::Reserved);
    let retire = request(BuildStorageAction::Retire, 2);
    ledger.apply_verified(&retire).unwrap();
    let bytes = serde_json::to_vec(&ledger).unwrap();
    let mut restarted: BuildStorageLedger = serde_json::from_slice(&bytes).unwrap();
    assert!(restarted.apply_verified(&reserve).is_err());
    assert!(
        restarted
            .apply_verified(&request(BuildStorageAction::Reserve, 3))
            .is_err()
    );
    restarted.apply_verified(&retire).unwrap();
    assert_eq!(
        restarted.phase(&retire).unwrap(),
        BuildStoragePhase::Retired
    );
    // A signed cancellation arriving before allocation must still permanently fence the ID.
    let mut first_retire = BuildStorageLedger::default();
    let signed = SignedBuildStorageReservation::sign(retire.clone(), &key).unwrap();
    signed
        .verify(&key.verifying_key(), &retire.target, Utc::now())
        .unwrap();
    first_retire.apply_verified(&retire).unwrap();
    assert!(first_retire.apply_verified(&reserve).is_err());
}

#[test]
fn journal_rejects_foreign_identity_stale_epochs_conflicting_replays_and_new_inspection() {
    let mut ledger = BuildStorageLedger::default();
    let reserve = request(BuildStorageAction::Reserve, 1);
    assert!(
        ledger
            .apply_verified(&request(BuildStorageAction::Inspect, 1))
            .is_err()
    );
    ledger.apply_verified(&reserve).unwrap();
    let mut conflict = reserve.clone();
    conflict.expires_at += Duration::seconds(1);
    assert!(ledger.apply_verified(&conflict).is_err());
    conflict.epoch = 2;
    ledger.apply_verified(&conflict).unwrap();
    assert!(ledger.apply_verified(&reserve).is_err());
    for field in [
        "customerID",
        "projectID",
        "buildID",
        "generation",
        "volumeBytes",
    ] {
        let mut foreign = serde_json::to_value(&conflict).unwrap();
        foreign[field] = if field == "generation" {
            serde_json::json!(2)
        } else if field == "volumeBytes" {
            serde_json::json!(128 * 1024 * 1024)
        } else {
            serde_json::json!("99")
        };
        foreign["epoch"] = serde_json::json!(3);
        let foreign = serde_json::from_value(foreign).unwrap();
        assert!(ledger.apply_verified(&foreign).is_err(), "{field}");
    }
    assert_eq!(
        ledger.phase(&conflict).unwrap(),
        BuildStoragePhase::Reserved
    );
}

#[test]
fn fresh_inspection_epoch_fences_delayed_reserve_without_allocating_or_reviving_retired_data() {
    let mut ledger = BuildStorageLedger::default();
    ledger
        .apply_verified(&request(BuildStorageAction::Reserve, 1))
        .unwrap();
    ledger
        .apply_verified(&request(BuildStorageAction::Inspect, 2))
        .unwrap();
    assert!(
        ledger
            .apply_verified(&request(BuildStorageAction::Reserve, 1))
            .is_err()
    );
    let bytes = serde_json::to_vec(&ledger).unwrap();
    let mut ledger: BuildStorageLedger = serde_json::from_slice(&bytes).unwrap();
    ledger
        .apply_verified(&request(BuildStorageAction::Retire, 3))
        .unwrap();
    ledger
        .apply_verified(&request(BuildStorageAction::Inspect, 4))
        .unwrap();
    assert_eq!(
        ledger
            .phase(&request(BuildStorageAction::Retire, 3))
            .unwrap(),
        BuildStoragePhase::Retired
    );
    assert!(
        ledger
            .apply_verified(&request(BuildStorageAction::Reserve, 5))
            .is_err()
    );
}

#[test]
#[ignore = "explicit owned privileged storage-helper container only; trusted fixture, no customer code or host mounts"]
fn fixed_build_filesystem_enforces_enospc_checks_mount_identity_and_fences_retirement() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    use spitzli_agent::storage::build::BuildStorageTrust;
    use spitzli_agent::storage::{StorageGuard, StorageKind, StoragePacket, StoragePool};
    use std::{
        os::unix::fs::{MetadataExt, PermissionsExt},
        process::Command,
        sync::{Arc, atomic::AtomicBool},
    };
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_STORAGE_TEST").as_deref(),
        Ok("yes")
    );
    assert!(std::path::Path::new("/.dockerenv").is_file());
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let base = tempfile::Builder::new()
        .prefix("webdock-build-filesystem-owned-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.path().join("volumes");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = base.path().join("config.json");
    std::fs::write(&config, serde_json::to_vec(&serde_json::json!({"root":root,"node":"build-node-1","clusterID":"78","poolID":"123e4567-e89b-42d3-a456-426614174000","capacityBytes":384*1024*1024,"reserveBytes":64*1024*1024})).unwrap()).unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    StoragePool::initialize(&config).unwrap();
    let key = SigningKey::from_bytes(&[32; 32]);
    let reserve = request(BuildStorageAction::Reserve, 1);
    let trust = BuildStorageTrust {
        version: 1,
        target: reserve.target.clone(),
        public_key: key
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    };
    let guard = StorageGuard::new(
        Utc::now() + Duration::seconds(120),
        Arc::new(AtomicBool::new(false)),
    );
    let image = root.join("1234/volume.ext4");
    let mount = root.join("1234/mount");
    let result = std::panic::catch_unwind(|| {
        let mut pool = StoragePool::open_kind(&config, StorageKind::Builds).unwrap();
        let mut foreign = reserve.clone();
        foreign.customer_id = "999".into();
        let mut tampered = SignedBuildStorageReservation::sign(reserve.clone(), &key).unwrap();
        tampered.reservation = foreign;
        assert!(pool.apply_build_storage(&tampered, &trust, &guard).is_err());
        assert!(!image.exists(), "invalid authority must not allocate");
        let packet = StoragePacket::new(
            "1234",
            "34",
            "78",
            1,
            64 * 1024 * 1024,
            (Utc::now() + Duration::seconds(120)).timestamp() as u64,
        )
        .unwrap();
        assert!(pool.ensure(&packet, &guard).is_err());
        let filesystem = pool
            .apply_build_storage(
                &SignedBuildStorageReservation::sign(reserve.clone(), &key).unwrap(),
                &trust,
                &guard,
            )
            .unwrap()
            .filesystem
            .unwrap();
        let data = filesystem.data_path;
        assert_eq!(std::fs::metadata(&data).unwrap().uid(), 1000);
        assert_eq!(std::fs::metadata(&data).unwrap().gid(), 1000);
        let script = data.join("owned-script");
        std::fs::write(&script, b"#!/bin/sh\nprintf owned-build-exec").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let exec = Command::new("/usr/bin/setpriv")
            .args([
                "--reuid=1000",
                "--regid=1000",
                "--clear-groups",
                "./owned-script",
            ])
            .current_dir(&data)
            .output()
            .unwrap();
        assert!(
            exec.status.success(),
            "build-only executable filesystem required"
        );
        assert_eq!(exec.stdout, b"owned-build-exec");
        let fill = Command::new("/usr/bin/setpriv")
            .args([
                "--reuid=1000",
                "--regid=1000",
                "--clear-groups",
                "/usr/bin/dd",
                "if=/dev/zero",
                "of=fill",
                "bs=1048576",
                "count=128",
                "status=none",
            ])
            .env("LC_ALL", "C")
            .current_dir(&data)
            .output()
            .unwrap();
        assert!(!fill.status.success());
        assert!(String::from_utf8_lossy(&fill.stderr).contains("No space left on device"));
        let written = std::fs::metadata(data.join("fill")).unwrap().len();
        assert!(written > 0 && written < 64 * 1024 * 1024);
        std::fs::remove_file(data.join("fill")).unwrap();
        let inspect =
            SignedBuildStorageReservation::sign(request(BuildStorageAction::Inspect, 1), &key)
                .unwrap();
        pool.apply_build_storage(&inspect, &trust, &guard).unwrap();
        let original = image.with_extension("original");
        std::fs::rename(&image, &original).unwrap();
        std::fs::copy(&original, &image).unwrap();
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            pool.apply_build_storage(&inspect, &trust, &guard).is_err(),
            "same UUID/path but different backing inode must be denied"
        );
        std::fs::remove_file(&image).unwrap();
        std::fs::rename(&original, &image).unwrap();
        assert!(
            Command::new("/usr/bin/umount")
                .arg(&mount)
                .status()
                .unwrap()
                .success()
        );
        assert!(pool.apply_build_storage(&inspect, &trust, &guard).is_err());
        assert!(
            !data.exists(),
            "inspection must not create an unbounded fallback directory"
        );
        pool.apply_build_storage(
            &SignedBuildStorageReservation::sign(request(BuildStorageAction::Reserve, 2), &key)
                .unwrap(),
            &trust,
            &guard,
        )
        .unwrap();
        let retired = pool
            .apply_build_storage(
                &SignedBuildStorageReservation::sign(request(BuildStorageAction::Retire, 3), &key)
                    .unwrap(),
                &trust,
                &guard,
            )
            .unwrap();
        assert_eq!(retired.phase, BuildStoragePhase::Retired);
        assert!(retired.filesystem.is_none());
        assert!(
            image.exists(),
            "retirement without exact consumer absence must retain physical storage"
        );
        assert!(pool.purge(&packet, &guard).is_err());
        drop(pool);
        let mut restarted = StoragePool::open_kind(&config, StorageKind::Builds).unwrap();
        assert!(
            restarted
                .apply_build_storage(
                    &SignedBuildStorageReservation::sign(
                        request(BuildStorageAction::Reserve, 4),
                        &key
                    )
                    .unwrap(),
                    &trust,
                    &guard
                )
                .is_err()
        );
        eprintln!(
            "owned build filesystem: uid/gid=1000; executable=nodev,nosuid; bytes_before_enospc={written}; inode/mount-loss checks passed; restart retirement fenced; no deletion authority"
        );
    });
    // Only detach this exact fixture's image; no global loop cleanup or host mounts.
    let _ = Command::new("/usr/bin/umount").arg(&mount).output();
    let loops = Command::new("/usr/sbin/losetup")
        .args(["--json", "--list", "--associated"])
        .arg(&image)
        .output()
        .unwrap();
    assert!(loops.status.success());
    let loops: serde_json::Value = serde_json::from_slice(&loops.stdout).unwrap();
    for entry in loops["loopdevices"].as_array().unwrap() {
        assert_eq!(entry["back-file"], image.to_str().unwrap());
        let device = entry["name"].as_str().unwrap();
        assert!(
            device
                .strip_prefix("/dev/loop")
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        );
        assert!(
            Command::new("/usr/sbin/losetup")
                .args(["--detach", device])
                .status()
                .unwrap()
                .success()
        );
    }
    result.unwrap();
}
