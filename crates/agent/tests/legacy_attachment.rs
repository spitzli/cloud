#![cfg(feature = "kubernetes")]
//! Explicitly owned K3s management fixture: real retained data, a different
//! controller-local pool identity, and no privileged storage socket at all.
//! BUILD remains the default. The separately pinned runtime profile creates
//! disposable QA objects in that cluster and requires a second explicit opt-in.
mod support;
use futures_util::FutureExt;
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{
        LimitRange, Namespace, Node, PersistentVolume, PersistentVolumeClaim, Pod, ResourceQuota,
        Secret, Service, ServiceAccount,
    },
    networking::v1::NetworkPolicy,
};
use kube::{
    Api, Client, Config,
    api::{DeleteParams, ListParams, PostParams, Preconditions},
    config::{KubeConfigOptions, Kubeconfig},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use spitzli_agent::{
    legacy::{name, namespace, new_workload, owner_labels},
    storage::{StorageClient, StorageConfig},
};
use spitzli_supervisor::*;
use std::{
    collections::BTreeMap,
    io::Cursor,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncReadExt;

const BASE: &str = "/opt/webdock-plugin-ci/legacy-attachment-fixture";
const BUILD_SYSTEM: &str = "e6d9884c-e6bd-4699-8a0c-99f2358ae8a1";
const BUILD_API: &str = "https://10.83.0.1:6443";
const BUILD_CA_SHA: &str = "a1fa015c59dca9c67aeb8934430f7f90c5f9bafa41faac2f7079c7bad55ef292";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QualificationProfile {
    Build,
    TrustedRuntimeWdEu02,
}

struct ProfilePins {
    base: &'static str,
    system: &'static str,
    api: &'static str,
    ca: &'static str,
    node: &'static str,
}
const BUILD_PINS: ProfilePins = ProfilePins {
    base: BASE,
    system: BUILD_SYSTEM,
    api: BUILD_API,
    ca: BUILD_CA_SHA,
    node: "webdock-build-eu-01",
};
const RUNTIME_PINS: ProfilePins = ProfilePins {
    base: "/opt/webdock-agent-qualification/retained-attachment-fixture",
    system: "91961f5a-b06b-44d1-9684-3f07ae5ca5bc",
    api: "https://10.0.0.2:6443",
    ca: "12381eae493b6fb156175929e9858d1c74770077d42ea12b115d8bffe0cf596a",
    node: "wd-eu-02",
};
const TRUSTED_IMAGE: &str = "ghcr.io/spitzli/tabularium@sha256:4627facbbdf045716949195b115c7ab5d823e8c2d810d8e3dda93eeaafd9e78f";

impl QualificationProfile {
    fn selected() -> Self {
        match std::env::var("WEBDOCK_RETAINED_QUALIFICATION_PROFILE") {
            Err(std::env::VarError::NotPresent) => Self::Build,
            Ok(value) if value == "build" => Self::Build,
            Ok(value) if value == "trusted-runtime-wd-eu-02" => {
                assert_eq!(
                    std::env::var("WEBDOCK_TRUSTED_RUNTIME_RETAINED_TEST").as_deref(),
                    Ok("1")
                );
                Self::TrustedRuntimeWdEu02
            }
            _ => panic!("Unknown qualification profile"),
        }
    }
    fn pins(self) -> &'static ProfilePins {
        match self {
            Self::Build => &BUILD_PINS,
            Self::TrustedRuntimeWdEu02 => &RUNTIME_PINS,
        }
    }
    fn pod_node(self) -> Option<&'static str> {
        match self {
            Self::Build => None,
            Self::TrustedRuntimeWdEu02 => Some(RUNTIME_PINS.node),
        }
    }
}

fn owned_base(profile: QualificationProfile) {
    let base = Path::new(profile.pins().base);
    assert_eq!(base.canonicalize().unwrap(), base);
    for path in base.ancestors() {
        let meta = std::fs::symlink_metadata(path).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink());
        assert_eq!(meta.uid(), 0);
        assert_eq!(meta.mode() & 0o022, 0);
    }
    assert_eq!(std::fs::metadata(base).unwrap().mode() & 0o777, 0o700);
}

fn fixture_root(path: &Path, profile: QualificationProfile) {
    owned_base(profile);
    assert_eq!(path.parent(), Some(Path::new(profile.pins().base)));
    let name = path.file_name().unwrap().to_str().unwrap();
    let id = uuid::Uuid::parse_str(name).unwrap();
    assert!(!id.is_nil());
    assert_eq!(name, id.to_string());
    assert!(
        matches!(std::fs::symlink_metadata(path),Err(error) if error.kind()==std::io::ErrorKind::NotFound)
    );
}

