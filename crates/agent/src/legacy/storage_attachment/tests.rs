use super::*;
use serde_json::{Value, json};

fn fixture() -> (
    Command,
    crate::storage::StorageConfig,
    Deployment,
    PersistentVolume,
    PersistentVolumeClaim,
    Vec<Node>,
) {
    let allowance = ResourceAllowance {
        apps: 2,
        cpu_millicores: 2000,
        memory_bytes: 2_147_483_648,
        volume_bytes: 2_147_483_648,
        ephemeral_bytes: 2_147_483_648,
        replicas_per_app: 1,
        concurrent_deployments: 1,
    };
    let spec: RuntimeSpec = serde_json::from_value(json!({"version":1,"installation":"test","projectId":"123","resourceId":"456","revision":5,"image":format!("registry.example/app@sha256:{}","a".repeat(64)),"arguments":[],"cpuMillicores":100,"memoryBytes":134217728,"ephemeralBytes":134217728,"replicas":1,"port":8080,"healthPath":"/healthz","volume":{"mountPath":"/data","sizeBytes":1073741824},"hostingBinding":{"clusterId":"789","quota":allowance,"finalQuota":allowance}})).unwrap();
    let command = Command {
        operation_id: "retained-update".into(),
        generation: 1,
        lease_until: timestamp().unwrap() + 300,
        authorization: "fixture".into(),
        action: Action::Apply,
        spec,
        expected_runtime_id: Some("deployment-uid".into()),
        material: None,
    };
    let config = crate::storage::StorageConfig {
        root: "/var/lib/webdock-storage".into(),
        node: "node-02".into(),
        cluster_id: webdock_core::id::RecordId::parse("789").unwrap(),
        pool_id: "11111111-1111-4111-8111-111111111111".into(),
        capacity_bytes: 2_147_483_648,
        reserve_bytes: 67_108_864,
    };
    let metadata = |name: &str, uid: &str| json!({"name":name,"namespace":"wd-123","uid":uid,"resourceVersion":"10","labels":owner_labels(&command.spec,true).unwrap(),"annotations":{"webdock.dev/revision":"4"}});
    let deployment = serde_json::from_value(json!({"metadata":metadata("app-456","deployment-uid"),"spec":{"replicas":1,"selector":{"matchLabels":{"webdock.dev/app-id":"456"}},"template":{"metadata":{"labels":owner_labels(&command.spec,true).unwrap(),"annotations":{"webdock.dev/revision":"4"}},"spec":{"containers":[{"name":"app","image":command.spec.image,"env":[{"name":"PRESERVED","valueFrom":{"secretKeyRef":{"name":"app-456-env-4","key":"PRESERVED"}}}],"volumeMounts":[{"name":"data","mountPath":"/data"}]}],"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"app-456-data"}}]}}}})).unwrap();
    let mut pv_metadata = metadata("wd-456-data", "pv-uid");
    pv_metadata.as_object_mut().unwrap().remove("namespace");
    let pv = serde_json::from_value(json!({
        "metadata":pv_metadata,
        "spec":{
            "accessModes":["ReadWriteOnce"], "volumeMode":"Filesystem",
            "capacity":{"storage":"1Gi"}, "persistentVolumeReclaimPolicy":"Retain", "storageClassName":"",
            "claimRef":{"namespace":"wd-123","name":"app-456-data","uid":"pvc-uid"},
            "local":{"path":"/var/lib/webdock-storage/456/mount/data"},
            "nodeAffinity":{"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":["node-01"]}]}]}}
        },
        "status":{"phase":"Bound"}
    })).unwrap();
    let pvc = serde_json::from_value(json!({
        "metadata":metadata("app-456-data","pvc-uid"),
        "spec":{"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","volumeName":"wd-456-data","storageClassName":"","resources":{"requests":{"storage":"1Gi"}}},
        "status":{"phase":"Bound"}
    })).unwrap();
    let node = serde_json::from_value(json!({"metadata":{"name":"node-01","uid":"node-uid","resourceVersion":"1","labels":{"kubernetes.io/hostname":"node-01"}},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"machineID":"11111111111111111111111111111111","systemUUID":"11111111-1111-4111-8111-111111111111","architecture":"amd64","operatingSystem":"linux","kernelVersion":"fixture","kubeletVersion":"v1.32.0","kubeProxyVersion":"v1.32.0","osImage":"fixture","containerRuntimeVersion":"containerd://fixture","bootID":"11111111-1111-4111-8111-111111111111"}}})).unwrap();
    (command, config, deployment, pv, pvc, vec![node])
}

fn changed<T: serde::Serialize + serde::de::DeserializeOwned>(
    value: &T,
    pointer: &str,
    replacement: Value,
) -> T {
    let mut value = serde_json::to_value(value).unwrap();
    *value.pointer_mut(pointer).unwrap() = replacement;
    serde_json::from_value(value).unwrap()
}

