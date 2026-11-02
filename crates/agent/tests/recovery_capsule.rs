//! Cryptographic/local replay tests only; not a substitute for real Core policy admission.
#![cfg(feature = "docker")]
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use webdock_agent::recovery::capsule::*;
use webdock_agent_protocol::recovery::RecoveryProfile;
fn write(path: &Path, value: &[u8]) {
    std::fs::write(path, value).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn key(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    assert!(
        Command::new("/usr/bin/ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    path
}
fn document(root: &Path) -> (ColdRecoveryCapsule, PathBuf, PathBuf, PathBuf) {
    let issuer = key(root, "issuer");
    let operator = key(root, "operator");
    let now = chrono::Utc::now().timestamp();
    let core_signers = root.join("core-signers");
    let operator_signers = root.join("operator-signers");
    write(
        &core_signers,
        format!(
            "webdock-core-recovery {}",
            std::fs::read_to_string(issuer.with_extension("pub")).unwrap()
        )
        .as_bytes(),
    );
    write(
        &operator_signers,
        format!(
            "unit-operator {}",
            std::fs::read_to_string(operator.with_extension("pub")).unwrap()
        )
        .as_bytes(),
    );
    let set = AdmittedRecoverySet {
        version: 1,
        set_id: "111".into(),
        operation_id: "112".into(),
        policy_id: "113".into(),
        policy_revision: 1,
        profile: RecoveryProfile::MailInstallationV1,
        profile_id: "mail-fixture".into(),
        approved_by: "unit-operator".into(),
        epoch: uuid::Uuid::new_v4().to_string(),
        capacity_bytes: 67108864,
        proof: serde_json::json!({"version":1,"objects":[{"opaqueReceipt":"unit"}]}),
        completed_at: now - 1,
    };
    let admitted = seal_admitted_set(&set, &issuer, root).unwrap();
    (
        ColdRecoveryCapsule {
            version: 1,
            admitted_set: admitted,
            actor_id: "unit-operator".into(),
            target_profile_id: "target".into(),
            target_identity_sha256: "a".repeat(64),
            target_nonce: uuid::Uuid::new_v4().to_string(),
            capacity_bytes: 67108864,
            issued_at: now,
            expires_at: now + 60,
            quarantine_only: true,
        },
        core_signers,
        operator_signers,
        operator,
    )
}
#[test]
fn standard_signatures_require_both_admitted_set_and_operator_and_exact_target_capacity_deadline() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let (capsule, core, operator, key) = document(root.path());
    let now = chrono::Utc::now().timestamp();
    let doc = seal_operator_capsule(&capsule, &key, root.path()).unwrap();
    assert!(
        verify_capsule(
            &doc,
            &core,
            &operator,
            "target",
            &"a".repeat(64),
            67108864,
            root.path(),
            now
        )
        .is_ok()
    );
    for (target, hash, capacity, at) in [
        ("other", "a".repeat(64), 67108864, now),
        ("target", "b".repeat(64), 67108864, now),
        ("target", "a".repeat(64), 134217728, now),
        ("target", "a".repeat(64), 67108864, now + 60),
    ] {
        assert!(
            verify_capsule(
                &doc,
                &core,
                &operator,
                target,
                &hash,
                capacity,
                root.path(),
                at
            )
            .is_err()
        );
    }
    let mut mutated = capsule.clone();
    mutated.quarantine_only = false;
    let doc = seal_operator_capsule(&mutated, &key, root.path()).unwrap();
    assert!(
        verify_capsule(
            &doc,
            &core,
            &operator,
            "target",
            &"a".repeat(64),
            67108864,
            root.path(),
            now
        )
        .is_err()
    );
    let mut mutated = capsule;
    mutated.admitted_set.payload.push('a');
    let doc = seal_operator_capsule(&mutated, &key, root.path()).unwrap();
    assert!(
        verify_capsule(
            &doc,
            &core,
            &operator,
            "target",
            &"a".repeat(64),
            67108864,
            root.path(),
            now
        )
        .is_err()
    );
}
#[test]
fn local_replay_custody_survives_crash_and_never_advertises_global_single_use() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let (capsule, core, operator, key) = document(root.path());
    let doc = seal_operator_capsule(&capsule, &key, root.path()).unwrap();
    let verified = verify_capsule(
        &doc,
        &core,
        &operator,
        "target",
        &"a".repeat(64),
        67108864,
        root.path(),
        chrono::Utc::now().timestamp(),
    )
    .unwrap();
    let journal = root.path().join("journal");
    std::fs::create_dir(&journal).unwrap();
    std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o700)).unwrap();
    let first = ReplayJournal::open(&journal).unwrap();
    first.begin(&verified).unwrap();
    assert!(ReplayJournal::open(&journal).is_err());
    drop(first);
    let resumed = ReplayJournal::open(&journal).unwrap();
    assert!(resumed.begin(&verified).is_err());
    resumed
        .finish(&verified, &serde_json::json!({"dispatch":"quarantined"}))
        .unwrap();
    assert!(
        resumed
            .finish(&verified, &serde_json::json!({"dispatch":"active"}))
            .is_err()
    );
}