fn qualified_cluster(config: &Config, profile: QualificationProfile) {
    assert_eq!(
        uuid::Uuid::parse_str(
            std::fs::read_to_string("/sys/class/dmi/id/product_uuid")
                .unwrap()
                .trim()
        )
        .unwrap()
        .to_string(),
        profile.pins().system
    );
    assert_eq!(
        config.cluster_url.to_string().trim_end_matches('/'),
        profile.pins().api
    );
    assert!(!config.accept_invalid_certs);
    assert!(config.proxy_url.is_none());
    assert!(config.tls_server_name.is_none());
    assert!(
        config.root_cert_file.is_none(),
        "reloadable kubeconfig CA is outside the pinned fixture profile"
    );
    let path = Path::new("/var/lib/rancher/k3s/server/tls/server-ca.crt");
    let meta = std::fs::symlink_metadata(path).unwrap();
    assert!(meta.is_file() && !meta.file_type().is_symlink());
    assert_eq!(meta.uid(), 0);
    let pem = std::fs::read(path).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&pem)), profile.pins().ca);
    let trusted = rustls_pemfile::certs(&mut Cursor::new(pem))
        .map(|v| v.unwrap().as_ref().to_vec())
        .collect::<Vec<_>>();
    assert!(!trusted.is_empty());
    assert_eq!(config.root_cert.as_ref().unwrap(), &trusted);
}

async fn response(
    client: Client,
    namespace: &str,
    app_id: &str,
    profile: QualificationProfile,
) -> String {
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let values = pods
        .list(
            &ListParams::default()
                .labels(&format!("webdock.dev/app-id={app_id}"))
                .limit(3),
        )
        .await
        .unwrap();
    let ready = values
        .items
        .iter()
        .filter(|p| {
            p.metadata.deletion_timestamp.is_none()
                && p.status
                    .as_ref()
                    .and_then(|s| s.container_statuses.as_ref())
                    .is_some_and(|c| !c.is_empty() && c.iter().all(|v| v.ready))
        })
        .collect::<Vec<_>>();
    assert_eq!(ready.len(), 1);
    if let Some(expected) = profile.pod_node() {
        assert_eq!(
            ready[0].spec.as_ref().and_then(|s| s.node_name.as_deref()),
            Some(expected)
        );
    }
    let mut exec = pods
        .exec(
            ready[0].metadata.name.as_deref().unwrap(),
            [
                "bun",
                "--smol",
                "-e",
                "console.log(await (await fetch('http://127.0.0.1:8080/')).text())",
            ],
            &kube::api::AttachParams::default().stdout(true).stderr(true),
        )
        .await
        .unwrap();
    let mut output = String::new();
    exec.stdout()
        .unwrap()
        .read_to_string(&mut output)
        .await
        .unwrap();
    exec.join().await.unwrap();
    output.trim().into()
}