#[test]
fn cross_node_apply_and_stop_prove_attachment_without_altering_objects() {
    let (mut command, config, deployment, pv, pvc, nodes) = fixture();
    let before = (deployment.clone(), pv.clone(), pvc.clone());
    assert!(verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_ok());
    command.action = Action::Stop;
    command.spec.replicas = 0;
    assert!(verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_ok());
    assert_eq!(before, (deployment, pv, pvc));
}

#[test]
fn local_storage_verification_is_not_rebound_to_a_foreign_node() {
    let (command, config, deployment, pv, pvc, nodes) = fixture();
    assert!(storage::verify(&command.spec, &config, Some(&pv), Some(&pvc)).is_err());
    assert!(verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_ok());
    assert_eq!(config.node, "node-02");
    let mut own = config;
    own.node = "node-01".into();
    assert!(storage::verify(&command.spec, &own, Some(&pv), Some(&pvc)).is_ok());
}

#[test]
fn foreign_owner_uid_capacity_path_mode_or_affinity_cannot_be_reused() {
    let (command, config, deployment, pv, pvc, nodes) = fixture();
    for (path, value) in [
        ("/metadata/uid", json!("foreign")),
        ("/metadata/labels/webdock.dev~1project-id", json!("999")),
        ("/metadata/annotations/webdock.dev~1revision", json!("6")),
    ] {
        let deployment = changed(&deployment, path, value);
        assert!(
            verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_err(),
            "{path}"
        );
    }
    for (path, value) in [
        (
            "/spec/local/path",
            json!("/var/lib/webdock-storage/999/mount/data"),
        ),
        ("/spec/claimRef/uid", json!("foreign")),
        ("/spec/capacity/storage", json!("2Gi")),
        ("/spec/volumeMode", json!("Block")),
        ("/spec/persistentVolumeReclaimPolicy", json!("Delete")),
        (
            "/spec/nodeAffinity/required/nodeSelectorTerms/0/matchExpressions/0/values",
            json!(["node-01", "node-02"]),
        ),
    ] {
        let pv = changed(&pv, path, value);
        assert!(
            verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_err(),
            "{path}"
        );
    }
    for (path, value) in [
        ("/spec/volumeMode", json!("Block")),
        ("/spec/resources/requests/storage", json!("2Gi")),
        ("/spec/volumeName", json!("foreign")),
    ] {
        let pvc = changed(&pvc, path, value);
        assert!(
            verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_err(),
            "{path}"
        );
    }
}

#[test]
fn no_expected_uid_storage_lifecycle_extra_sources_subpaths_or_unready_node() {
    let (command, config, deployment, pv, pvc, nodes) = fixture();
    let mut missing = command.clone();
    missing.expected_runtime_id = None;
    assert!(verify(&missing, &config, &deployment, &pv, &pvc, &nodes).is_err());
    for action in [Action::Retire, Action::PurgeStorage] {
        let mut command = command.clone();
        command.action = action;
        assert!(verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_err());
    }
    for key in ["subPath", "subPathExpr"] {
        let mut value = serde_json::to_value(&deployment).unwrap();
        value["spec"]["template"]["spec"]["containers"][0]["volumeMounts"][0][key] =
            json!("foreign");
        let deployment = serde_json::from_value(value).unwrap();
        assert!(verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_err());
    }
    let mut value = serde_json::to_value(&deployment).unwrap();
    value["spec"]["template"]["spec"]["volumes"][0]["emptyDir"] = json!({});
    let deployment = serde_json::from_value(value).unwrap();
    assert!(verify(&command, &config, &deployment, &pv, &pvc, &nodes).is_err());
    let node = changed(&nodes[0], "/status/conditions/0/status", json!("False"));
    assert!(verify(&command, &config, &fixture().2, &pv, &pvc, &[node]).is_err());
}

#[test]
fn privileged_initialization_or_unproved_public_routing_is_rejected_before_reuse() {
    let (command, config, deployment, pv, pvc, nodes) = fixture();
    for key in ["initContainers", "ephemeralContainers"] {
        let mut value = serde_json::to_value(&deployment).unwrap();
        value["spec"]["template"]["spec"][key] = json!([{"name":"root-init","image":command.spec.image,"securityContext":{"privileged":true,"runAsUser":0}}]);
        let modified = serde_json::from_value(value).unwrap();
        assert!(verify(&command, &config, &modified, &pv, &pvc, &nodes).is_err());
        assert!(deployment_update(modified, &command).is_err());
    }
    let mut routed = command.clone();
    routed.spec.hosting_binding.as_mut().unwrap().public_route = Some(PublicRoute {
        allocation_id: webdock_core::id::RecordId::parse("55").unwrap(),
        hostname: "fixture.apps.example".into(),
        revision: 5,
    });
    assert!(verify(&routed, &config, &deployment, &pv, &pvc, &nodes).is_err());
}

