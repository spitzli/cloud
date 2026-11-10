#![cfg(feature = "kubernetes")]
mod support;
use spitzli_agent::legacy::{deployment_update, name, namespace, owner_labels};
use spitzli_supervisor::*;
fn spec() -> RuntimeSpec {
    let mut spec = support::spec();
    spec.hosting_binding = Some(HostingBinding {
        public_route: None,
        cluster_id: spitzli_core::id::RecordId::parse("789").unwrap(),
        quota: allowance(),
        final_quota: allowance(),
    });
    spec
}
fn allowance() -> ResourceAllowance {
    ResourceAllowance {
        apps: 2,
        cpu_millicores: 1000,
        memory_bytes: 1024 * 1024 * 1024,
        volume_bytes: 1024 * 1024 * 1024,
        ephemeral_bytes: 1024 * 1024 * 1024,
        replicas_per_app: 1,
        concurrent_deployments: 1,
    }
}
fn existing(spec: &RuntimeSpec) -> k8s_openapi::api::apps::v1::Deployment {
    serde_json::from_value(serde_json::json!({"metadata":{"name":name(spec),"namespace":namespace(spec),"uid":"retained-uid","resourceVersion":"10","labels":owner_labels(spec,true).unwrap(),"annotations":{"webdock.dev/revision":"1"}},"spec":{"replicas":1,"selector":{"matchLabels":{"webdock.dev/app-id":"456"}},"template":{"metadata":{"labels":owner_labels(spec,true).unwrap()},"spec":{"containers":[{"name":"app","image":spec.image,"env":[{"name":"DATABASE_URL","valueFrom":{"secretKeyRef":{"name":"app-456-services-1","key":"url"}}}],"volumeMounts":[{"name":"data","mountPath":"/data"}]}],"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"app-456-data"}}],"imagePullSecrets":[{"name":"retained-registry"}]}}}})).unwrap()
}
#[test]
fn retained_probe_host_is_only_a_header_for_its_own_container() {
    for host in [None, Some("api.example.test:3130")] {
        let mut spec = spec();
        spec.health_host = host.map(str::to_owned);
        let mut command = Command {
            operation_id: "probe-host".into(),
            generation: 1,
            lease_until: timestamp().unwrap() + 60,
            authorization: "test".into(),
            material: None,
            action: Action::Apply,
            spec: spec.clone(),
            expected_runtime_id: None,
        };
        let (created, _) = spitzli_agent::legacy::new_workload(&command).unwrap();
        command.expected_runtime_id = Some("retained-uid".into());
        let updated = deployment_update(existing(&spec), &command).unwrap();
        for deployment in [created, updated] {
            let app = &deployment.spec.unwrap().template.spec.unwrap().containers[0];
            for probe in [
                &app.readiness_probe,
                &app.liveness_probe,
                &app.startup_probe,
            ] {
                let get = probe.as_ref().unwrap().http_get.as_ref().unwrap();
                assert!(get.host.is_none());
                assert_eq!(
                    get.http_headers.as_ref().map(|headers| headers
                        .iter()
                        .map(|header| (header.name.as_str(), header.value.as_str()))
                        .collect::<Vec<_>>()),
                    host.map(|host| vec![("Host", host)])
                );
            }
        }
    }
}
#[test]
fn retained_update_preserves_uid_storage_secret_refs_and_original_layout() {
    let mut spec = spec();
    spec.revision = 2;
    let old = existing(&spec);
    let command = Command {
        operation_id: "test-update".into(),
        generation: 1,
        lease_until: timestamp().unwrap() + 60,
        authorization: "test".into(),
        material: None,
        action: Action::Apply,
        spec: spec.clone(),
        expected_runtime_id: Some("retained-uid".into()),
    };
    let next = deployment_update(old.clone(), &command).unwrap();
    assert_eq!(next.metadata.uid, old.metadata.uid);
    assert_eq!(
        next.metadata.resource_version,
        old.metadata.resource_version
    );
    let before = old.spec.unwrap().template.spec.unwrap();
    let after = next.spec.unwrap().template.spec.unwrap();
    assert_eq!(before.volumes, after.volumes);
    assert_eq!(before.image_pull_secrets, after.image_pull_secrets);
    assert_eq!(before.containers[0].env, after.containers[0].env);
    assert_eq!(namespace(&spec), "wd-123");
    assert_eq!(name(&spec), "app-456");
    assert_eq!(
        after.containers[0]
            .resources
            .as_ref()
            .unwrap()
            .limits
            .as_ref()
            .unwrap()["cpu"]
            .0,
        "100m"
    );
    let mut wrong = command;
    wrong.expected_runtime_id = Some("foreign".into());
    assert!(deployment_update(existing(&spec), &wrong).is_err());
}

#[tokio::test]
#[ignore = "requires explicitly owned disposable Kind config/context/image"]
async fn existing_kubernetes_uid_local_volume_data_and_secret_references_survive_native_updates() {
    use k8s_openapi::api::{
        apps::v1::Deployment,
        core::v1::{Namespace, Node, PersistentVolume, PersistentVolumeClaim, Secret, Service},
    };
    use kube::{
        Api, Client, Config,
        api::{DeleteParams, ListParams, PostParams},
        config::{KubeConfigOptions, Kubeconfig},
    };
    use std::{io::Write, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
    struct Authority;
    #[async_trait::async_trait]
    impl Authorizer for Authority {
        async fn authorize(&self, _: &Command) -> Result<()> {
            Ok(())
        }
    }
    let config_path = PathBuf::from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap());
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("kind-webdock-rust-"));
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(&config_path).unwrap(),
        &KubeConfigOptions {
            context: Some(context),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::try_from(config).unwrap();
    let nodes: Api<Node> = Api::all(client.clone());
    let node = nodes
        .list(&ListParams::default().limit(2))
        .await
        .unwrap()
        .items
        .into_iter()
        .next()
        .unwrap()
        .metadata
        .name
        .unwrap();
    assert!(node.starts_with("webdock-rust-"));
    let nonce = uuid::Uuid::new_v4();
    let numeric = u64::from_be_bytes(nonce.as_bytes()[..8].try_into().unwrap())
        % 1_000_000_000_000_000
        + 1000;
    let mut spec = spec();
    spec.project_id = spitzli_core::id::RecordId::parse(&numeric.to_string()).unwrap();
    spec.resource_id = spitzli_core::id::RecordId::parse(&(numeric + 1).to_string()).unwrap();
    spec.image = std::env::var("WEBDOCK_KUBE_TEST_IMAGE").unwrap();
    spec.arguments.extend(["-h".into(), "/data".into()]);
    spec.volume = Some(RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: Some(67_108_864),
    });
    let ns_name = namespace(&spec);
    let app_name = name(&spec);
    let pv_name = format!("wd-{}-data", spec.resource_id);
    let claim = format!("{app_name}-data");
    let local_path = format!("/var/local/webdock-rust-tests/{}", nonce.simple());
    let run = |args: &[&str]| {
        let result = std::process::Command::new("docker")
            .env("DOCKER_HOST", "unix:///var/run/docker.sock")
            .arg("exec")
            .arg(&node)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run(&["mkdir", "-p", &local_path]);
    run(&["chown", "65532:65532", &local_path]);
    let mut writer = std::process::Command::new("docker")
        .env("DOCKER_HOST", "unix:///var/run/docker.sock")
        .args([
            "exec",
            "-i",
            &node,
            "tee",
            &format!("{local_path}/index.html"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    writer
        .stdin
        .take()
        .unwrap()
        .write_all(b"retained-fixture")
        .unwrap();
    assert!(writer.wait().unwrap().success());
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let mut ns_labels = owner_labels(&spec, false).unwrap();
    ns_labels.insert(
        "pod-security.kubernetes.io/enforce".into(),
        "restricted".into(),
    );
    namespaces
        .create(
            &PostParams::default(),
            &serde_json::from_value(
                serde_json::json!({"metadata":{"name":ns_name,"labels":ns_labels}}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let volumes: Api<PersistentVolume> = Api::all(client.clone());
    let claims: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), &ns_name);
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), &ns_name);
    let services: Api<Service> = Api::namespaced(client.clone(), &ns_name);
    let secrets: Api<Secret> = Api::namespaced(client.clone(), &ns_name);
    let metadata = |name: &str| serde_json::json!({"name":name,"labels":owner_labels(&spec,true).unwrap(),"annotations":{"webdock.dev/revision":"1"}});
    volumes.create(&PostParams::default(),&serde_json::from_value(serde_json::json!({"metadata":metadata(&pv_name),"spec":{"capacity":{"storage":"67108864"},"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","persistentVolumeReclaimPolicy":"Retain","storageClassName":"","claimRef":{"namespace":ns_name,"name":claim},"local":{"path":local_path},"nodeAffinity":{"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[node]}]}]}}}})).unwrap()).await.unwrap();
    claims.create(&PostParams::default(),&serde_json::from_value(serde_json::json!({"metadata":metadata(&claim),"spec":{"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","storageClassName":"","volumeName":pv_name,"resources":{"requests":{"storage":"67108864"}}}})).unwrap()).await.unwrap();
    let secret_name = format!("{app_name}-services-1");
    secrets.create(&PostParams::default(),&serde_json::from_value(serde_json::json!({"metadata":metadata(&secret_name),"stringData":{"url":"test-only"}})).unwrap()).await.unwrap();
    let initial = Command {
        operation_id: "fixture-initial".into(),
        generation: 1,
        lease_until: timestamp().unwrap() + 300,
        authorization: "fixture".into(),
        material: None,
        action: Action::Apply,
        spec: spec.clone(),
        expected_runtime_id: Some("retained-uid".into()),
    };
    let mut template = deployment_update(existing(&spec), &initial).unwrap();
    template.metadata.uid = None;
    template.metadata.resource_version = None;
    let pod = template
        .spec
        .as_mut()
        .unwrap()
        .template
        .spec
        .as_mut()
        .unwrap();
    pod.image_pull_secrets = None;
    pod.volumes.as_mut().unwrap()[0]
        .persistent_volume_claim
        .as_mut()
        .unwrap()
        .claim_name = claim.clone();
    pod.containers[0].env.as_mut().unwrap()[0]
        .value_from
        .as_mut()
        .unwrap()
        .secret_key_ref
        .as_mut()
        .unwrap()
        .name = secret_name.clone();
    template.spec.as_mut().unwrap().selector.match_labels = Some(std::collections::BTreeMap::from(
        [("webdock.dev/app-id".into(), spec.resource_id.to_string())],
    ));
    let created = deployments
        .create(&PostParams::default(), &template)
        .await
        .unwrap();
    let uid = created.metadata.uid.unwrap();
    services.create(&PostParams::default(),&serde_json::from_value(serde_json::json!({"metadata":metadata(&app_name),"spec":{"type":"ClusterIP","selector":{"webdock.dev/app-id":spec.resource_id.to_string()},"ports":[{"name":"http","port":spec.port,"targetPort":spec.port}]}})).unwrap()).await.unwrap();
    let client_for_task = client.clone();
    let observed_claim = claims.get(&claim).await.unwrap().metadata.uid;
    let result = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while !spitzli_agent::legacy::inspect(client_for_task.clone(), &spec)
            .await
            .unwrap()
            .ready
        {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        spec.revision = 2;
        spec.cpu_millicores = 200;
        let update = Command {
            operation_id: "retained-update".into(),
            generation: 1,
            lease_until: timestamp().unwrap() + 300,
            authorization: "fixture".into(),
            material: None,
            action: Action::Apply,
            spec: spec.clone(),
            expected_runtime_id: Some(uid.clone()),
        };
        let result = spitzli_agent::legacy::reconcile(
            client_for_task.clone(),
            &update,
            &LeaseGuard::new(update.clone(), Arc::new(Authority)),
        )
        .await
        .unwrap();
        assert!(result.ready);
        assert_eq!(result.runtime_id, Some(uid.clone()));
        let stop = Command {
            operation_id: "retained-stop".into(),
            spec: RuntimeSpec {
                revision: 3,
                ..spec.clone()
            },
            action: Action::Stop,
            ..update.clone()
        };
        assert_eq!(
            spitzli_agent::legacy::reconcile(
                client_for_task.clone(),
                &stop,
                &LeaseGuard::new(stop.clone(), Arc::new(Authority))
            )
            .await
            .unwrap()
            .phase,
            Phase::Stopped
        );
        let resume = Command {
            operation_id: "retained-resume".into(),
            spec: RuntimeSpec {
                revision: 4,
                ..spec
            },
            ..update
        };
        let result = spitzli_agent::legacy::reconcile(
            client_for_task,
            &resume,
            &LeaseGuard::new(resume.clone(), Arc::new(Authority)),
        )
        .await
        .unwrap();
        assert!(result.ready);
        assert_eq!(result.runtime_id, Some(uid));
    })
    .await;
    assert_eq!(
        claims.get(&claim).await.unwrap().metadata.uid,
        observed_claim
    );
    assert_eq!(
        volumes
            .get(&pv_name)
            .await
            .unwrap()
            .spec
            .unwrap()
            .local
            .unwrap()
            .path,
        local_path
    );
    secrets.get(&secret_name).await.unwrap();
    let bytes = std::process::Command::new("docker")
        .env("DOCKER_HOST", "unix:///var/run/docker.sock")
        .args(["exec", &node, "cat", &format!("{local_path}/index.html")])
        .output()
        .unwrap();
    assert_eq!(bytes.stdout, b"retained-fixture");
    namespaces
        .delete(&ns_name, &DeleteParams::default())
        .await
        .unwrap();
    for _ in 0..100 {
        if namespaces.get_opt(&ns_name).await.unwrap().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    volumes
        .delete(&pv_name, &DeleteParams::default())
        .await
        .unwrap();
    run(&["rm", "-r", "--", &local_path]);
    result.unwrap();
}

#[test]
fn new_legacy_objects_use_original_names_and_restricted_retained_claim() {
    let mut spec = support::spec();
    spec.hosting_binding = Some(spitzli_supervisor::HostingBinding {
        public_route: None,
        cluster_id: spitzli_core::id::RecordId::parse("789").unwrap(),
        quota: allowance(),
        final_quota: allowance(),
    });
    spec.volume = Some(spitzli_supervisor::RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: Some(67108864),
    });
    let command = spitzli_supervisor::Command {
        operation_id: "initial-create".into(),
        generation: 1,
        lease_until: spitzli_supervisor::timestamp().unwrap() + 60,
        authorization: "fixture-only".into(),
        material: None,
        action: spitzli_supervisor::Action::Apply,
        spec,
        expected_runtime_id: None,
    };
    let (deployment, service) = spitzli_agent::legacy::new_workload(&command).unwrap();
    let value = serde_json::to_value(deployment).unwrap();
    assert_eq!(value["metadata"]["namespace"], "wd-123");
    assert_eq!(value["metadata"]["name"], "app-456");
    assert_eq!(value["spec"]["strategy"]["type"], "Recreate");
    let pod = &value["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(
        pod["containers"][0]["securityContext"]["readOnlyRootFilesystem"],
        true
    );
    assert_eq!(
        pod["volumes"][1]["persistentVolumeClaim"]["claimName"],
        "app-456-data"
    );
    assert_eq!(service.metadata.name.as_deref(), Some("app-456"));
}

#[tokio::test]
#[ignore = "requires explicit owned disposable Kind node namespace, private kubeconfig and root; no production context"]
async fn disposable_node_allocates_fixed_storage_and_preserves_claims_on_retry_stop_resume() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("webdock_agent=debug")
        .try_init();
    use k8s_openapi::api::core::v1::{Namespace, Node, PersistentVolume, PersistentVolumeClaim};
    use kube::{
        Api, Client, Config,
        api::{DeleteParams, ListParams},
        config::{KubeConfigOptions, Kubeconfig},
    };
    use spitzli_agent::storage::{StorageClient, StoragePacket, StoragePool};
    use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_NODE_TEST").as_deref(),
        Ok("yes")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    assert!(std::path::Path::new("/.dockerenv").is_file());
    let kubeconfig = std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap();
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("kind-webdock-rust-"));
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(&kubeconfig).unwrap(),
        &KubeConfigOptions {
            context: Some(context),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::try_from(config).unwrap();
    let nodes: Api<Node> = Api::all(client.clone());
    let nodes = nodes.list(&ListParams::default()).await.unwrap();
    assert_eq!(nodes.items.len(), 1);
    let node = nodes.items[0].metadata.name.clone().unwrap();
    assert!(node.starts_with("webdock-rust-"));
    let base = tempfile::Builder::new()
        .prefix("webdock-allocation-owned-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.path().join("pool");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config_path = base.path().join("config.json");
    std::fs::write(&config_path,serde_json::to_vec(&serde_json::json!({"root":root,"node":node,"clusterID":"789","poolID":uuid::Uuid::new_v4().to_string(),"capacityBytes":268435456,"reserveBytes":67108864})).unwrap()).unwrap();
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    StoragePool::initialize(&config_path).unwrap();
    let socket = base.path().join("storage.sock");
    let server_socket = socket.clone();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        spitzli_agent::storage::serve(&config_path, &server_socket, async {
            let _ = stop_rx.await;
        })
        .await
    });
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let helper = StorageClient::new(socket);
    let storage = (helper.clone(), helper.information().await.unwrap());
    let numeric = u64::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap())
        % 1_000_000_000_000
        + 1000;
    let mut spec = spec();
    spec.project_id = spitzli_core::id::RecordId::parse(&numeric.to_string()).unwrap();
    spec.resource_id = spitzli_core::id::RecordId::parse(&(numeric + 1).to_string()).unwrap();
    spec.image = std::env::var("WEBDOCK_KUBE_TEST_IMAGE").unwrap();
    spec.memory_bytes = 134217728;
    spec.volume = Some(RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: Some(67108864),
    });
    spec.arguments=vec!["sh".into(),"-c".into(),"test -f /data/index.html || printf allocated-content > /data/index.html; exec httpd -f -p 8080 -h /data".into()];
    let command = Command {
        operation_id: "initial-allocation".into(),
        generation: 1,
        lease_until: timestamp().unwrap() + 300,
        authorization: "owned-fixture".into(),
        material: None,
        action: Action::Apply,
        spec: spec.clone(),
        expected_runtime_id: None,
    };
    struct Authority;
    #[async_trait::async_trait]
    impl Authorizer for Authority {
        async fn authorize(&self, c: &Command) -> Result<()> {
            if c.spec.installation == "test-install" {
                Ok(())
            } else {
                Err(ExecutionError::Denied)
            }
        }
    }
    let test_client = client.clone();
    let test_spec = spec.clone();
    let test_root = root.clone();
    let test = tokio::spawn(async move {
        let mut command = command;
        let first = spitzli_agent::legacy::reconcile_with_storage(
            test_client.clone(),
            &command,
            &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            Some(&storage),
        )
        .await
        .unwrap();
        assert!(first.ready);
        assert_eq!(
            first.storage,
            Some(StorageObservation {
                volume_bytes: Some(67108864),
                retained: false
            })
        );
        let path = test_root
            .join(test_spec.resource_id.as_str())
            .join("mount/data");
        assert_eq!(
            std::fs::read(path.join("index.html")).unwrap(),
            b"allocated-content"
        );
        let state = std::fs::read(test_root.join("_pool/state.json")).unwrap();
        let claims: Api<PersistentVolumeClaim> =
            Api::namespaced(test_client.clone(), &namespace(&test_spec));
        let claim_name = format!("{}-data", name(&test_spec));
        let claim_uid = claims.get(&claim_name).await.unwrap().metadata.uid;
        let retry = spitzli_agent::legacy::reconcile_with_storage(
            test_client.clone(),
            &command,
            &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            Some(&storage),
        )
        .await
        .unwrap();
        assert_eq!(retry.runtime_id, first.runtime_id);
        command.expected_runtime_id = first.runtime_id.clone();
        command.spec.revision = 2;
        command.operation_id = "stop-allocation".into();
        command.action = Action::Stop;
        assert_eq!(
            spitzli_agent::legacy::reconcile_with_storage(
                test_client.clone(),
                &command,
                &LeaseGuard::new(command.clone(), Arc::new(Authority)),
                Some(&storage)
            )
            .await
            .unwrap()
            .phase,
            Phase::Stopped
        );
        command.spec.revision = 3;
        command.operation_id = "resume-allocation".into();
        command.action = Action::Apply;
        let resumed = spitzli_agent::legacy::reconcile_with_storage(
            test_client.clone(),
            &command,
            &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            Some(&storage),
        )
        .await
        .unwrap();
        assert!(resumed.ready);
        assert_eq!(resumed.runtime_id, first.runtime_id);
        assert_eq!(
            claims.get(&claim_name).await.unwrap().metadata.uid,
            claim_uid
        );
        assert_eq!(
            std::fs::read(path.join("index.html")).unwrap(),
            b"allocated-content"
        );
        assert_eq!(
            std::fs::read(test_root.join("_pool/state.json")).unwrap(),
            state
        );
        command.spec.revision = 4;
        command.operation_id = "forbidden-resize".into();
        command.spec.volume.as_mut().unwrap().size_bytes = Some(134217728);
        assert!(
            spitzli_agent::legacy::reconcile_with_storage(
                test_client.clone(),
                &command,
                &LeaseGuard::new(command.clone(), Arc::new(Authority)),
                Some(&storage)
            )
            .await
            .is_err()
        );
        command.spec.volume.as_mut().unwrap().size_bytes = Some(67108864);
        let mut purge = command.clone();
        purge.operation_id = "premature-purge".into();
        purge.action = Action::PurgeStorage;
        purge.expected_runtime_id = None;
        assert!(
            spitzli_agent::legacy::reconcile_with_storage(
                test_client.clone(),
                &purge,
                &LeaseGuard::new(purge.clone(), Arc::new(Authority)),
                Some(&storage)
            )
            .await
            .is_err()
        );
        command.operation_id = "retire-allocation".into();
        command.action = Action::Retire;
        let retired = spitzli_agent::legacy::reconcile_with_storage(
            test_client.clone(),
            &command,
            &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            Some(&storage),
        )
        .await
        .unwrap();
        assert_eq!(retired.phase, Phase::Absent);
        assert!(retired.ready);
        assert_eq!(
            retired.storage,
            Some(StorageObservation {
                volume_bytes: Some(67108864),
                retained: true
            })
        );
        assert_eq!(
            claims.get(&claim_name).await.unwrap().metadata.uid,
            claim_uid
        );
        assert_eq!(
            std::fs::read(path.join("index.html")).unwrap(),
            b"allocated-content"
        );
        let replay = spitzli_agent::legacy::reconcile_with_storage(
            test_client.clone(),
            &command,
            &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            Some(&storage),
        )
        .await
        .unwrap();
        assert_eq!(replay.storage, retired.storage);
        let retained_state = std::fs::read(test_root.join("_pool/state.json")).unwrap();
        let inspected = spitzli_agent::legacy::inspect_completed(
            test_client.clone(),
            &command,
            &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            Some(&storage),
        )
        .await
        .unwrap();
        assert_eq!(inspected.storage, retired.storage);
        assert_eq!(
            std::fs::read(test_root.join("_pool/state.json")).unwrap(),
            retained_state
        );
        assert_eq!(
            std::fs::read(path.join("index.html")).unwrap(),
            b"allocated-content"
        );
        purge.spec.revision = 5;
        purge.operation_id = "purge-allocation".into();
        let purged = spitzli_agent::legacy::reconcile_with_storage(
            test_client.clone(),
            &purge,
            &LeaseGuard::new(purge.clone(), Arc::new(Authority)),
            Some(&storage),
        )
        .await
        .unwrap();
        assert_eq!(
            purged.storage,
            Some(StorageObservation {
                volume_bytes: Some(0),
                retained: false
            })
        );
        assert!(!path.exists());
        let inspected = spitzli_agent::legacy::inspect_completed(
            test_client.clone(),
            &purge,
            &LeaseGuard::new(purge.clone(), Arc::new(Authority)),
            Some(&storage),
        )
        .await
        .unwrap();
        assert_eq!(inspected.storage, purged.storage);
        assert!(claims.get_opt(&claim_name).await.unwrap().is_none());
        assert_eq!(
            spitzli_agent::legacy::reconcile_with_storage(
                test_client,
                &purge,
                &LeaseGuard::new(purge.clone(), Arc::new(Authority)),
                Some(&storage)
            )
            .await
            .unwrap()
            .storage,
            purged.storage
        );
    })
    .await;
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let _ = namespaces
        .delete(&namespace(&spec), &DeleteParams::default())
        .await;
    for _ in 0..300 {
        if namespaces
            .get_opt(&namespace(&spec))
            .await
            .unwrap()
            .is_none()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        namespaces
            .get_opt(&namespace(&spec))
            .await
            .unwrap()
            .is_none()
    );
    let volumes: Api<PersistentVolume> = Api::all(client);
    let pv_name = format!("wd-{}-data", spec.resource_id);
    let _ = volumes.delete(&pv_name, &DeleteParams::default()).await;
    for _ in 0..100 {
        if volumes.get_opt(&pv_name).await.unwrap().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(volumes.get_opt(&pv_name).await.unwrap().is_none());
    let packet = StoragePacket::new(
        spec.resource_id.as_str(),
        spec.project_id.as_str(),
        "789",
        1,
        67108864,
        timestamp().unwrap() + 60,
    )
    .unwrap();
    let purge = helper.purge(&packet).await;
    let _ = stop_tx.send(());
    server.await.unwrap().unwrap();
    purge.unwrap();
    test.unwrap();
}
