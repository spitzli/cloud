#![cfg(feature = "kubernetes")]
use chrono::{Duration, Utc};
use serde_json::json;
use webdock_agent::storage::build::BuildFilesystem;
use webdock_agent::storage::build_kubernetes::{
    client_config, consumers_absent, verify_volume_objects, volume_manifests,
};
use webdock_agent_protocol::build_storage::{
    BuildStorageAction, BuildStorageReservation, BuildStorageTarget, BuildVolumeHandle,
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
        volume_bytes: 67108864,
        action: BuildStorageAction::Reserve,
        epoch: 1,
        expires_at: Utc::now() + Duration::seconds(120),
    }
}

#[tokio::test]
async fn node_connection_rejects_ambient_auth_files_exec_proxy_and_foreign_cluster_before_io() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let ca = include_bytes!("../../build/tests/fixtures/kubernetes-worker-ca.crt");
    let hash = format!("{:x}", Sha256::digest(ca));
    let config = json!({"apiVersion":"v1","kind":"Config","contexts":[{"name":"owned","context":{"cluster":"build","user":"node","namespace":"webdock-build-qualification"}}],
        "clusters":[{"name":"build","cluster":{"server":"https://10.83.0.1:6443","certificate-authority-data":base64::engine::general_purpose::STANDARD.encode(ca)}}],
        "users":[{"name":"node","user":{"token":"owned-test-only"}}]});
    let bytes = serde_json::to_vec(&config).unwrap();
    client_config(
        &bytes,
        "owned",
        "https://10.83.0.1:6443",
        &hash,
        "webdock-build-qualification",
    )
    .await
    .unwrap();
    for (pointer, value) in [
        ("/users/0/user", json!({"exec":{"command":"/not/executed"}})),
        ("/users/0/user", json!({"tokenFile":"/not/read"})),
        (
            "/users/0/user",
            json!({"client-certificate":"/not/read","client-key":"/not/read"}),
        ),
        (
            "/clusters/0/cluster",
            json!({"server":"https://10.83.0.1:6443","proxy-url":"http://proxy.invalid","certificate-authority-data":"bad"}),
        ),
        (
            "/clusters/0/cluster/server",
            json!("https://run.invalid:6443"),
        ),
        ("/contexts/0/context/namespace", json!("kube-system")),
    ] {
        let mut other = config.clone();
        *other.pointer_mut(pointer).unwrap() = value;
        assert!(
            client_config(
                &serde_json::to_vec(&other).unwrap(),
                "owned",
                "https://10.83.0.1:6443",
                &hash,
                "webdock-build-qualification"
            )
            .await
            .is_err(),
            "{pointer}"
        );
    }
}
fn fixture() -> (
    BuildStorageReservation,
    BuildVolumeHandle,
    serde_json::Value,
    serde_json::Value,
) {
    let request = reservation();
    let fs = BuildFilesystem {
        allocation_fingerprint: request.allocation_fingerprint().unwrap(),
        volume_bytes: request.volume_bytes,
        filesystem_uuid: "123e4567-e89b-42d3-a456-426614174001".into(),
        data_path: "/var/lib/owned-build-storage/1234/mount/data".into(),
    };
    let (mut pv, mut pvc) =
        volume_manifests(&request, &fs, "webdock-build-qualification", "pvc-uid-1").unwrap();
    pv["metadata"]["uid"] = json!("pv-uid-1");
    pvc["metadata"]["uid"] = json!("pvc-uid-1");
    pv["metadata"]["resourceVersion"] = json!("11");
    pvc["metadata"]["resourceVersion"] = json!("12");
    pvc["status"] = json!({"phase":"Bound"});
    let handle = BuildVolumeHandle::new(
        request.clone(),
        "webdock-build-qualification",
        "pv-uid-1",
        "pvc-uid-1",
        &fs.filesystem_uuid,
    )
    .unwrap();
    (request, handle, pv, pvc)
}
#[test]
fn volume_binding_checks_uid_capacity_source_claim_and_node_without_adopting_foreign_objects() {
    let (_, handle, pv, pvc) = fixture();
    let path = std::path::Path::new("/var/lib/owned-build-storage/1234/mount/data");
    verify_volume_objects(&handle, path, &pv, &pvc).unwrap();
    for (kind, pointer, value) in [
        ("pv", "/metadata/uid", json!("replacement")),
        ("pvc", "/metadata/uid", json!("replacement")),
        ("pv", "/spec/local/path", json!("/etc")),
        ("pv", "/spec/claimRef/uid", json!("replacement")),
        (
            "pv",
            "/spec/nodeAffinity/required/nodeSelectorTerms/0/matchExpressions/0/values/0",
            json!("other-node"),
        ),
        ("pvc", "/spec/resources/requests/storage", json!("128Mi")),
        ("pvc", "/spec/volumeName", json!("other-volume")),
    ] {
        let (mut p, mut c) = (pv.clone(), pvc.clone());
        *if kind == "pv" {
            p.pointer_mut(pointer)
        } else {
            c.pointer_mut(pointer)
        }
        .unwrap() = value;
        assert!(
            verify_volume_objects(&handle, path, &p, &c).is_err(),
            "{pointer}"
        );
    }
    let mut foreign = pv.clone();
    foreign["spec"]["hostPath"] = json!({"path":"/"});
    assert!(verify_volume_objects(&handle, path, &foreign, &pvc).is_err());
    let mut equivalent = pv.clone();
    equivalent["spec"]["capacity"]["storage"] = json!("64Mi");
    verify_volume_objects(&handle, path, &equivalent, &pvc).unwrap();
}