#[test]
fn service_account_credentials_cannot_survive_a_retained_update() {
    let (command, config, deployment, pv, pvc, nodes) = fixture();
    for key in ["serviceAccountName", "serviceAccount"] {
        let mut value = serde_json::to_value(&deployment).unwrap();
        value["spec"]["template"]["spec"][key] = json!("privileged-account");
        let modified = serde_json::from_value(value).unwrap();
        assert!(verify(&command, &config, &modified, &pv, &pvc, &nodes).is_err());
        assert!(deployment_update(modified, &command).is_err());
    }
    let mut value = serde_json::to_value(&deployment).unwrap();
    value["spec"]["template"]["spec"]["automountServiceAccountToken"] = json!(true);
    let modified = serde_json::from_value(value).unwrap();
    assert!(verify(&command, &config, &modified, &pv, &pvc, &nodes).is_err());
    assert!(deployment_update(modified, &command).is_err());
    let mut value = serde_json::to_value(&deployment).unwrap();
    value["spec"]["template"]["spec"]["volumes"].as_array_mut().unwrap().push(json!({"name":"token","projected":{"sources":[{"serviceAccountToken":{"path":"token","expirationSeconds":3600}}]}}));
    let modified = serde_json::from_value(value).unwrap();
    assert!(verify(&command, &config, &modified, &pv, &pvc, &nodes).is_err());
    assert!(deployment_update(modified, &command).is_err());
}

#[test]
fn deleting_missing_or_nonstatic_objects_cannot_be_reused() {
    let (command, config, deployment, pv, pvc, nodes) = fixture();
    for resource in ["deployment", "pv", "pvc"] {
        let mut d = serde_json::to_value(&deployment).unwrap();
        let mut p = serde_json::to_value(&pv).unwrap();
        let mut c = serde_json::to_value(&pvc).unwrap();
        let value = match resource {
            "deployment" => &mut d,
            "pv" => &mut p,
            _ => &mut c,
        };
        value["metadata"]["deletionTimestamp"] = json!("2026-01-01T00:00:00Z");
        assert!(
            verify(
                &command,
                &config,
                &serde_json::from_value(d).unwrap(),
                &serde_json::from_value(p).unwrap(),
                &serde_json::from_value(c).unwrap(),
                &nodes
            )
            .is_err()
        );
    }
    for (pointer, value) in [
        ("/metadata/resourceVersion", Value::Null),
        ("/metadata/uid", Value::Null),
        ("/spec/storageClassName", json!("dynamic")),
        ("/metadata/labels/webdock.dev~1app-id", json!("999")),
    ] {
        let p = changed(&pv, pointer, value.clone());
        let c = changed(&pvc, pointer, value);
        assert!(verify(&command, &config, &deployment, &p, &pvc, &nodes).is_err());
        assert!(verify(&command, &config, &deployment, &pv, &c, &nodes).is_err());
    }
    let mut source = serde_json::to_value(&pv).unwrap();
    source["spec"]["hostPath"] = json!({"path":"/foreign"});
    assert!(
        verify(
            &command,
            &config,
            &deployment,
            &serde_json::from_value(source).unwrap(),
            &pvc,
            &nodes
        )
        .is_err()
    );
    let mut mode = command.clone();
    mode.spec.replicas = 2;
    assert!(verify(&mode, &config, &deployment, &pv, &pvc, &nodes).is_err());
    let mut empty = deployment.clone();
    empty
        .spec
        .as_mut()
        .unwrap()
        .template
        .spec
        .as_mut()
        .unwrap()
        .volumes = None;
    assert!(verify(&command, &config, &empty, &pv, &pvc, &nodes).is_err());
}

#[test]
fn proof_detects_object_and_physical_node_changes_but_not_normal_node_heartbeat() {
    let (command, config, deployment, pv, pvc, nodes) = fixture();
    let proof = verify(&command, &config, &deployment, &pv, &pvc, &nodes).unwrap();
    let p = changed(&pv, "/metadata/resourceVersion", json!("11"));
    assert_ne!(
        proof,
        verify(&command, &config, &deployment, &p, &pvc, &nodes).unwrap()
    );
    let c = changed(&pvc, "/metadata/uid", json!("replacement"));
    assert!(verify(&command, &config, &deployment, &pv, &c, &nodes).is_err());
    let d = changed(&deployment, "/metadata/resourceVersion", json!("11"));
    assert_ne!(
        proof,
        verify(&command, &config, &d, &pv, &pvc, &nodes).unwrap()
    );
    let node = changed(&nodes[0], "/metadata/resourceVersion", json!("999"));
    assert_eq!(
        proof,
        verify(&command, &config, &deployment, &pv, &pvc, &[node]).unwrap()
    );
    let node = changed(
        &nodes[0],
        "/status/nodeInfo/systemUUID",
        json!("22222222-2222-4222-8222-222222222222"),
    );
    assert_ne!(
        proof,
        verify(&command, &config, &deployment, &pv, &pvc, &[node]).unwrap()
    );
    let mut alias = nodes[0].clone();
    alias.metadata.name = Some("other-node".into());
    assert!(
        verify(
            &command,
            &config,
            &deployment,
            &pv,
            &pvc,
            &[nodes[0].clone(), alias]
        )
        .is_err()
    );
    assert!(verify(&command, &config, &deployment, &pv, &pvc, &[]).is_err());
}
