#![cfg(feature = "kubernetes")]
use webdock_agent::plugin_runtime::{STAGE_SCRIPT, plugin_objects};
use webdock_agent_protocol::plugin::*;
#[path = "support/plugin_runtime.rs"]
mod fixture;
use fixture::claim;

#[test]
fn fixed_profile_has_private_staging_and_no_customer_or_storage_identity() {
    let claim = claim();
    let objects = plugin_objects(&claim).unwrap();
    let kinds: Vec<_> = objects
        .iter()
        .map(|o| o["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "Namespace",
            "Secret",
            "Deployment",
            "Service",
            "NetworkPolicy",
            "IngressRouteTCP"
        ]
    );
    let pod = &objects[2]["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(pod["securityContext"]["runAsUser"], 65532);
    assert_eq!(
        pod["containers"][0]["args"],
        serde_json::json!(["/run/webdock/runtime/runtime.json"])
    );
    assert_eq!(pod["initContainers"][0]["image"], claim.spec.image);
    assert_eq!(
        pod["initContainers"][0]["command"],
        serde_json::json!(["/bin/sh", "-ec", STAGE_SCRIPT])
    );
    assert!(
        pod["containers"][0]["volumeMounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["name"] != "source")
    );
    assert!(
        pod["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("hostPath").is_none() && v.get("persistentVolumeClaim").is_none())
    );
    assert_eq!(pod["volumes"][1]["emptyDir"]["medium"], "Memory");
    assert!(
        !serde_json::to_string(&objects)
            .unwrap()
            .contains("project-id")
    );
    assert_eq!(objects[5]["spec"]["tls"]["passthrough"], true);
    assert_eq!(objects[1]["immutable"], true);
}
#[test]
fn suspension_changes_replicas_without_replacing_owned_configuration() {
    let mut claim = claim();
    let apply = plugin_objects(&claim).unwrap();
    claim.lease.action = PluginRuntimeAction::Suspend;
    let stopped = plugin_objects(&claim).unwrap();
    assert_eq!(stopped[2]["spec"]["replicas"], 0);
    assert_eq!(apply[1], stopped[1]);
    assert_eq!(apply[0], stopped[0]);
}
#[test]
fn tampered_material_never_generates_kubernetes_secrets() {
    let mut claim = claim();
    claim
        .material
        .files
        .insert("runtime.key".into(), "changed".into());
    assert!(plugin_objects(&claim).is_err());
}

#[path = "support/plugin_kube.rs"]
mod api_fixture;
use std::sync::{Arc, Mutex, atomic::Ordering};
use webdock_agent::plugin_runtime::{
    KubernetesPluginRuntime, PluginRuntimeAuthority, PluginRuntimeExecutor,
};
struct Authority {
    state: Arc<api_fixture::State>,
    checkpoint: Mutex<Option<PluginRuntimeCheckpoint>>,
}
#[async_trait::async_trait]
impl PluginRuntimeAuthority for Authority {
    async fn guard(&self, claim: &PluginRuntimeClaim) -> webdock_supervisor::Result<()> {
        claim
            .validate(webdock_supervisor::timestamp()?)
            .map_err(|_| webdock_supervisor::ExecutionError::Denied)?;
        if self.state.revoked.load(Ordering::SeqCst) {
            return Err(webdock_supervisor::ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(
        &self,
        claim: &PluginRuntimeClaim,
        checkpoint: PluginRuntimeCheckpoint,
    ) -> webdock_supervisor::Result<()> {
        self.guard(claim).await?;
        *self.checkpoint.lock().unwrap() = Some(checkpoint);
        Ok(())
    }
}
#[tokio::test]
async fn real_kube_client_reconciles_lost_create_replies_and_retains_configuration_on_suspend() {
    let fixture = api_fixture::Fixture::open().await;
    fixture.state.lost_deployment.store(true, Ordering::SeqCst);
    let authority = Authority {
        state: fixture.state.clone(),
        checkpoint: Mutex::new(None),
    };
    let mut claim = claim();
    let runtime =
        KubernetesPluginRuntime::new(fixture.client.clone(), claim.spec.platform_id.clone());
    assert!(runtime.execute(&claim, &authority).await.is_err());
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 5);
    claim.checkpoint = authority.checkpoint.lock().unwrap().clone();
    let result = runtime.execute(&claim, &authority).await.unwrap();
    assert!(result.converged);
    assert!(result.checkpoint.complete());
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 6);
    claim.checkpoint = Some(result.checkpoint.clone());
    claim.lease.action = PluginRuntimeAction::Suspend;
    let stopped = runtime.execute(&claim, &authority).await.unwrap();
    assert_eq!(stopped.checkpoint, result.checkpoint);
    assert_eq!(fixture.state.objects.lock().unwrap().len(), 6);
    claim.lease.action = PluginRuntimeAction::Apply;
    runtime.execute(&claim, &authority).await.unwrap();
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 6);
    let objects = fixture.state.objects.lock().unwrap();
    assert_eq!(
        objects.values().filter(|o| o["kind"] == "Secret").count(),
        1
    );
}
#[tokio::test]
async fn revoked_guard_stops_after_uncertain_mutation_and_recovery_keeps_its_identity() {
    let fixture = api_fixture::Fixture::open().await;
    fixture.state.revoke_on_secret.store(true, Ordering::SeqCst);
    let authority = Authority {
        state: fixture.state.clone(),
        checkpoint: Mutex::new(None),
    };
    let mut claim = claim();
    let runtime =
        KubernetesPluginRuntime::new(fixture.client.clone(), claim.spec.platform_id.clone());
    assert!(runtime.execute(&claim, &authority).await.is_err());
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 3);
    claim.checkpoint = authority.checkpoint.lock().unwrap().clone();
    assert!(claim.checkpoint.as_ref().unwrap().secret_uid.is_none());
    fixture.state.revoked.store(false, Ordering::SeqCst);
    fixture
        .state
        .revoke_on_secret
        .store(false, Ordering::SeqCst);
    runtime.execute(&claim, &authority).await.unwrap();
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 6);
}
#[test]
fn actual_stage_script_copies_projection_symlinks_into_private_owned_regular_files() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let private = root.path().join("private");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&private).unwrap();
    for name in MATERIAL_FILES {
        let actual = source.join(format!("data-{name}"));
        std::fs::write(&actual, b"private-fixture").unwrap();
        std::fs::set_permissions(&actual, std::fs::Permissions::from_mode(0o440)).unwrap();
        std::os::unix::fs::symlink(actual, source.join(name)).unwrap();
    }
    let script = STAGE_SCRIPT
        .replace("/source", source.to_str().unwrap())
        .replace("/private", private.to_str().unwrap());
    assert!(
        std::process::Command::new("/bin/sh")
            .args(["-ec", &script])
            .status()
            .unwrap()
            .success()
    );
    let dir = private.join("runtime");
    assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
    for name in MATERIAL_FILES {
        let info = std::fs::symlink_metadata(dir.join(name)).unwrap();
        assert!(info.is_file());
        assert_eq!(info.nlink(), 1);
        assert_eq!(info.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(info.mode() & 0o777, 0o600);
    }
}