#[test]
fn storage_identity_metadata_fits_kubernetes_label_limits() {
    let (_, _, pv, pvc) = fixture();
    for object in [pv, pvc] {
        for value in object["metadata"]["labels"].as_object().unwrap().values() {
            assert!(
                value.as_str().unwrap().len() <= 63,
                "Kubernetes label value too long"
            );
        }
    }
}

#[test]
fn storage_queue_accepts_only_immutable_exact_scope_requests_and_bounded_data() {
    use webdock_agent::storage::build_queue::parse_request;
    use webdock_agent_protocol::build_storage::SignedBuildStorageReservation;
    let request = reservation();
    let signed = SignedBuildStorageReservation::sign(
        request.clone(),
        &ed25519_dalek::SigningKey::from_bytes(&[25; 32]),
    )
    .unwrap();
    let value = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":request.queue_name().unwrap(),"namespace":"webdock-build-storage-requests","uid":"request-uid-1","resourceVersion":"1",
        "labels":{"webdock.dev/component":"build-volume-request","webdock.dev/volume-id":request.volume_id,"webdock.dev/generation":"1","webdock.dev/epoch":"1"},
        "annotations":{"webdock.dev/request-sha256":request.fingerprint().unwrap()}},"immutable":true,"data":{"request.json":serde_json::to_string(&signed).unwrap()}});
    parse_request(&value, "webdock-build-storage-requests").unwrap();
    for (pointer, other) in [
        ("/immutable", json!(false)),
        ("/metadata/namespace", json!("kube-system")),
        ("/metadata/uid", json!("")),
        ("/metadata/name", json!("other")),
        ("/metadata/labels/webdock.dev~1volume-id", json!("999")),
        (
            "/metadata/annotations/webdock.dev~1request-sha256",
            json!("a".repeat(64)),
        ),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = other;
        assert!(
            parse_request(&changed, "webdock-build-storage-requests").is_err(),
            "{pointer}"
        );
    }
    let mut binary = value.clone();
    binary["binaryData"] = json!({"other":"YQ=="});
    assert!(parse_request(&binary, "webdock-build-storage-requests").is_err());
    let mut extra = value;
    extra["data"]["private"] = "not accepted".into();
    assert!(parse_request(&extra, "webdock-build-storage-requests").is_err());
}
#[test]
fn resource_absence_requires_complete_inventory_and_blocks_every_pvc_consumer_even_without_labels()
{
    let (_, handle, _, _) = fixture();
    let empty = json!({"metadata":{},"items":[]});
    assert!(consumers_absent(&handle, None, &empty).unwrap());
    let mut paged = empty.clone();
    paged["metadata"]["continue"] = json!("next-page");
    assert!(consumers_absent(&handle, None, &paged).is_err());
    let unrelated = json!({"metadata":{},"items":[{"metadata":{"name":"other","namespace":handle.namespace},"spec":{"volumes":[{"name":"other","emptyDir":{}}]}}]});
    assert!(consumers_absent(&handle, None, &unrelated).unwrap());
    let foreign = json!({"metadata":{},"items":[{"metadata":{"name":"foreign","namespace":handle.namespace},"spec":{"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":handle.claim_name()}}]}}]});
    assert!(!consumers_absent(&handle, None, &foreign).unwrap());
    let pending = json!({"metadata":{},"items":[{"metadata":{"name":"pending","namespace":handle.namespace,"labels":{"webdock.dev/build-id":"56","webdock.dev/generation":"1"}},"spec":{"volumes":[]},"status":{"phase":"Pending"}}]});
    assert!(!consumers_absent(&handle, None, &pending).unwrap());
    assert!(!consumers_absent(&handle,Some(&json!({"metadata":{"name":handle.job_name(),"namespace":handle.namespace,"uid":"job-1"}})),&empty).unwrap());
}
