use chrono::{Duration, Utc};
use ed25519_dalek::SigningKey;
use webdock_agent_protocol::build_storage::{
    BuildStorageAction, BuildStorageReservation, BuildStorageTarget, BuildVolumeHandle,
    SignedBuildStorageReservation,
};

fn reservation() -> BuildStorageReservation {
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
        action: BuildStorageAction::Reserve,
        epoch: 1,
        expires_at: Utc::now() + Duration::seconds(120),
    }
}

#[test]
fn volume_handle_binds_exact_attempt_resources_and_rejects_replacement_uids() {
    let request = reservation();
    let handle = BuildVolumeHandle::new(
        request.clone(),
        "webdock-build-qualification",
        "pv-uid-1",
        "pvc-uid-1",
        "123e4567-e89b-42d3-a456-426614174001",
    )
    .unwrap();
    handle.validate_for(&request).unwrap();
    assert_eq!(handle.volume_name(), handle.claim_name());
    assert_eq!(handle.volume_name(), "wd-bv-56-1-1234");
    assert_ne!(handle.job_name(), handle.volume_name());
    let mut other = request.clone();
    other.generation = 2;
    assert!(handle.validate_for(&other).is_err());
    let mut replaced = handle.clone();
    replaced.pvc_uid = "pvc-uid-2".into();
    assert_ne!(
        handle.fingerprint().unwrap(),
        replaced.fingerprint().unwrap()
    );
    let mut foreign = handle.clone();
    foreign.pvc_name = "another-claim".into();
    assert!(foreign.validate_for(&request).is_err());
    let mut foreign = handle.clone();
    foreign.pv_name = "another-volume".into();
    assert!(foreign.validate_for(&request).is_err());
    // Expired reserve authority does not change the identity of a retained volume.
    let mut retained = handle.clone();
    retained.reservation.expires_at = chrono::DateTime::UNIX_EPOCH;
    retained.validate_for(&request).unwrap();
    let mut invalid = handle.clone();
    invalid.namespace = "../kube-system".into();
    assert!(invalid.validate_for(&request).is_err());
}

#[test]
fn signature_binds_every_scope_action_budget_and_lifetime_before_storage_io() {
    let key = SigningKey::from_bytes(&[23; 32]);
    let request = reservation();
    let signed = SignedBuildStorageReservation::sign(request.clone(), &key).unwrap();
    signed
        .verify(&key.verifying_key(), &request.target, Utc::now())
        .unwrap();
    assert!(
        signed
            .verify(
                &SigningKey::from_bytes(&[24; 32]).verifying_key(),
                &request.target,
                Utc::now()
            )
            .is_err()
    );
    for (field, value) in [
        ("customerID", serde_json::json!("99")),
        ("projectID", serde_json::json!("99")),
        ("buildID", serde_json::json!("99")),
        ("volumeID", serde_json::json!("99")),
        ("generation", serde_json::json!(2)),
        ("volumeBytes", serde_json::json!(128 * 1024 * 1024)),
        ("epoch", serde_json::json!(2)),
        ("action", serde_json::json!("retire")),
        ("audience", serde_json::json!("other-service")),
        (
            "expiresAt",
            serde_json::json!(Utc::now() + Duration::seconds(180)),
        ),
    ] {
        let mut tampered = serde_json::to_value(&signed).unwrap();
        tampered["reservation"][field] = value;
        let tampered: SignedBuildStorageReservation = serde_json::from_value(tampered).unwrap();
        assert!(
            tampered
                .verify(&key.verifying_key(), &request.target, Utc::now())
                .is_err(),
            "{field}"
        );
    }
    let mut foreign = request.target.clone();
    foreign.node = "build-node-2".into();
    assert!(
        signed
            .verify(&key.verifying_key(), &foreign, Utc::now())
            .is_err()
    );
    assert!(
        signed
            .verify(&key.verifying_key(), &request.target, request.expires_at)
            .is_err()
    );
}

#[test]
fn allocation_identity_survives_renewal_but_not_customer_attempt_or_capacity_changes() {
    let request = reservation();
    let identity = request.allocation_fingerprint().unwrap();
    let mut renewed = request.clone();
    renewed.epoch += 1;
    renewed.expires_at += Duration::seconds(30);
    renewed.action = BuildStorageAction::Inspect;
    assert_eq!(identity, renewed.allocation_fingerprint().unwrap());
    for field in [
        "customerID",
        "projectID",
        "buildID",
        "volumeID",
        "generation",
        "volumeBytes",
    ] {
        let mut changed = serde_json::to_value(&request).unwrap();
        changed[field] = if field == "volumeBytes" {
            serde_json::json!(128 * 1024 * 1024)
        } else if field == "generation" {
            serde_json::json!(2)
        } else {
            serde_json::json!("99")
        };
        let changed: BuildStorageReservation = serde_json::from_value(changed).unwrap();
        assert_ne!(
            identity,
            changed.allocation_fingerprint().unwrap(),
            "{field}"
        );
    }
    for invalid in ["../123", "0", "012", "9223372036854775808"] {
        let mut changed = request.clone();
        changed.volume_id = invalid.into();
        assert!(changed.allocation_fingerprint().is_err());
    }
    let mut unknown = serde_json::to_value(&request).unwrap();
    unknown["hostPath"] = "/".into();
    assert!(serde_json::from_value::<BuildStorageReservation>(unknown).is_err());
}

#[test]
fn volume_reply_is_bound_to_request_uid_operation_and_tenant_without_private_error_payloads() {
    use webdock_agent_protocol::build_storage::{
        BuildVolumePhase, BuildVolumeReply, BuildVolumeResult,
    };
    let request = reservation();
    let handle = BuildVolumeHandle::new(
        request.clone(),
        "webdock-build-qualification",
        "pv-uid-1",
        "pvc-uid-1",
        "123e4567-e89b-42d3-a456-426614174001",
    )
    .unwrap();
    let mut reply = BuildVolumeReply {
        version: 1,
        request_uid: "request-uid-1".into(),
        request_fingerprint: request.fingerprint().unwrap(),
        result: Some(BuildVolumeResult {
            phase: BuildVolumePhase::Ready,
            handle: Some(handle),
        }),
        failure_code: None,
    };
    reply
        .validate_for(&request, "request-uid-1", "webdock-build-qualification")
        .unwrap();
    assert!(
        reply
            .validate_for(&request, "replacement", "webdock-build-qualification")
            .is_err()
    );
    let mut changed = request.clone();
    changed.action = BuildStorageAction::Retire;
    assert!(
        reply
            .validate_for(&changed, "request-uid-1", "webdock-build-qualification")
            .is_err()
    );
    reply.result.as_mut().unwrap().handle = None;
    assert!(
        reply
            .validate_for(&request, "request-uid-1", "webdock-build-qualification")
            .is_err()
    );
    let mut raw = serde_json::to_value(reply).unwrap();
    raw["failureCode"] = serde_json::json!("raw credential-bearing provider error");
    assert!(serde_json::from_value::<BuildVolumeReply>(raw).is_err());
    let name = request.queue_name().unwrap();
    assert!(name.len() <= 63);
    assert_ne!(name, changed.queue_name().unwrap());
}
