//! Opt-in acceptance against the owned local RUN fixture, never a production context.
#![cfg(feature = "kubernetes")]
mod support;

use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Namespace, Pod},
};
use kube::{
    Api, Client, Config,
    api::{DeleteParams, ListParams},
    config::{KubeConfigOptions, Kubeconfig},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use webdock_agent::kubernetes::{ApprovedChart, KubernetesAdapter};
use webdock_build::{
    artifact::{Archive, ArtifactStore, MAX_OUTPUT, StoredArtifact, validate_oci},
    publisher::{PublicationGuard, RegistryPublisher},
};
use webdock_supervisor::*;

struct Authority;
#[async_trait::async_trait]
impl Authorizer for Authority {
    async fn authorize(&self, command: &Command) -> Result<()> {
        if command.spec.installation.starts_with("test-") {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
}
struct PublicationAuthority(AtomicUsize);
#[async_trait::async_trait]
impl PublicationGuard for PublicationAuthority {
    async fn check(&self) -> webdock_build::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn command(spec: &RuntimeSpec, generation: u64) -> Command {
    Command {
        operation_id: format!("build-runtime-{generation}"),
        generation,
        lease_until: timestamp().unwrap() + 600,
        authorization: "owned-fixture-only".into(),
        material: None,
        action: Action::Apply,
        spec: spec.clone(),
        expected_runtime_id: None,
    }
}
fn private_path(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var(name).unwrap());
    let metadata = path.symlink_metadata().unwrap();
    assert!(path.is_absolute() && metadata.is_file() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(metadata.mode() & 0o077, 0);
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(std::fs::canonicalize(&path).unwrap(), path);
    path
}

async fn diagnose_release(spec: &RuntimeSpec, kubeconfig: &std::path::Path, context: &str) {
    // This fixture has no runtime secrets. Capture only bounded Helm status metadata,
    // never kubeconfig, manifest values or Secret bodies.
    let history = tokio::process::Command::new("/usr/local/bin/helm")
        .args([
            "history",
            &spec.name(),
            "--namespace",
            &spec.namespace(),
            "--kubeconfig",
            kubeconfig.to_str().unwrap(),
            "--kube-context",
            context,
            "--output",
            "json",
        ])
        .output()
        .await
        .unwrap();
    eprintln!(
        "owned release diagnostic: {}",
        String::from_utf8_lossy(&history.stdout[..history.stdout.len().min(16384)])
    );
}

async fn verify_owned_http(pods: &Api<Pod>, pod_name: &str) {
    let mut forward = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        pods.portforward(pod_name, &[8080]),
    )
    .await
    .unwrap()
    .unwrap();
    let mut stream = forward.take_stream(8080).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    // One loopback connection lets the maintained HTTP client handle Node's Date,
    // Content-Length and chunked responses without a custom HTTP parser.
    let bridge = tokio::spawn(async move {
        if let Ok(Ok((mut socket, peer))) =
            tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept()).await
        {
            assert!(peer.ip().is_loopback());
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                tokio::io::copy_bidirectional(&mut socket, &mut stream),
            )
            .await;
        }
    });
    let received = async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .http1_only()
            .pool_max_idle_per_host(0)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| "owned_http_client")?;
        let mut response = client
            .get(format!("http://{address}/"))
            .header("Connection", "close")
            .send()
            .await
            .map_err(|_| "owned_http_request")?;
        if response.status() != reqwest::StatusCode::OK {
            return Err("owned_http_status");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "owned_http_body")? {
            if body.len() + chunk.len() > 4096 {
                return Err("owned_http_limit");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
    .await;
    bridge.abort();
    forward.abort();
    let _ = bridge.await;
    assert_eq!(received.unwrap(), b"owned");
}

async fn sample_owned_run_during_build_outage(pods: &Api<Pod>, pod_name: &str, image_id: &str) {
    let Some(path) = std::env::var_os("WEBDOCK_BUILD_OUTAGE_TEST_WORK") else {
        return;
    };
    let root = PathBuf::from(path);
    let meta = root.symlink_metadata().unwrap();
    assert!(
        root.is_absolute()
            && root.canonicalize().unwrap() == root
            && meta.is_dir()
            && meta.mode() & 0o777 == 0o700
    );
    assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
    let write = |name: &str, value: &serde_json::Value| {
        let temporary = root.join(format!("{name}.tmp"));
        std::fs::write(&temporary, serde_json::to_vec(value).unwrap()).unwrap();
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::rename(temporary, root.join(name)).unwrap();
    };
    write(
        "run-ready.json",
        &serde_json::json!({"imageID":image_id,"podName":pod_name}),
    );
    let until = std::time::Instant::now() + std::time::Duration::from_secs(180);
    let mut samples = Vec::new();
    while !root.join("outage-complete").exists() {
        assert!(
            std::time::Instant::now() < until,
            "owned BUILD outage handshake deadline"
        );
        let pod = pods.get(pod_name).await.unwrap();
        assert_eq!(
            pod.status.unwrap().container_statuses.unwrap()[0].image_id,
            image_id
        );
        verify_owned_http(pods, pod_name).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        samples.push(serde_json::json!({"unixMs":now,"http200Owned":true,"imageUnchanged":true}));
        write(
            "http-samples.json",
            &serde_json::json!({"imageID":image_id,"samples":samples}),
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(samples.len() >= 3);
    verify_owned_http(pods, pod_name).await;
    println!(
        "owned RUN HTTP/digest stayed healthy through separately controlled BUILD API outage: {} samples",
        samples.len()
    );
}

async fn publish_verified_oci(
    oci: Vec<u8>,
    registry: std::net::SocketAddr,
) -> (
    tempfile::TempDir,
    ArtifactStore,
    StoredArtifact,
    String,
    String,
) {
    let image_digest = validate_oci(&oci).unwrap();
    let artifact_directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        artifact_directory.path(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let store = ArtifactStore::open(artifact_directory.path().into(), MAX_OUTPUT * 2).unwrap();
    let envelope = Archive::from_files(BTreeMap::from([("image.tar".into(), oci)]))
        .unwrap()
        .encode()
        .unwrap();
    let artifact = store.put("987661", "987662", 1, &envelope).unwrap();
    assert_eq!(artifact.image_digest(), image_digest);
    let guard = PublicationAuthority(AtomicUsize::new(0));
    let publisher = RegistryPublisher::loopback_test(registry).unwrap();
    let image = publisher
        .publish("987661", "987663", "987664", &store, &artifact, &guard)
        .await
        .unwrap();
    assert_eq!(
        image,
        format!("{registry}/customers/987661/projects/987663@{image_digest}")
    );
    assert!(guard.0.load(Ordering::SeqCst) > 5);
    let readback = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!(
            "http://{registry}/v2/customers/987661/projects/987663/manifests/{image_digest}"
        ))
        .header("Accept", "application/vnd.oci.image.manifest.v1+json")
        .send()
        .await
        .unwrap();
    assert!(readback.status().is_success());
    assert_eq!(
        readback
            .headers()
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        image_digest
    );
    assert_eq!(
        format!(
            "sha256:{:x}",
            Sha256::digest(readback.bytes().await.unwrap())
        ),
        image_digest
    );
    println!("owned registry immutable manifest verified: {image_digest}");

    (artifact_directory, store, artifact, image, image_digest)
}

#[tokio::test]
#[ignore = "requires --publish-only with an actual validated Kubernetes-built OCI archive"]
async fn real_build_oci_publishes_and_registry_readback_matches() {
    assert_eq!(
        std::env::var("WEBDOCK_BUILD_PUBLISH_TEST").as_deref(),
        Ok("1")
    );
    let registry: std::net::SocketAddr = std::env::var("WEBDOCK_BUILD_RUN_REGISTRY")
        .unwrap()
        .parse()
        .unwrap();
    assert!(registry.ip().is_loopback());
    let oci_path = private_path("WEBDOCK_BUILD_RUN_OCI");
    assert!(oci_path.metadata().unwrap().len() <= MAX_OUTPUT);
    let (_directory, _store, _artifact, image, digest) =
        publish_verified_oci(std::fs::read(oci_path).unwrap(), registry).await;
    assert!(image.ends_with(&digest));
    println!(
        "owned publication-only acceptance: OCI pushed and registry manifest readback verified: {digest}"
    );
}

#[tokio::test]
#[ignore = "requires tools/tests/kubernetes-build-runtime.py and actual validated Kubernetes build OCI"]
async fn real_build_registry_digest_rolls_out_and_failed_rollout_remains_recoverable() {
    runtime_acceptance(false).await;
}

#[tokio::test]
#[ignore = "requires --health-only with an actual Kubernetes-built Railpack OCI archive"]
async fn real_build_registry_digest_serves_http_under_native_runtime_policy() {
    assert_eq!(
        std::env::var("WEBDOCK_BUILD_HEALTH_TEST").as_deref(),
        Ok("1")
    );
    runtime_acceptance(true).await;
}

async fn runtime_acceptance(health_only: bool) {
    assert_eq!(
        std::env::var("WEBDOCK_BUILD_RUNTIME_TEST").as_deref(),
        Ok("1")
    );
    let kubeconfig = private_path("WEBDOCK_KUBE_TEST_CONFIG");
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("k3d-webdock-rust-build-run-"));
    let registry: std::net::SocketAddr = std::env::var("WEBDOCK_BUILD_RUN_REGISTRY")
        .unwrap()
        .parse()
        .unwrap();
    assert!(registry.ip().is_loopback());
    let oci_path = private_path("WEBDOCK_BUILD_RUN_OCI");
    assert!(oci_path.metadata().unwrap().len() <= MAX_OUTPUT);
    let oci = std::fs::read(oci_path).unwrap();
    let image_digest = validate_oci(&oci).unwrap();
    if !health_only {
        let archive = Archive::read(&oci, MAX_OUTPUT).unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(
            archive
                .files()
                .get(&format!(
                    "blobs/sha256/{}",
                    image_digest.strip_prefix("sha256:").unwrap()
                ))
                .unwrap(),
        )
        .unwrap();
        let config_digest = manifest["config"]["digest"]
            .as_str()
            .unwrap()
            .strip_prefix("sha256:")
            .unwrap();
        let image_config: serde_json::Value = serde_json::from_slice(
            archive
                .files()
                .get(&format!("blobs/sha256/{config_digest}"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            image_config["config"]["Entrypoint"],
            serde_json::json!(["/fixture"]),
            "use the actual checked-in fixture image with overridable CMD"
        );
        assert_eq!(image_config["config"]["Cmd"], serde_json::json!(["serve"]));
    }
    let (_artifact_directory, store, artifact, image, published_digest) =
        publish_verified_oci(oci, registry).await;
    assert_eq!(image_digest, published_digest);

    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(&kubeconfig).unwrap(),
        &KubeConfigOptions {
            context: Some(context.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(config.cluster_url.host(), Some("127.0.0.1"));
    assert!(!config.accept_invalid_certs);
    let client = Client::try_from(config).unwrap();
    let mut spec = support::spec();
    spec.installation = format!("test-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    spec.image = image.clone();
    spec.arguments.clear(); // Run the immutable image's default entrypoint and command.
    if health_only {
        spec.memory_bytes = 128 * 1024 * 1024;
    } // Bounded Node runtime budget.
    let namespace = spec.namespace();
    let expected_labels = spec.labels();
    let chart_path = std::fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helm/webdock-plugin"),
    )
    .unwrap();
    let chart =
        ApprovedChart::verify(&chart_path, &ApprovedChart::digest(&chart_path).unwrap()).unwrap();
    let adapter = KubernetesAdapter::connect(
        kubeconfig.clone(),
        context.clone(),
        spec.installation.clone(),
        "/usr/local/bin/helm".into(),
        chart,
    )
    .await
    .unwrap();
    let diagnostic_kubeconfig = kubeconfig.clone();
    let diagnostic_context = context.clone();
    let result = tokio::spawn(async move {
        let apply = command(&spec, 1);
        let ready = adapter.reconcile(&apply, &LeaseGuard::new(apply.clone(), Arc::new(Authority))).await;
        if ready.is_err() {
            let pods: Api<Pod> = Api::namespaced(client.clone(), &spec.namespace());
            if let Ok(pods) = pods.list(&ListParams::default().limit(10)).await {
                for pod in pods.items {
                    eprintln!("owned runtime startup diagnostic: {}", serde_json::to_string(&pod.status).unwrap());
                }
            }
        }
        let ready = ready.unwrap();
        assert!(ready.ready && ready.phase == Phase::Running);
        let deployment: Api<Deployment> = Api::namespaced(client.clone(), &spec.namespace());
        let before = deployment.get(&spec.name()).await.unwrap();
        assert_eq!(before.spec.as_ref().unwrap().template.spec.as_ref().unwrap().containers[0].image.as_deref(), Some(image.as_str()));
        let pods: Api<Pod> = Api::namespaced(client.clone(), &spec.namespace());
        let running = pods.list(&ListParams::default()).await.unwrap();
        assert_eq!(running.items.len(), 1);
        let image_id = running.items[0].status.as_ref().unwrap().container_statuses.as_ref().unwrap()[0].image_id.clone();
        assert!(image_id.ends_with(&image_digest), "runtime must pull the pushed manifest digest: {image_id}");
        println!("owned runtime pulled immutable manifest: {image_id}");
        verify_owned_http(&pods, running.items[0].metadata.name.as_ref().unwrap()).await;
        println!("owned runtime HTTP response verified");
        if health_only {
            let pod_spec = running.items[0].spec.as_ref().unwrap();
            assert_eq!(pod_spec.security_context.as_ref().unwrap().run_as_user, Some(65532));
            assert_eq!(pod_spec.containers[0].security_context.as_ref().unwrap().read_only_root_filesystem, Some(true));
            println!("owned health-only acceptance: exact digest serves HTTP as UID65532 with read-only root filesystem; recovery not exercised");
            sample_owned_run_during_build_outage(&pods,running.items[0].metadata.name.as_ref().unwrap(),&image_id).await;
            return;
        }

        // The build remains a valid, registry-verified image while a different rollout can fail.
        spec.revision = 2;
        spec.port = 8081;
        spec.arguments = vec!["wait".into()]; // A bad release starts without its HTTP listener.
        let bad = command(&spec, 2);
        let failed = adapter.reconcile(&bad, &LeaseGuard::new(bad.clone(), Arc::new(Authority))).await;
        assert!(failed.is_err(), "a failed readiness probe must not report a healthy deployment");
        diagnose_release(&spec, &diagnostic_kubeconfig, &diagnostic_context).await;
        let previous=adapter.inspect(&spec).await.unwrap();
        assert!(previous.ready && previous.revision==1,"failed desired revision2 must leave the prior revision1 healthy after atomic rollback");
        let preserved=deployment.get(&spec.name()).await.unwrap();
        let container=&preserved.spec.as_ref().unwrap().template.spec.as_ref().unwrap().containers[0];
        assert_eq!(container.ports.as_ref().unwrap()[0].container_port,8080);
        assert!(container.args.as_ref().is_none_or(|args|args.is_empty()));
        let preserved_pods=pods.list(&ListParams::default()).await.unwrap();assert_eq!(preserved_pods.items.len(),1);
        verify_owned_http(&pods,preserved_pods.items[0].metadata.name.as_ref().unwrap()).await;
        println!("owned failed8081 rollout returned error and atomic rollback kept revision1 on8080 serving HTTP");
        // Keep the original immutable image available rather than rebuilding or retagging it.
        assert_eq!(store.load(artifact.storage_key().into(), artifact.digest().into(), artifact.size(), artifact.image_digest().into()).unwrap().image_digest(), image_digest);
        spec.revision = 3;
        spec.port = 8080;
        spec.arguments.clear();
        let rollback = command(&spec, 3);
        let recovered = adapter.reconcile(&rollback, &LeaseGuard::new(rollback.clone(), Arc::new(Authority))).await;
        if recovered.is_err() { diagnose_release(&spec, &diagnostic_kubeconfig, &diagnostic_context).await; }
        let recovered = recovered.unwrap();
        assert!(recovered.ready && recovered.phase == Phase::Running);
        assert_eq!(recovered.runtime_id, ready.runtime_id);
        let restored = deployment.get(&spec.name()).await.unwrap();
        assert_eq!(restored.spec.unwrap().template.spec.unwrap().containers[0].image.as_deref(), Some(image.as_str()));
        println!("owned runtime acceptance: registry readback verified; exact digest pulled; failed rollout observed; retained digest restored");
    }).await;
    // Cleanup runs even after an assertion in the isolated test task; namespace ownership is checked.
    let namespaces: Api<Namespace> = Api::all(
        Client::try_from(
            Config::from_custom_kubeconfig(
                Kubeconfig::read_from(&kubeconfig).unwrap(),
                &KubeConfigOptions {
                    context: Some(context.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
        )
        .unwrap(),
    );
    if let Some(existing) = namespaces.get_opt(&namespace).await.unwrap() {
        let labels = existing.metadata.labels.as_ref().unwrap();
        assert!(
            expected_labels
                .iter()
                .all(|(key, value)| labels.get(key) == Some(value))
        );
        let status = tokio::process::Command::new("/usr/local/bin/helm")
            .args([
                "uninstall",
                &namespace,
                "--namespace",
                &namespace,
                "--kubeconfig",
                kubeconfig.to_str().unwrap(),
                "--kube-context",
                &context,
                "--no-hooks",
            ])
            .output()
            .await
            .unwrap();
        assert!(status.status.success(), "owned release cleanup failed");
        namespaces
            .delete(
                &namespace,
                &DeleteParams {
                    preconditions: Some(kube::api::Preconditions {
                        uid: existing.metadata.uid,
                        resource_version: None,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        while namespaces.get_opt(&namespace).await.unwrap().is_some() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "owned namespace cleanup did not finish"
            );
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    result.unwrap();
}
