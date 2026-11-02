#![cfg(feature = "kubernetes")]
mod support;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::{
    Api, Client, Config,
    api::{DeleteParams, PostParams},
    config::{KubeConfigOptions, Kubeconfig},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use webdock_agent::{
    kubernetes::{ApprovedChart, KubernetesAdapter},
    policy::{GrantKind, MountGrant, RuntimePolicy, RuntimePolicyConfig},
};
use webdock_supervisor::*;
struct TestAuthority;
#[async_trait::async_trait]
impl Authorizer for TestAuthority {
    async fn authorize(&self, c: &Command) -> Result<()> {
        if c.spec.installation.starts_with("test-") {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
}
fn command(spec: RuntimeSpec, generation: u64, action: Action) -> Command {
    Command {
        operation_id: format!("kube-operation-{generation}"),
        generation,
        lease_until: timestamp().unwrap() + 300,
        authorization: "test-only".into(),
        material: None,
        action,
        spec,
        expected_runtime_id: None,
    }
}
#[tokio::test]
#[ignore = "requires explicit disposable WEBDOCK_KUBE_TEST_CONFIG, CONTEXT and IMAGE"]
async fn independent_helm_release_rolls_out_suspends_resumes_and_preserves_approved_secret() {
    let kubeconfig = PathBuf::from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap());
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(
        context.starts_with("kind-webdock-rust-") || context.starts_with("k3d-webdock-rust-"),
        "only an owned disposable context is allowed"
    );
    let image = std::env::var("WEBDOCK_KUBE_TEST_IMAGE").unwrap();
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(&kubeconfig).unwrap(),
        &KubeConfigOptions {
            context: Some(context.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::try_from(config).unwrap();
    let mut spec = support::spec();
    spec.installation = format!("test-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    spec.image = image;
    spec.volume = Some(RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: Some(67108864),
    });
    spec.arguments=vec!["sh".into(),"-c".into(),"test -f /data/proof || printf retained-chart >/data/proof; exec httpd -f -p 8080 -h /run/secrets".into()];
    spec.health_path = Some("/index".into());
    spec.mounts = vec![MountReference {
        name: "index".into(),
        revision: 1,
    }];
    let ns = spec.namespace();
    let mut labels = spec.labels();
    labels.insert(
        "pod-security.kubernetes.io/enforce".into(),
        "restricted".into(),
    );
    let namespaces: Api<Namespace> = Api::all(client.clone());
    namespaces.create(&PostParams::default(),&serde_json::from_value(serde_json::json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":ns,"labels":labels}})).unwrap()).await.unwrap();
    let secret_name = RuntimePolicy::kubernetes_secret_name(&spec, "index");
    let secrets: Api<Secret> = Api::namespaced(client.clone(), &ns);
    let secret = Secret {
        metadata: kube::core::ObjectMeta {
            name: Some(secret_name.clone()),
            labels: Some(spec.labels()),
            ..Default::default()
        },
        string_data: Some(BTreeMap::from([("value".into(), "healthy-test".into())])),
        ..Default::default()
    };
    secrets
        .create(&PostParams::default(), &secret)
        .await
        .unwrap();
    let policy = RuntimePolicy::load(RuntimePolicyConfig {
        grants: vec![MountGrant {
            name: "index".into(),
            revision: 1,
            installation: spec.installation.clone(),
            project_id: spec.project_id.to_string(),
            resource_id: spec.resource_id.to_string(),
            kind: GrantKind::Secret {
                sha256: format!("{:x}", Sha256::digest(b"healthy-test")),
            },
        }],
        ..Default::default()
    })
    .unwrap();
    let chart = std::fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helm/webdock-plugin"),
    )
    .unwrap();
    let chart = ApprovedChart::verify(&chart, &ApprovedChart::digest(&chart).unwrap()).unwrap();
    let policy = Arc::new(policy);
    let metrics_chart = chart.clone();
    let revoked_chart = chart.clone();
    let adapter = KubernetesAdapter::connect(
        kubeconfig.clone(),
        context.clone(),
        spec.installation.clone(),
        PathBuf::from("/usr/local/bin/helm"),
        chart,
    )
    .await
    .unwrap()
    .with_policy(policy.clone());
    let (_reader_directory, metrics_reader) =
        metrics_reader(&client, &kubeconfig, &context, &spec, metrics_chart, policy).await;
    let revoked = KubernetesAdapter::connect(
        kubeconfig.clone(),
        context.clone(),
        spec.installation.clone(),
        "/usr/local/bin/helm".into(),
        revoked_chart,
    )
    .await
    .unwrap();
    let release = spec.name();
    let outcome = tokio::spawn(async move {
        let apply = command(spec.clone(), 1, Action::Apply);
        let result = adapter
            .reconcile(
                &apply,
                &LeaseGuard::new(apply.clone(), Arc::new(TestAuthority)),
            )
            .await
            .unwrap();
        assert!(result.ready);
        let mut metrics = None;
        for _ in 0..160 {
            match metrics_reader.metrics(&spec).await {
                Ok(value) => {
                    metrics = Some(value);
                    break;
                }
                Err(ExecutionError::Unsupported(_)) => {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await
                }
                Err(error) => panic!("real Kubernetes metrics: {error}"),
            }
        }
        let metrics = metrics.expect("Metrics API must report the owned runtime container");
        assert!(metrics.cpu_nanoseconds.is_none());
        assert!(metrics.cpu_nanocores.is_some());
        assert!(metrics.memory_bytes.is_some());
        assert!(timestamp().unwrap().saturating_sub(metrics.observed_at) <= 120);

        let claims: Api<k8s_openapi::api::core::v1::PersistentVolumeClaim> =
            Api::namespaced(client.clone(), &spec.namespace());
        let claim_name = format!("{}-data", spec.name());
        let initial_claim = claims.get(&claim_name).await.unwrap();
        let claim_uid = initial_claim.metadata.uid.clone();
        let pv_name = initial_claim.spec.unwrap().volume_name.unwrap();
        let volumes: Api<k8s_openapi::api::core::v1::PersistentVolume> = Api::all(client.clone());
        assert_eq!(
            volumes
                .get(&pv_name)
                .await
                .unwrap()
                .spec
                .unwrap()
                .persistent_volume_reclaim_policy
                .as_deref(),
            Some("Delete")
        );
        let uid = result.runtime_id;
        assert!(
            revoked
                .reconcile(
                    &apply,
                    &LeaseGuard::new(apply.clone(), Arc::new(TestAuthority))
                )
                .await
                .is_err()
        );
        let stop = command(spec.clone(), 2, Action::Stop);
        let result = revoked
            .reconcile(
                &stop,
                &LeaseGuard::new(stop.clone(), Arc::new(TestAuthority)),
            )
            .await
            .unwrap();
        assert_eq!(result.phase, Phase::Stopped);
        secrets.get(&secret_name).await.unwrap();
        let resume = command(spec.clone(), 3, Action::Apply);
        let result = adapter
            .reconcile(
                &resume,
                &LeaseGuard::new(resume.clone(), Arc::new(TestAuthority)),
            )
            .await
            .unwrap();
        assert!(result.ready);
        assert_eq!(result.runtime_id, uid);
        spec.revision = 2;
        spec.cpu_millicores = 200;
        let update = command(spec.clone(), 4, Action::Apply);
        let result = adapter
            .reconcile(
                &update,
                &LeaseGuard::new(update.clone(), Arc::new(TestAuthority)),
            )
            .await
            .unwrap();
        assert!(result.ready);
        assert_eq!(result.revision, 2);
        assert_eq!(result.runtime_id, uid);
        assert!(adapter.logs(&spec, 10).await.unwrap().len() <= 12000);
        spec.revision = 3;
        let mut retire = command(spec.clone(), 5, Action::Retire);
        retire.expected_runtime_id = uid.clone();
        use std::os::unix::fs::PermissionsExt;
        let journal_directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let executor = Executor::new(
            Journal::open(journal_directory.path().join("journal.json")).unwrap(),
            Arc::new(TestAuthority),
            Arc::new(adapter.clone()),
        );
        let retired = executor.execute(retire.clone()).await.unwrap();
        assert_eq!(retired.phase, Phase::Absent);
        assert!(retired.ready);
        assert_eq!(
            adapter
                .reconcile(
                    &retire,
                    &LeaseGuard::new(retire.clone(), Arc::new(TestAuthority))
                )
                .await
                .unwrap()
                .phase,
            Phase::Absent
        );
        secrets.get(&secret_name).await.unwrap();
        assert_eq!(
            claims.get(&claim_name).await.unwrap().metadata.uid,
            claim_uid
        );
        let retained_before = claims.get(&claim_name).await.unwrap();
        retire.generation = 6;
        assert_eq!(
            executor.execute(retire).await.unwrap().storage,
            retired.storage
        );
        assert_eq!(
            claims
                .get(&claim_name)
                .await
                .unwrap()
                .metadata
                .resource_version,
            retained_before.metadata.resource_version
        );
        spec.revision = 4;
        let mut purge = command(spec.clone(), 7, Action::PurgeStorage);
        let purged = executor.execute(purge.clone()).await.unwrap();
        assert_eq!(
            purged.storage,
            Some(StorageObservation {
                volume_bytes: Some(0),
                retained: false
            })
        );
        assert!(claims.get_opt(&claim_name).await.unwrap().is_none());
        assert!(volumes.get_opt(&pv_name).await.unwrap().is_none());
        let namespaces: Api<Namespace> = Api::all(client.clone());
        let purged_before = namespaces.get(&spec.namespace()).await.unwrap();
        purge.generation = 8;
        assert_eq!(
            executor.execute(purge.clone()).await.unwrap().storage,
            purged.storage
        );
        assert_eq!(
            namespaces
                .get(&spec.namespace())
                .await
                .unwrap()
                .metadata
                .resource_version,
            purged_before.metadata.resource_version
        );
        assert_eq!(
            adapter
                .reconcile(
                    &purge,
                    &LeaseGuard::new(purge.clone(), Arc::new(TestAuthority))
                )
                .await
                .unwrap()
                .storage,
            purged.storage
        );
    })
    .await;
    let _ = tokio::process::Command::new("/usr/local/bin/helm")
        .arg("uninstall")
        .arg(&release)
        .arg("--namespace")
        .arg(&ns)
        .arg("--kubeconfig")
        .arg(&kubeconfig)
        .arg("--kube-context")
        .arg(&context)
        .arg("--no-hooks")
        .output()
        .await;
    namespaces
        .delete(&ns, &DeleteParams::default())
        .await
        .unwrap();
    outcome.unwrap();
}

async fn metrics_reader(
    client: &Client,
    kubeconfig: &std::path::Path,
    context: &str,
    spec: &RuntimeSpec,
    chart: ApprovedChart,
    policy: Arc<RuntimePolicy>,
) -> (tempfile::TempDir, KubernetesAdapter) {
    use k8s_openapi::api::{
        authorization::v1::SelfSubjectAccessReview,
        core::v1::ServiceAccount,
        rbac::v1::{Role, RoleBinding},
    };
    use std::os::unix::fs::PermissionsExt;
    let namespace = spec.namespace();
    let accounts: Api<ServiceAccount> = Api::namespaced(client.clone(), &namespace);
    accounts
        .create(
            &PostParams::default(),
            &serde_json::from_value(serde_json::json!({"metadata":{"name":"metrics-reader"}}))
                .unwrap(),
        )
        .await
        .unwrap();
    let roles: Api<Role> = Api::namespaced(client.clone(), &namespace);
    roles.create(&PostParams::default(),&serde_json::from_value(serde_json::json!({"metadata":{"name":"metrics-reader"},"rules":[{"apiGroups":[""],"resources":["pods"],"verbs":["get","list"]},{"apiGroups":["apps"],"resources":["deployments"],"verbs":["get"]},{"apiGroups":["metrics.k8s.io"],"resources":["pods"],"verbs":["get"]}]})).unwrap()).await.unwrap();
    let bindings: Api<RoleBinding> = Api::namespaced(client.clone(), &namespace);
    bindings.create(&PostParams::default(),&serde_json::from_value(serde_json::json!({"metadata":{"name":"metrics-reader"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"Role","name":"metrics-reader"},"subjects":[{"kind":"ServiceAccount","name":"metrics-reader","namespace":namespace}]})).unwrap()).await.unwrap();
    let token = tokio::process::Command::new("kubectl")
        .arg("--kubeconfig")
        .arg(kubeconfig)
        .args([
            "--context",
            context,
            "create",
            "token",
            "metrics-reader",
            "-n",
            &namespace,
            "--duration=10m",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        token.status.success(),
        "TokenRequest failed: {}",
        String::from_utf8_lossy(&token.stderr)
    );
    let token = String::from_utf8(token.stdout).unwrap();
    let original = Kubeconfig::read_from(kubeconfig).unwrap();
    let chosen = original
        .contexts
        .iter()
        .find(|c| c.name == context)
        .unwrap()
        .context
        .as_ref()
        .unwrap();
    let cluster = original
        .clusters
        .iter()
        .find(|c| c.name == chosen.cluster)
        .unwrap()
        .cluster
        .clone()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("reader.json");
    std::fs::write(&path,serde_json::to_vec(&serde_json::json!({"apiVersion":"v1","kind":"Config","clusters":[{"name":"fixture","cluster":cluster}],"users":[{"name":"reader","user":{"token":token.trim()}}],"contexts":[{"name":"reader","context":{"cluster":"fixture","user":"reader","namespace":namespace}}],"current-context":"reader"})).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(&path).unwrap(),
        &KubeConfigOptions {
            context: Some("reader".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let reader = Client::try_from(config).unwrap();
    let reviews: Api<SelfSubjectAccessReview> = Api::all(reader);
    for attributes in [
        serde_json::json!({"verb":"get","resource":"nodes","subresource":"proxy"}),
        serde_json::json!({"verb":"create","resource":"pods","subresource":"exec","namespace":namespace}),
        serde_json::json!({"verb":"get","group":"metrics.k8s.io","resource":"pods","namespace":"kube-system"}),
    ] {
        let review: SelfSubjectAccessReview =
            serde_json::from_value(serde_json::json!({"spec":{"resourceAttributes":attributes}}))
                .unwrap();
        assert!(
            !reviews
                .create(&PostParams::default(), &review)
                .await
                .unwrap()
                .status
                .unwrap()
                .allowed
        );
    }
    let adapter = KubernetesAdapter::connect(
        path,
        "reader".into(),
        spec.installation.clone(),
        "/usr/local/bin/helm".into(),
        chart,
    )
    .await
    .unwrap()
    .with_policy(policy);
    (directory, adapter)
}
