#![cfg(feature = "kubernetes")]
mod support;
use serde_json::json;
use spitzli_agent::kubernetes::autoscaling::{manifest, verify};
use spitzli_supervisor::AutoscalingPolicy;

#[test]
fn hpa_is_bounded_and_tied_to_exact_owned_deployment() {
    let mut spec = support::spec();
    spec.autoscaling = Some(AutoscalingPolicy {
        min_replicas: 1,
        max_replicas: 4,
        target_cpu_utilization_percent: 70,
    });
    let value = manifest(&spec, "deployment-uid").unwrap();
    assert_eq!(value["spec"]["minReplicas"], 1);
    assert_eq!(value["spec"]["maxReplicas"], 4);
    assert_eq!(
        value["spec"]["metrics"][0]["resource"]["target"]["averageUtilization"],
        70
    );
    assert_eq!(
        value["spec"]["behavior"]["scaleDown"]["stabilizationWindowSeconds"],
        300
    );
    verify(&spec, "deployment-uid", &value, true).unwrap();
    let mut newer = spec.clone();
    newer.revision += 1;
    assert!(
        verify(&newer, "deployment-uid", &value, true).is_err(),
        "completion requires the current revision fence"
    );
    verify(&newer, "deployment-uid", &value, false).unwrap();
    for pointer in [
        "/metadata/namespace",
        "/spec/scaleTargetRef/name",
        "/metadata/ownerReferences/0/uid",
        "/metadata/labels/webdock.dev~1project-id",
    ] {
        let mut foreign = value.clone();
        *foreign.pointer_mut(pointer).unwrap() = json!("foreign");
        assert!(
            verify(&spec, "deployment-uid", &foreign, true).is_err(),
            "{pointer}"
        );
    }
    let mut changed = value.clone();
    changed["spec"]["maxReplicas"] = json!(20);
    assert!(verify(&spec, "deployment-uid", &changed, true).is_err());
    // Disabling/removing an owned policy verifies custody even though the new spec is fixed.
    spec.autoscaling = None;
    verify(&spec, "deployment-uid", &value, false).unwrap();
}

struct LocalAuthority;

#[test]
fn fixed_and_stopped_values_remain_compatible_with_pre_scaling_charts() {
    let mut spec = support::spec();
    assert!(
        spitzli_agent::kubernetes::helm_values(&spec, 1, false)
            .unwrap()
            .get("autoscaling")
            .is_none()
    );
    spec.autoscaling = Some(AutoscalingPolicy {
        min_replicas: 1,
        max_replicas: 2,
        target_cpu_utilization_percent: 70,
    });
    assert!(
        spitzli_agent::kubernetes::helm_values(&spec, 1, false)
            .unwrap()
            .get("autoscaling")
            .is_some()
    );
    assert!(
        spitzli_agent::kubernetes::helm_values(&spec, 1, true)
            .unwrap()
            .get("autoscaling")
            .is_none()
    );
}
#[async_trait::async_trait]
impl spitzli_supervisor::Authorizer for LocalAuthority {
    async fn authorize(
        &self,
        command: &spitzli_supervisor::Command,
    ) -> spitzli_supervisor::Result<()> {
        if command.spec.installation.starts_with("test-auto-") {
            Ok(())
        } else {
            Err(spitzli_supervisor::ExecutionError::Denied)
        }
    }
}

#[tokio::test]
#[ignore = "owned local Kubernetes cluster with metrics; explicit WEBDOCK_KUBE_TEST_CONFIG/CONTEXT/IMAGE"]
async fn real_hpa_scales_both_directions_and_cannot_restart_stopped_app() {
    acceptance(false, false).await;
}

#[tokio::test]
#[ignore = "owned local Kubernetes cluster with metrics; retained deployment compatibility"]
async fn real_retained_hpa_preserves_identity_and_stops() {
    acceptance(true, false).await;
}