#[tokio::test]
#[ignore = "requires explicit private claim and owned Kubernetes context; no ambient cluster"]
async fn owned_cluster_executes_real_digest_artifact_and_keeps_uids_on_restart() {
    use kube::config::{KubeConfigOptions, Kubeconfig};
    let path =
        std::path::PathBuf::from(std::env::var("WEBDOCK_PLUGIN_RUNTIME_TEST_KUBECONFIG").unwrap());
    let context = std::env::var("WEBDOCK_PLUGIN_RUNTIME_TEST_CONTEXT").unwrap();
    let isolated =
        context.starts_with("kind-webdock-rust-") || context.starts_with("k3d-webdock-rust-");
    let explicit_dev = context == "webdock-dev"
        && std::env::var("WEBDOCK_PLUGIN_RUNTIME_ALLOW_DEV_K3S").as_deref() == Ok("true");
    assert!(
        isolated || explicit_dev,
        "an explicitly owned cluster is required"
    );
    let raw = Kubeconfig::read_from(&path).unwrap();
    assert!(
        raw.auth_infos
            .iter()
            .filter_map(|a| a.auth_info.as_ref())
            .all(|a| a.exec.is_none())
    );
    let config = kube::Config::from_custom_kubeconfig(
        raw,
        &KubeConfigOptions {
            context: Some(context),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!config.accept_invalid_certs);
    assert_eq!(config.cluster_url.scheme_str(), Some("https"));
    if explicit_dev {
        assert_eq!(config.cluster_url.host(), Some("127.0.0.1"));
        assert_eq!(config.cluster_url.port_u16(), Some(56443));
    }

    let client = kube::Client::try_from(config).unwrap();
    let bytes = webdock_agent::config::read_private(
        &std::path::PathBuf::from(std::env::var("WEBDOCK_PLUGIN_RUNTIME_TEST_CLAIM").unwrap()),
        512 * 1024,
    )
    .unwrap();
    let mut claim: PluginRuntimeClaim = serde_json::from_slice(&bytes).unwrap();
    assert!(claim.spec.platform_id.starts_with("test-"));
    claim
        .validate(webdock_supervisor::timestamp().unwrap())
        .unwrap();
    let namespaces: kube::Api<k8s_openapi::api::core::v1::Namespace> =
        kube::Api::all(client.clone());
    assert!(
        namespaces
            .get_opt(&claim.spec.namespace)
            .await
            .unwrap()
            .is_none(),
        "fixture must own a fresh namespace"
    );
    let authority = Authority {
        state: Arc::new(api_fixture::State::default()),
        checkpoint: Mutex::new(None),
    };
    let runtime = KubernetesPluginRuntime::new(client.clone(), claim.spec.platform_id.clone());
    let initial = runtime.execute(&claim, &authority).await.unwrap();
    claim.checkpoint = Some(initial.checkpoint.clone());
    claim.lease.action = PluginRuntimeAction::Suspend;
    let stopped = runtime.execute(&claim, &authority).await.unwrap();
    assert_eq!(initial.checkpoint, stopped.checkpoint);
    claim.lease.action = PluginRuntimeAction::Apply;
    let resumed = runtime.execute(&claim, &authority).await.unwrap();
    assert_eq!(initial.checkpoint, resumed.checkpoint);
    // This test observes deployment convergence only; root's installer separately authenticates
    // candidate Describe/Health before it can publish a ready runtime.
    let ns = namespaces.get(&claim.spec.namespace).await.unwrap();
    assert_eq!(ns.metadata.uid, initial.checkpoint.namespace_uid);
    let params = kube::api::DeleteParams {
        preconditions: Some(kube::api::Preconditions {
            uid: ns.metadata.uid,
            resource_version: ns.metadata.resource_version,
        }),
        ..Default::default()
    };
    namespaces
        .delete(&claim.spec.namespace, &params)
        .await
        .unwrap();
}

#[tokio::test]
async fn changed_route_during_rollout_cannot_be_reported_as_converged() {
    struct ChangedRoute {
        state: Arc<api_fixture::State>,
    }
    #[async_trait::async_trait]
    impl PluginRuntimeAuthority for ChangedRoute {
        async fn guard(&self, _: &PluginRuntimeClaim) -> webdock_supervisor::Result<()> {
            Ok(())
        }
        async fn checkpoint(
            &self,
            _: &PluginRuntimeClaim,
            checkpoint: PluginRuntimeCheckpoint,
        ) -> webdock_supervisor::Result<()> {
            if checkpoint.route_uid.is_some() {
                let mut objects = self.state.objects.lock().unwrap();
                let route = objects
                    .values_mut()
                    .find(|o| o["kind"] == "IngressRouteTCP")
                    .unwrap();
                route["metadata"]["uid"] = "replaced".into();
            }
            Ok(())
        }
    }
    let fixture = api_fixture::Fixture::open().await;
    let claim = claim();
    let runtime =
        KubernetesPluginRuntime::new(fixture.client.clone(), claim.spec.platform_id.clone());
    assert!(
        runtime
            .execute(
                &claim,
                &ChangedRoute {
                    state: fixture.state.clone()
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn network_isolation_exists_before_runtime_can_start() {
    struct Isolated;
    #[async_trait::async_trait]
    impl PluginRuntimeAuthority for Isolated {
        async fn guard(&self, _: &PluginRuntimeClaim) -> webdock_supervisor::Result<()> {
            Ok(())
        }
        async fn checkpoint(
            &self,
            _: &PluginRuntimeClaim,
            checkpoint: PluginRuntimeCheckpoint,
        ) -> webdock_supervisor::Result<()> {
            assert!(
                checkpoint.deployment_uid.is_none() || checkpoint.network_policy_uid.is_some(),
                "Deployment must never precede its isolation policy"
            );
            Ok(())
        }
    }
    let fixture = api_fixture::Fixture::open().await;
    let claim = claim();
    let runtime =
        KubernetesPluginRuntime::new(fixture.client.clone(), claim.spec.platform_id.clone());
    runtime.execute(&claim, &Isolated).await.unwrap();
}

#[tokio::test]
async fn tcp_crd_normalization_preserves_ready_runtime_convergence() {
    let fixture = api_fixture::Fixture::open().await;
    let authority = Authority {
        state: fixture.state.clone(),
        checkpoint: Mutex::new(None),
    };
    let claim = claim();
    let runtime =
        KubernetesPluginRuntime::new(fixture.client.clone(), claim.spec.platform_id.clone());
    let result = runtime.execute(&claim, &authority).await.unwrap();
    assert!(result.converged);
    assert!(result.checkpoint.complete());
    let objects = fixture.state.objects.lock().unwrap();
    let route = objects
        .values()
        .find(|object| object["kind"] == "IngressRouteTCP")
        .unwrap();
    assert!(
        route["spec"]["routes"][0]["services"][0]
            .get("kind")
            .is_none()
    );
    let profile = plugin_objects(&claim).unwrap();
    assert!(
        profile[5]["spec"]["routes"][0]["services"][0]
            .get("kind")
            .is_none(),
        "emit only fields supported by the TCP CRD"
    );
}

#[tokio::test]
async fn suspend_requires_complete_custody_without_creating_missing_resources() {
    let fixture = api_fixture::Fixture::open().await;
    let authority = Authority {
        state: fixture.state.clone(),
        checkpoint: Mutex::new(None),
    };
    let mut claim = claim();
    claim.lease.action = PluginRuntimeAction::Suspend;
    let runtime =
        KubernetesPluginRuntime::new(fixture.client.clone(), claim.spec.platform_id.clone());
    assert!(runtime.execute(&claim, &authority).await.is_err());
    assert_eq!(fixture.state.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn suspend_checks_all_six_objects_before_changing_replicas() {
    let fixture = api_fixture::Fixture::open().await;
    let authority = Authority {
        state: fixture.state.clone(),
        checkpoint: Mutex::new(None),
    };
    let mut claim = claim();
    let runtime =
        KubernetesPluginRuntime::new(fixture.client.clone(), claim.spec.platform_id.clone());
    claim.checkpoint = Some(
        runtime
            .execute(&claim, &authority)
            .await
            .unwrap()
            .checkpoint,
    );
    fixture
        .state
        .objects
        .lock()
        .unwrap()
        .values_mut()
        .find(|o| o["kind"] == "IngressRouteTCP")
        .unwrap()["metadata"]["uid"] = "foreign".into();
    claim.lease.action = PluginRuntimeAction::Suspend;
    assert!(runtime.execute(&claim, &authority).await.is_err());
    assert_eq!(fixture.state.patches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn suspend_waits_for_terminating_unready_pods_and_fences_already_zero_deployments() {
    let fixture = api_fixture::Fixture::open().await;
    let authority = Authority {
        state: fixture.state.clone(),
        checkpoint: Mutex::new(None),
    };
    let mut claim = claim();
    let runtime =
        KubernetesPluginRuntime::new(fixture.client.clone(), claim.spec.platform_id.clone());
    claim.checkpoint = Some(
        runtime
            .execute(&claim, &authority)
            .await
            .unwrap()
            .checkpoint,
    );
    claim.lease.action = PluginRuntimeAction::Suspend;
    fixture.state.pods.lock().unwrap().push(serde_json::json!({
        "apiVersion":"v1", "kind":"Pod", "metadata":{"name":"terminating","uid":"pod-1","deletionTimestamp":"2026-10-24T00:00:00Z"},
        "spec":{"containers":[{"name":"runtime","image":"fixture"}]},
        "status":{"phase":"Running","conditions":[{"type":"Ready","status":"False"}]}
    }));
    fixture
        .state
        .drain_pods_on_list
        .store(true, Ordering::SeqCst);
    runtime.execute(&claim, &authority).await.unwrap();
    assert!(
        fixture.state.pod_lists.load(Ordering::SeqCst) >= 2,
        "termination must be observed before completion"
    );
    let before = fixture
        .state
        .objects
        .lock()
        .unwrap()
        .values()
        .find(|o| o["kind"] == "Deployment")
        .unwrap()["metadata"]["resourceVersion"]
        .clone();
    claim.lease.token = "d".repeat(64);
    runtime.execute(&claim, &authority).await.unwrap();
    let after = fixture
        .state
        .objects
        .lock()
        .unwrap()
        .values()
        .find(|o| o["kind"] == "Deployment")
        .unwrap()["metadata"]["resourceVersion"]
        .clone();
    assert_ne!(
        before, after,
        "a fresh suspension fences previously read resource versions even at zero"
    );
    let deployments: kube::Api<k8s_openapi::api::apps::v1::Deployment> =
        kube::Api::namespaced(fixture.client.clone(), &claim.spec.namespace);
    let stale = serde_json::json!({"metadata":{"uid":claim.checkpoint.as_ref().unwrap().deployment_uid,"resourceVersion":before},"spec":{"replicas":1}});
    let error = deployments
        .patch(
            "runtime",
            &kube::api::PatchParams::default(),
            &kube::api::Patch::Merge(&stale),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, kube::Error::Api(response) if response.code == 409));
}