// Readiness certifies the complete owned namespace boundary, as in the real bootstrap.
async fn fixture_namespace_ready(client: Client, namespace: &Namespace, nonce: &str) {
    let ns = namespace.metadata.name.as_deref().unwrap();
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let owned = namespaces.get(ns).await.unwrap();
    assert_eq!(owned.metadata.uid, namespace.metadata.uid);
    assert_eq!(owned.metadata.labels, namespace.metadata.labels);
    assert!(owned.metadata.deletion_timestamp.is_none());
    assert!(owned.metadata.resource_version.is_some());
    assert_eq!(
        owned
            .metadata
            .labels
            .as_ref()
            .unwrap()
            .get("tests.webdock.dev/owner")
            .map(String::as_str),
        Some(nonce)
    );
    assert_eq!(
        owned
            .metadata
            .labels
            .as_ref()
            .unwrap()
            .get("security.webdock.dev/ready")
            .map(String::as_str),
        Some("false")
    );
    let mut labels = namespace.metadata.labels.clone().unwrap();
    labels.remove("security.webdock.dev/ready");
    labels.remove("pod-security.kubernetes.io/enforce");
    labels.remove("pod-security.kubernetes.io/enforce-version");
    labels.remove("kubernetes.io/metadata.name");
    let metadata = |name: &str| json!({"name":name,"namespace":ns,"labels":labels});
    let quotas: Api<ResourceQuota> = Api::namespaced(client.clone(), ns);
    let desired_quota: ResourceQuota = serde_json::from_value(json!({
        "metadata":metadata("fixture-allocation"),
        "spec":{"hard":{
            "requests.cpu":"400m","limits.cpu":"400m",
            "requests.memory":"256Mi","limits.memory":"256Mi",
            "requests.ephemeral-storage":"128Mi","limits.ephemeral-storage":"128Mi",
            "pods":"2","services":"1","services.nodeports":"0","services.loadbalancers":"0",
            "persistentvolumeclaims":"1","requests.storage":"64Mi",
            "count/jobs.batch":"0","count/secrets":"4","count/configmaps":"0"
        }}
    }))
    .unwrap();
    let quota = quotas
        .create(&PostParams::default(), &desired_quota)
        .await
        .unwrap();
    let limits: Api<LimitRange> = Api::namespaced(client.clone(), ns);
    let desired_limit: LimitRange = serde_json::from_value(json!({
        "metadata":metadata("fixture-defaults"),
        "spec":{"limits":[{"type":"Container",
            "defaultRequest":{"cpu":"200m","memory":"128Mi","ephemeral-storage":"64Mi"},
            "default":{"cpu":"200m","memory":"128Mi","ephemeral-storage":"64Mi"},
            "max":{"cpu":"200m","memory":"128Mi","ephemeral-storage":"64Mi"}
        }]}
    }))
    .unwrap();
    let limit = limits
        .create(&PostParams::default(), &desired_limit)
        .await
        .unwrap();
    let policies: Api<NetworkPolicy> = Api::namespaced(client.clone(), ns);
    let desired_policy: NetworkPolicy = serde_json::from_value(json!({
        "metadata":metadata("fixture-deny"),
        "spec":{"podSelector":{},"policyTypes":["Ingress","Egress"]}
    }))
    .unwrap();
    let policy = policies
        .create(&PostParams::default(), &desired_policy)
        .await
        .unwrap();
    let accounts: Api<ServiceAccount> = Api::namespaced(client.clone(), ns);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut account = loop {
        if let Some(account) = accounts.get_opt("default").await.unwrap() {
            break account;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "default ServiceAccount did not appear"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    account.automount_service_account_token = Some(false);
    account.metadata.labels = Some(labels.clone());
    let account = accounts
        .replace("default", &PostParams::default(), &account)
        .await
        .unwrap();
    let current_account = accounts.get("default").await.unwrap();
    assert_eq!(current_account.metadata.uid, account.metadata.uid);
    assert_eq!(current_account.automount_service_account_token, Some(false));
    assert_eq!(current_account.metadata.labels, Some(labels.clone()));
    let current_quota = quotas.get("fixture-allocation").await.unwrap();
    assert_eq!(current_quota.metadata.uid, quota.metadata.uid);
    assert_eq!(current_quota.metadata.labels, Some(labels.clone()));
    assert_eq!(current_quota.spec, desired_quota.spec);
    let current_limit = limits.get("fixture-defaults").await.unwrap();
    assert_eq!(current_limit.metadata.uid, limit.metadata.uid);
    assert_eq!(current_limit.metadata.labels, Some(labels.clone()));
    assert_eq!(current_limit.spec, desired_limit.spec);
    let current_policy = policies.get("fixture-deny").await.unwrap();
    assert_eq!(current_policy.metadata.uid, policy.metadata.uid);
    assert_eq!(current_policy.metadata.labels, Some(labels));
    assert_eq!(current_policy.spec, desired_policy.spec);
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    assert!(
        pods.list(&ListParams::default().limit(1))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let mut current = namespaces.get(ns).await.unwrap();
    assert_eq!(current.metadata.uid, namespace.metadata.uid);
    assert_eq!(current.metadata.labels, namespace.metadata.labels);
    assert!(current.metadata.resource_version.is_some());
    assert!(current.metadata.deletion_timestamp.is_none());
    let labels = current.metadata.labels.as_mut().unwrap();
    assert_eq!(
        labels.get("tests.webdock.dev/owner").map(String::as_str),
        Some(nonce)
    );
    assert_eq!(
        labels.get("security.webdock.dev/ready").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        labels
            .get("pod-security.kubernetes.io/enforce")
            .map(String::as_str),
        Some("restricted")
    );
    labels.insert("security.webdock.dev/ready".into(), "true".into());
    current.status = None;
    let ready = namespaces
        .replace(ns, &PostParams::default(), &current)
        .await
        .unwrap();
    assert_eq!(ready.metadata.uid, namespace.metadata.uid);
    assert_eq!(
        ready
            .metadata
            .labels
            .as_ref()
            .unwrap()
            .get("security.webdock.dev/ready")
            .map(String::as_str),
        Some("true")
    );
    eprintln!(
        "owned namespace hardening verified before readiness: quota, limits, default-deny, token-disabled ServiceAccount, Restricted PSA"
    );
}

struct Authority {
    client: Client,
    namespace: String,
    environment: String,
    pv: String,
    race: AtomicBool,
    raced: AtomicBool,
}
#[async_trait::async_trait]
impl Authorizer for Authority {
    async fn authorize(&self, command: &Command) -> Result<()> {
        if command.spec.installation != "test-install"
            || !command.operation_id.starts_with("fixture-")
        {
            return Err(ExecutionError::Denied);
        }
        if self.race.load(Ordering::SeqCst) && !self.raced.load(Ordering::SeqCst) {
            let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &self.namespace);
            if secrets
                .get_opt(&self.environment)
                .await
                .map_err(|_| ExecutionError::Runtime)?
                .is_some()
                && !self.raced.swap(true, Ordering::SeqCst)
            {
                let api: Api<PersistentVolume> = Api::all(self.client.clone());
                let mut value = api
                    .get(&self.pv)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
                value
                    .metadata
                    .annotations
                    .get_or_insert_default()
                    .insert("tests.webdock.dev/raced".into(), "true".into());
                api.replace(&self.pv, &PostParams::default(), &value)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires an explicitly owned pinned K3s profile, fresh local fixture path and cached immutable image"]
async fn owned_k3s_cross_node_update_preserves_data_env_and_rejects_raced_binding() {
    assert_eq!(
        std::env::var("WEBDOCK_OWNED_K3S_RETAINED_TEST").as_deref(),
        Ok("1")
    );
    // Only the retained adapter's sanitized API status diagnostics are enabled.
    tracing_subscriber::fmt()
        .with_env_filter("off,spitzli_agent::legacy=debug")
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .try_init()
        .expect("fixture diagnostic subscriber must be exclusive");
    let profile = QualificationProfile::selected();
    assert_eq!(
        uuid::Uuid::parse_str(
            std::fs::read_to_string("/sys/class/dmi/id/product_uuid")
                .unwrap()
                .trim()
        )
        .unwrap()
        .to_string(),
        profile.pins().system
    );
    let config_path = PathBuf::from(std::env::var("WEBDOCK_OWNED_K3S_CONFIG").unwrap());
    assert_eq!(config_path, Path::new("/etc/rancher/k3s/k3s.yaml"));
    assert_eq!(config_path.canonicalize().unwrap(), config_path);
    let config_meta = std::fs::symlink_metadata(&config_path).unwrap();
    assert!(config_meta.is_file() && !config_meta.file_type().is_symlink());
    assert_eq!(config_meta.uid(), 0);
    assert_eq!(config_meta.mode() & 0o077, 0);
    assert_eq!(config_meta.nlink(), 1);
    let context = std::env::var("WEBDOCK_OWNED_K3S_CONTEXT").unwrap();
    let root = PathBuf::from(std::env::var("WEBDOCK_OWNED_K3S_ROOT").unwrap());
    fixture_root(&root, profile);
    let node_name = std::env::var("WEBDOCK_OWNED_K3S_NODE").unwrap();
    assert_eq!(node_name, profile.pins().node);
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(&config_path).unwrap(),
        &KubeConfigOptions {
            context: Some(context),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    qualified_cluster(&config, profile);
    let client = Client::try_from(config).unwrap();
    let nodes: Api<Node> = Api::all(client.clone());
    let target = nodes.get(&node_name).await.unwrap();
    let info = target.status.as_ref().unwrap().node_info.as_ref().unwrap();
    if profile == QualificationProfile::TrustedRuntimeWdEu02 {
        assert_eq!(
            target.metadata.uid.as_deref(),
            Some("29366860-fa4c-45c9-bb5c-69c54b038d4e")
        );
        assert_eq!(
            target
                .metadata
                .labels
                .as_ref()
                .and_then(|v| v.get("kubernetes.io/hostname"))
                .map(String::as_str),
            Some(RUNTIME_PINS.node)
        );
        assert_eq!(info.machine_id, "791bf8e3d5facd8bcee9a4c9c7965695");
        assert!(
            target
                .status
                .as_ref()
                .unwrap()
                .conditions
                .as_ref()
                .unwrap()
                .iter()
                .any(|v| v.type_ == "Ready" && v.status == "True")
        );
        assert!(
            !target
                .spec
                .as_ref()
                .is_some_and(|v| v.unschedulable == Some(true))
        );
        eprintln!(
            "TRUSTED RUNTIME management qualification creates owned QA objects: node={} uid={} machine={} system={}",
            node_name,
            target.metadata.uid.as_deref().unwrap(),
            info.machine_id,
            info.system_uuid
        );
    }
    assert_eq!(
        info.machine_id,
        std::fs::read_to_string("/etc/machine-id").unwrap().trim()
    );
    assert_eq!(
        uuid::Uuid::parse_str(&info.system_uuid).unwrap(),
        uuid::Uuid::parse_str(
            std::fs::read_to_string("/sys/class/dmi/id/product_uuid")
                .unwrap()
                .trim()
        )
        .unwrap()
    );
    let numeric = u64::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap())
        % 1_000_000_000_000
        + 1000;
    let mut spec = support::spec();
    spec.project_id = spitzli_core::id::RecordId::parse(&numeric.to_string()).unwrap();
    spec.resource_id = spitzli_core::id::RecordId::parse(&(numeric + 1).to_string()).unwrap();
    spec.revision = 4;
    spec.image = std::env::var("WEBDOCK_OWNED_K3S_IMAGE").unwrap();
    assert!(spitzli_supervisor::image_digest(&spec.image));
    if profile == QualificationProfile::TrustedRuntimeWdEu02 {
        assert_eq!(spec.image, TRUSTED_IMAGE);
    }
    spec.cpu_millicores = 200;
    spec.memory_bytes = 134217728;
    spec.ephemeral_bytes = 67108864;
    spec.volume = Some(RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: Some(67108864),
    });
    spec.arguments=vec!["bun".into(),"--smol".into(),"-e".into(),"Bun.serve({port:8080,fetch:async()=>new Response((await Bun.file('/data/sentinel').text())+':'+process.env.PRESERVED)});".into()];
    let quota = ResourceAllowance {
        apps: 2,
        cpu_millicores: 1000,
        memory_bytes: 1073741824,
        volume_bytes: 1073741824,
        ephemeral_bytes: 1073741824,
        replicas_per_app: 1,
        concurrent_deployments: 1,
    };
    spec.hosting_binding = Some(HostingBinding {
        cluster_id: spitzli_core::id::RecordId::parse("789").unwrap(),
        quota: quota.clone(),
        final_quota: quota,
        public_route: None,
    });
    let ns = namespace(&spec);
    let app = name(&spec);
    let pv_name = format!("wd-{}-data", spec.resource_id);
    let claim = format!("{app}-data");
    let nonce = uuid::Uuid::new_v4().to_string();
    let mut labels = owner_labels(&spec, true).unwrap();
    labels.insert("tests.webdock.dev/owner".into(), nonce.clone());
    let namespaces: Api<Namespace> = Api::all(client.clone());
    assert!(namespaces.get_opt(&ns).await.unwrap().is_none());
    let mut namespace_labels = owner_labels(&spec, false).unwrap();
    namespace_labels.insert("tests.webdock.dev/owner".into(), nonce.clone());
    namespace_labels.insert("security.webdock.dev/ready".into(), "false".into());
    namespace_labels.insert(
        "pod-security.kubernetes.io/enforce".into(),
        "restricted".into(),
    );
    namespace_labels.insert(
        "pod-security.kubernetes.io/enforce-version".into(),
        "latest".into(),
    );
    let created_ns=namespaces.create(&PostParams::default(),&serde_json::from_value(json!({"metadata":{"name":ns,"labels":namespace_labels,"annotations":{"webdock.dev/revision":"4"}},"spec":{}})).unwrap()).await.unwrap();
    let ns_uid = created_ns.metadata.uid.clone().unwrap();
    let volumes: Api<PersistentVolume> = Api::all(client.clone());
    let claims: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), &ns);
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), &ns);
    let services: Api<Service> = Api::namespaced(client.clone(), &ns);
    let secrets: Api<Secret> = Api::namespaced(client.clone(), &ns);
    let policies: Api<NetworkPolicy> = Api::namespaced(client.clone(), &ns);
    let metadata = |name: &str| json!({"name":name,"namespace":ns,"labels":labels,"annotations":{"webdock.dev/revision":"4"}});
    let path = root.join(spec.resource_id.as_str()).join("mount/data");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let root_identity = std::fs::symlink_metadata(&root).unwrap();
    let marker = root.join("fixture-owner.json");
    std::fs::write(&marker, nonce.as_bytes()).unwrap();
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("sentinel"), b"retained-content").unwrap();
    let status = std::process::Command::new("chown")
        .arg("65532:65532")
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    let mut pv_meta = metadata(&pv_name);
    pv_meta.as_object_mut().unwrap().remove("namespace");
    let pv=volumes.create(&PostParams::default(),&serde_json::from_value(json!({"metadata":pv_meta,"spec":{"capacity":{"storage":"64Mi"},"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","persistentVolumeReclaimPolicy":"Retain","storageClassName":"","claimRef":{"namespace":ns,"name":claim},"local":{"path":path},"nodeAffinity":{"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[node_name]}]}]}}}})).unwrap()).await.unwrap();
    let pv_uid = pv.metadata.uid.clone().unwrap();
    let result=std::panic::AssertUnwindSafe(async {
        fixture_namespace_ready(client.clone(), &created_ns, &nonce).await;
        let created_claim=claims.create(&PostParams::default(),&serde_json::from_value(json!({"metadata":metadata(&claim),"spec":{"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","storageClassName":"","volumeName":pv_name,"resources":{"requests":{"storage":"64Mi"}}}})).unwrap()).await.unwrap();
        let claim_uid=created_claim.metadata.uid.clone().unwrap();
        // This is an existing-attachment test, not a binding-controller race.
        // A freshly prebound PV initially has no claim UID. Do not ask the
        // strict runtime inspector to adopt that transitional state.
        let binding_deadline=tokio::time::Instant::now()+Duration::from_secs(60);
        loop {
            let bound_pv=volumes.get(&pv_name).await.unwrap();let bound_claim=claims.get(&claim).await.unwrap();
            assert_eq!(bound_pv.metadata.uid.as_ref(),Some(&pv_uid));assert_eq!(bound_claim.metadata.uid.as_ref(),Some(&claim_uid));
            assert_eq!(bound_claim.spec.as_ref().unwrap().volume_name.as_deref(),Some(pv_name.as_str()));
            let attached_uid=bound_pv.spec.as_ref().and_then(|v|v.claim_ref.as_ref()).and_then(|v|v.uid.as_ref());
            assert!(attached_uid.is_none()||attached_uid==Some(&claim_uid),"fixture PV bound a different PVC UID");
            let pv_phase=bound_pv.status.as_ref().and_then(|v|v.phase.as_deref());let pvc_phase=bound_claim.status.as_ref().and_then(|v|v.phase.as_deref());
            if attached_uid==Some(&claim_uid)&&pv_phase==Some("Bound")&&pvc_phase==Some("Bound") {eprintln!("owned fixture binding mature: pvUID={} pvcUID={} phases=Bound/Bound",pv_uid,claim_uid);break;}
            assert!(tokio::time::Instant::now()<binding_deadline,"owned fixture binding did not mature: PV={pv_phase:?} PVC={pvc_phase:?} matchingUID={}",attached_uid==Some(&claim_uid));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let old_env=format!("{app}-env-4");let new_env=format!("{app}-env-5");
        secrets.create(&PostParams::default(),&serde_json::from_value(json!({"metadata":metadata(&old_env),"type":"Opaque","immutable":true,"stringData":{"PRESERVED":"retained-env-value","JSC_useJIT":"false"}})).unwrap()).await.unwrap();
        let initial=Command{operation_id:"fixture-initial".into(),generation:1,lease_until:timestamp().unwrap()+300,authorization:"fixture".into(),action:Action::Apply,spec:spec.clone(),expected_runtime_id:None,material:None};
        let (mut deployment,service)=new_workload(&initial).unwrap();
        if let Some(node)=profile.pod_node() {deployment.spec.as_mut().unwrap().template.spec.as_mut().unwrap().node_selector=Some(BTreeMap::from([("kubernetes.io/hostname".into(),node.into())]));}
        deployment.spec.as_mut().unwrap().template.spec.as_mut().unwrap().containers[0].env=Some(serde_json::from_value(json!([{"name":"PRESERVED","valueFrom":{"secretKeyRef":{"name":old_env,"key":"PRESERVED"}}},{"name":"JSC_useJIT","value":"false"}])).unwrap());
        let created=deployments.create(&PostParams::default(),&deployment).await.unwrap();let uid=created.metadata.uid.unwrap();
        services.create(&PostParams::default(),&service).await.unwrap();
        let deadline=tokio::time::Instant::now()+Duration::from_secs(120);
        while !spitzli_agent::legacy::inspect(client.clone(),&spec).await.unwrap().ready {assert!(tokio::time::Instant::now()<deadline);tokio::time::sleep(Duration::from_millis(250)).await;}
        let old=deployments.get(&app).await.unwrap();let old_secret=secrets.get(&old_env).await.unwrap();
        assert_eq!(response(client.clone(),&ns,spec.resource_id.as_str(),profile).await,"retained-content:retained-env-value");
        spec.revision=5;
        let source="a".repeat(64);
        spec.hosted_access=Some(HostedAccess{service_bindings:None,environment:Some(EnvironmentReference{revision:5,source_sha256:source.clone()})});
        let mut update=Command{operation_id:"fixture-update".into(),generation:1,lease_until:timestamp().unwrap()+300,authorization:"fixture".into(),action:Action::Apply,spec:spec.clone(),expected_runtime_id:Some(uid.clone()),material:None};
        update.material=Some(RuntimeMaterial{operation_id:update.operation_id.clone(),request_sha256:update.fingerprint().unwrap(),service_bindings:None,environment:Some(EnvironmentMaterial{source_sha256:source,values:BTreeMap::from([("PRESERVED".into(),"retained-env-value".into()),("JSC_useJIT".into(),"false".into())])})});
        let helper_socket=root.join("must-never-be-created.sock");
        let storage=(StorageClient::new(helper_socket.clone()),StorageConfig{root:root.clone(),node:"controller-node-02".into(),cluster_id:spec.hosting_binding.as_ref().unwrap().cluster_id.clone(),pool_id:uuid::Uuid::new_v4().to_string(),capacity_bytes:1073741824,reserve_bytes:67108864});
        let authority=Arc::new(Authority{client:client.clone(),namespace:ns.clone(),environment:new_env.clone(),pv:pv_name.clone(),race:AtomicBool::new(true),raced:AtomicBool::new(false)});
        let quotas:Api<ResourceQuota>=Api::namespaced(client.clone(),&ns);
        let namespace_before=namespaces.get(&ns).await.unwrap();
        assert!(quotas.get_opt("webdock-allocation").await.unwrap().is_none());
        let mut routed=update.clone();routed.spec.hosting_binding.as_mut().unwrap().public_route=Some(PublicRoute{allocation_id:spitzli_core::id::RecordId::parse("55").unwrap(),hostname:"fixture.apps.example".into(),revision:5});
        let hash=routed.fingerprint().unwrap();routed.material.as_mut().unwrap().request_sha256=hash;
        assert!(spitzli_agent::legacy::reconcile_with_storage(client.clone(),&routed,&LeaseGuard::new(routed.clone(),authority.clone()),Some(&storage)).await.is_err());
        assert_eq!(namespaces.get(&ns).await.unwrap(),namespace_before);
        assert!(quotas.get_opt("webdock-allocation").await.unwrap().is_none());
        assert!(policies.get_opt("webdock-acme-solver").await.unwrap().is_none());
        assert!(secrets.get_opt(&new_env).await.unwrap().is_none());
        assert!(spitzli_agent::legacy::reconcile_with_storage(client.clone(),&update,&LeaseGuard::new(update.clone(),authority.clone()),Some(&storage)).await.is_err());
        assert!(authority.raced.load(Ordering::SeqCst));
        let unchanged=deployments.get(&app).await.unwrap();
        assert_eq!(unchanged.spec,old.spec);assert_eq!(unchanged.metadata.uid,old.metadata.uid);
        assert_eq!(secrets.get(&old_env).await.unwrap().data,old_secret.data);
        authority.race.store(false,Ordering::SeqCst);
        let before_pv=volumes.get(&pv_name).await.unwrap();let before_pvc=claims.get(&claim).await.unwrap();
        let observed=spitzli_agent::legacy::reconcile_with_storage(client.clone(),&update,&LeaseGuard::new(update.clone(),authority),Some(&storage)).await.unwrap();
        assert!(observed.ready);assert_eq!(observed.runtime_id,Some(uid));
        assert!(observed.conditions.iter().any(|v|v.reason=="ExistingVerifiedAttachment"));
        let after=deployments.get(&app).await.unwrap();
        assert_eq!(after.spec.as_ref().unwrap().template.spec.as_ref().unwrap().volumes,old.spec.as_ref().unwrap().template.spec.as_ref().unwrap().volumes);
        assert_eq!(volumes.get(&pv_name).await.unwrap(),before_pv);assert_eq!(claims.get(&claim).await.unwrap(),before_pvc);
        assert_eq!(std::fs::read(path.join("sentinel")).unwrap(),b"retained-content");
        assert_eq!(response(client.clone(),&ns,spec.resource_id.as_str(),profile).await,"retained-content:retained-env-value");
        assert_eq!(secrets.get(&new_env).await.unwrap().data,old_secret.data);
        let mut stop=update.clone();stop.operation_id="fixture-stop".into();stop.action=Action::Stop;stop.spec.revision=6;stop.spec.hosted_access=None;stop.material=None;
        let stop_authority=Arc::new(Authority{client:client.clone(),namespace:ns.clone(),environment:new_env.clone(),pv:pv_name.clone(),race:AtomicBool::new(false),raced:AtomicBool::new(false)});
        let stopped=spitzli_agent::legacy::reconcile_with_storage(client.clone(),&stop,&LeaseGuard::new(stop.clone(),stop_authority.clone()),Some(&storage)).await.unwrap();
        assert_eq!(stopped.phase,Phase::Stopped);
        let mut resume=stop;resume.operation_id="fixture-resume".into();resume.action=Action::Apply;resume.spec.revision=7;
        let resumed=spitzli_agent::legacy::reconcile_with_storage(client.clone(),&resume,&LeaseGuard::new(resume.clone(),stop_authority),Some(&storage)).await.unwrap();
        assert!(resumed.ready);assert_eq!(resumed.runtime_id,after.metadata.uid);
        assert_eq!(response(client.clone(),&ns,spec.resource_id.as_str(),profile).await,"retained-content:retained-env-value");
        assert_eq!(volumes.get(&pv_name).await.unwrap(),before_pv);assert_eq!(claims.get(&claim).await.unwrap(),before_pvc);
        assert!(!helper_socket.exists());assert!(!root.join("_pool").exists());
    }).catch_unwind().await;
    owned_base(profile);
    assert_eq!(root.canonicalize().unwrap(), root);
    let actual_root = std::fs::symlink_metadata(&root).unwrap();
    assert!(actual_root.is_dir() && !actual_root.file_type().is_symlink());
    assert_eq!(actual_root.uid(), 0);
    assert_eq!(actual_root.mode() & 0o777, 0o700);
    assert_eq!(
        (actual_root.dev(), actual_root.ino()),
        (root_identity.dev(), root_identity.ino())
    );
    let marker_meta = std::fs::symlink_metadata(&marker).unwrap();
    assert!(marker_meta.is_file() && !marker_meta.file_type().is_symlink());
    assert_eq!(marker_meta.uid(), 0);
    assert_eq!(marker_meta.mode() & 0o777, 0o600);
    assert_eq!(marker_meta.nlink(), 1);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), nonce);
    let actual = namespaces.get(&ns).await.unwrap();
    assert_eq!(
        actual
            .metadata
            .labels
            .as_ref()
            .unwrap()
            .get("tests.webdock.dev/owner"),
        Some(&nonce)
    );
    namespaces
        .delete(
            &ns,
            &DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(ns_uid),
                    resource_version: actual.metadata.resource_version.clone(),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for _ in 0..200 {
        if namespaces.get_opt(&ns).await.unwrap().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(namespaces.get_opt(&ns).await.unwrap().is_none());
    let current_pv = volumes.get(&pv_name).await.unwrap();
    assert_eq!(current_pv.metadata.uid.as_ref(), Some(&pv_uid));
    assert_eq!(
        current_pv
            .metadata
            .labels
            .as_ref()
            .and_then(|v| v.get("tests.webdock.dev/owner")),
        Some(&nonce)
    );
    assert_eq!(
        current_pv
            .spec
            .as_ref()
            .and_then(|v| v.local.as_ref())
            .map(|v| v.path.as_str()),
        path.to_str()
    );
    assert_eq!(
        current_pv
            .spec
            .as_ref()
            .and_then(|v| v.claim_ref.as_ref())
            .and_then(|v| v.namespace.as_deref()),
        Some(ns.as_str())
    );
    assert_eq!(
        current_pv
            .spec
            .as_ref()
            .and_then(|v| v.claim_ref.as_ref())
            .and_then(|v| v.name.as_deref()),
        Some(claim.as_str())
    );
    volumes
        .delete(
            &pv_name,
            &DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(pv_uid),
                    resource_version: current_pv.metadata.resource_version.clone(),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for _ in 0..200 {
        if volumes.get_opt(&pv_name).await.unwrap().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(volumes.get_opt(&pv_name).await.unwrap().is_none());
    std::fs::remove_dir_all(root).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