#[tokio::test]
#[ignore = "owned local Kubernetes cluster with metrics; policy update and deletion"]
async fn real_policy_update_and_delete() {
    acceptance(false, true).await;
    acceptance(true, true).await;
}

async fn acceptance(retained: bool, short: bool) {
    use k8s_openapi::api::{
        autoscaling::v2::HorizontalPodAutoscaler,
        core::v1::{Namespace, Pod},
    };
    use kube::{
        Api, Client, Config,
        api::{DeleteParams, ListParams},
        config::{KubeConfigOptions, Kubeconfig},
    };
    use spitzli_agent::{
        kubernetes::{ApprovedChart, KubernetesAdapter},
        policy::{RuntimePolicy, RuntimePolicyConfig},
    };
    use spitzli_supervisor::*;
    use std::{path::PathBuf, sync::Arc, time::Duration};
    let path = PathBuf::from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap());
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("kind-webdock-rust-"));
    assert_eq!(
        std::env::var("WEBDOCK_AGENT_AUTOSCALING_ENABLED").unwrap(),
        "true"
    );
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(&path).unwrap(),
        &KubeConfigOptions {
            context: Some(context.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        config.cluster_url.host(),
        Some("127.0.0.1" | "localhost")
    ));
    let client = Client::try_from(config).unwrap();
    let mut spec = support::spec();
    spec.installation = format!(
        "test-auto-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    );
    spec.image = std::env::var("WEBDOCK_KUBE_TEST_IMAGE").unwrap();
    if retained {
        spec.project_id = spitzli_core::id::RecordId::parse(&format!(
            "{}",
            uuid::Uuid::new_v4().as_u128() % 1_000_000_000_000_000 + 1
        ))
        .unwrap();
        let allowance = ResourceAllowance {
            apps: 4,
            cpu_millicores: 1000,
            memory_bytes: 1073741824,
            ephemeral_bytes: 1073741824,
            volume_bytes: 0,
            replicas_per_app: 4,
            concurrent_deployments: 2,
        };
        spec.hosting_binding = Some(HostingBinding {
            public_route: None,
            cluster_id: spitzli_core::id::RecordId::parse("789").unwrap(),
            quota: allowance.clone(),
            final_quota: allowance,
        });
    }
    let (namespace, workload_name, _) =
        spitzli_agent::kubernetes::autoscaling::identity(&spec).unwrap();
    spec.cpu_millicores = 100;
    spec.memory_bytes = 67108864;
    spec.autoscaling = Some(AutoscalingPolicy {
        min_replicas: 1,
        max_replicas: 2,
        target_cpu_utilization_percent: 70,
    });
    spec.arguments = vec!["sh".into(), "-c".into(), "mkdir -p /tmp/www; hostname >/tmp/www/index.html; (while :; do :; done) & echo $! >/tmp/load.pid; exec httpd -f -p 8080 -h /tmp/www".into()];
    let chart = std::fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helm/webdock-plugin"),
    )
    .unwrap();
    let chart = ApprovedChart::verify(&chart, &ApprovedChart::digest(&chart).unwrap()).unwrap();
    let diagnostics = tempfile::tempdir().unwrap();
    let helm = diagnostics.path().join("helm");
    let log = diagnostics.path().join("helm.log");
    std::fs::write(
        &helm,
        format!(
            "#!/bin/sh\nexec /usr/local/bin/helm \"$@\" 2>>{}\n",
            log.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&helm, std::fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = KubernetesAdapter::connect(
        path.clone(),
        context.clone(),
        spec.installation.clone(),
        helm,
        chart,
    )
    .await
    .unwrap()
    .with_policy(Arc::new(
        RuntimePolicy::load(RuntimePolicyConfig::default()).unwrap(),
    ));
    let namespaces: Api<Namespace> = Api::all(client.clone());
    assert!(namespaces.get_opt(&namespace).await.unwrap().is_none());
    let client_namespace = format!("{}-client", spec.installation);
    let result: Result<()> = async {
        namespaces.create(&kube::api::PostParams::default(), &serde_json::from_value(json!({
            "apiVersion":"v1","kind":"Namespace","metadata":{"name":client_namespace,"labels":{
                "webdock.dev/role":"control-plane","pod-security.kubernetes.io/enforce":"restricted",
                "spitzli.cloud/owned-fixture":spec.installation}}
        })).unwrap()).await.unwrap();
        let client_pods:Api<Pod>=Api::namespaced(client.clone(),&client_namespace);
        client_pods.create(&kube::api::PostParams::default(),&serde_json::from_value(json!({
            "apiVersion":"v1","kind":"Pod","metadata":{"name":"probe","labels":{"webdock.dev/component":"control-plane"}},
            "spec":{"automountServiceAccountToken":false,"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},
            "containers":[{"name":"probe","image":spec.image,"command":["sleep","1200"],"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}}}],"restartPolicy":"Never"}
        })).unwrap()).await.unwrap();
        let client_deadline=tokio::time::Instant::now()+Duration::from_secs(90);
        while client_pods.get("probe").await.unwrap().status.and_then(|s|s.phase).as_deref()!=Some("Running") {
            assert!(tokio::time::Instant::now()<client_deadline,"probe pod did not start");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        let command = |spec: RuntimeSpec, action: Action, uid: Option<String>| Command {
            operation_id: format!("autoscaling-{}", spec.revision),
            generation: spec.revision,
            lease_until: timestamp().unwrap() + 900,
            authorization: "local-test".into(),
            material: None,
            action,
            spec,
            expected_runtime_id: uid,
        };
        let first = command(spec.clone(), Action::Apply, None);
        let initial = adapter
            .reconcile(
                &first,
                &LeaseGuard::new(first.clone(), Arc::new(LocalAuthority)),
            )
            .await?;
        let uid = initial.runtime_id.clone().unwrap();
        let hpas: Api<HorizontalPodAutoscaler> = Api::namespaced(client.clone(), &namespace);
        if short {
            spec.revision += 1;
            spec.autoscaling.as_mut().unwrap().target_cpu_utilization_percent = 65;
            let update = command(spec.clone(), Action::Apply, Some(uid.clone()));
            assert!(adapter.reconcile(&update, &LeaseGuard::new(update.clone(), Arc::new(LocalAuthority))).await?.ready);
            let hpa = serde_json::to_value(hpas.get(&workload_name).await.unwrap()).unwrap();
            assert_eq!(hpa["spec"]["metrics"][0]["resource"]["target"]["averageUtilization"], 65);
            spec.revision += 1;
            let deletion = command(spec.clone(), Action::Retire, Some(uid));
            let result = adapter.reconcile(&deletion, &LeaseGuard::new(deletion.clone(), Arc::new(LocalAuthority))).await?;
            assert_eq!(result.phase, Phase::Absent);
            assert!(result.ready);
            assert!(hpas.get_opt(&workload_name).await.unwrap().is_none());
            println!("{retained}: policy update and direct automatic-app deletion passed");
            return Ok(());
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            let value = adapter.inspect(&spec).await?;
            if value.ready
                && value
                    .autoscaling
                    .as_ref()
                    .is_some_and(|s| s.current_replicas == 2 && s.metrics_available)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "real scale-up did not reach max"
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        println!("{retained}: scale-up reached maximum");
        let deployments: Api<k8s_openapi::api::apps::v1::Deployment> = Api::namespaced(client.clone(), &namespace);
        let delayed_scale = deployments.get_scale(&workload_name).await.unwrap();
        spec.revision += 1;
        spec.replicas = 0;
        let stop = command(spec.clone(), Action::Stop, Some(uid.clone()));
        let stopped = adapter
            .reconcile(
                &stop,
                &LeaseGuard::new(stop.clone(), Arc::new(LocalAuthority)),
            )
            .await?;
        assert_eq!(stopped.phase, Phase::Stopped);
        assert!(hpas.get_opt(&workload_name).await.unwrap().is_none());
        let stale = deployments.replace_scale(&workload_name, &kube::api::PostParams::default(), &delayed_scale).await;
        assert!(matches!(stale, Err(kube::Error::Api(ref error)) if error.code == 409), "stale in-flight HPA write was not fenced");
        tokio::time::sleep(Duration::from_secs(20)).await;
        assert_eq!(adapter.inspect(&spec).await?.phase, Phase::Stopped);
        println!("{retained}: stopped without HPA; resuming");
        spec.revision += 1;
        spec.replicas = 1;
        let resume = command(spec.clone(), Action::Apply, Some(uid.clone()));
        adapter
            .reconcile(
                &resume,
                &LeaseGuard::new(resume.clone(), Arc::new(LocalAuthority)),
            )
            .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            let value = adapter.inspect(&spec).await?;
            if value.ready
                && value
                    .autoscaling
                    .as_ref()
                    .is_some_and(|s| s.current_replicas == 2)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "scale-up after resume failed"
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        println!("{retained}: resume scaled to maximum");
        let pods: Api<Pod> = Api::namespaced(client.clone(), &namespace);
        let list = pods.list(&ListParams::default()).await.unwrap();
        if !retained {
            let denied=std::process::Command::new("kubectl").args(["--kubeconfig",path.to_str().unwrap(),"--context",&context,"-n",&namespace,"exec",list.items[0].metadata.name.as_deref().unwrap(),"--","wget","-T","2","-qO-",&format!("http://{workload_name}:8080")]).output().unwrap();
            assert!(!denied.status.success(),"unapproved app-to-app probe bypassed ingress policy");
        }
        let mut served = std::collections::BTreeSet::new();
        for _ in 0..20 {
            let output = std::process::Command::new("kubectl")
                .args([
                    "--kubeconfig",
                    path.to_str().unwrap(),
                    "--context",
                    &context,
                    "-n",
                    if retained { &namespace } else { &client_namespace },
                    "exec",
                    if retained { list.items[0].metadata.name.as_deref().unwrap() } else { "probe" },
                    "--",
                    "wget",
                    "-T",
                    "5",
                    "-qO-",
                    &format!("http://{workload_name}.{namespace}.svc.cluster.local:8080"),
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
            served.insert(String::from_utf8(output.stdout).unwrap());
        }
        assert!(served.len() >= 2, "Service did not distribute traffic");
        for pod in list.items {
            let status = std::process::Command::new("kubectl")
                .args([
                    "--kubeconfig",
                    path.to_str().unwrap(),
                    "--context",
                    &context,
                    "-n",
                    &namespace,
                    "exec",
                    pod.metadata.name.as_deref().unwrap(),
                    "--",
                    "sh",
                    "-c",
                    "kill $(cat /tmp/load.pid)",
                ])
                .status()
                .unwrap();
            assert!(status.success());
        }
        println!(
            "{retained}: traffic reached {} replicas; CPU load stopped",
            served.len()
        );
        if !retained {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(480);
            loop {
                let value = adapter.inspect(&spec).await?;
                if value.ready
                    && value
                        .autoscaling
                        .as_ref()
                        .is_some_and(|s| s.current_replicas == 1 && s.metrics_available)
                {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "scale-down did not reach min"
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
        spec.revision += 1;
        spec.autoscaling = None;
        spec.replicas = 2;
        let fixed = command(spec.clone(), Action::Apply, Some(uid));
        assert!(
            adapter
                .reconcile(
                    &fixed,
                    &LeaseGuard::new(fixed.clone(), Arc::new(LocalAuthority))
                )
                .await?
                .ready
        );
        assert!(hpas.get_opt(&workload_name).await.unwrap().is_none());
        Ok(())
    }
    .await;
    let _ = namespaces
        .delete(&namespace, &DeleteParams::default())
        .await;
    let _ = namespaces
        .delete(&client_namespace, &DeleteParams::default())
        .await;
    if result.is_err() {
        eprintln!(
            "Owned fixture Helm diagnostics: {}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
    }
    result.unwrap();
}
