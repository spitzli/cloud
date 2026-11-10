#![cfg(feature = "kubernetes")]
mod support;
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Namespace, Pod, Secret},
};
use kube::{
    Api, Client, Config,
    api::{DeleteParams, PostParams},
    config::{KubeConfigOptions, Kubeconfig},
};
use sha2::{Digest, Sha256};
use spitzli_supervisor::*;
use std::{collections::BTreeMap, sync::Arc};
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
#[tokio::test]
#[ignore = "requires explicit owned disposable Kind context and digest image"]
async fn service_ca_rotation_preservation_and_explicit_clear_reach_real_pods() {
    let kubeconfig = std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap();
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("kind-webdock-rust-native-"));
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap()).unwrap(),
        &KubeConfigOptions {
            context: Some(context.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::try_from(config).unwrap();
    let numeric = u64::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap())
        % 1_000_000_000_000
        + 1000;
    let mut spec = support::spec();
    spec.project_id = spitzli_core::id::RecordId::parse(&numeric.to_string()).unwrap();
    spec.resource_id = spitzli_core::id::RecordId::parse(&(numeric + 1).to_string()).unwrap();
    spec.image = std::env::var("WEBDOCK_KUBE_TEST_IMAGE").unwrap();
    let allowance = ResourceAllowance {
        apps: 2,
        cpu_millicores: 1000,
        memory_bytes: 1073741824,
        volume_bytes: 0,
        ephemeral_bytes: 1073741824,
        replicas_per_app: 1,
        concurrent_deployments: 1,
    };
    spec.hosting_binding = Some(HostingBinding {
        public_route: None,
        cluster_id: spitzli_core::id::RecordId::parse("789").unwrap(),
        quota: allowance.clone(),
        final_quota: allowance,
    });
    let binding = (numeric + 2).to_string();
    let backend = (numeric + 3).to_string();
    let path = format!("/var/run/webdock/services/{binding}/ca.crt");
    spec.arguments = vec![
        "sh".into(),
        "-c".into(),
        format!(
            "if test -f {path}; then sha256sum {path}; else echo cleared; fi; echo ready >/tmp/index.html; exec httpd -f -p 8080 -h /tmp"
        ),
    ];
    let namespace = spitzli_agent::legacy::namespace(&spec);
    let mut destination_ips = vec![];
    for id in [numeric + 3, numeric + 4] {
        let ns = format!("wd-db-{id}");
        let namespaces: Api<Namespace> = Api::all(client.clone());
        namespaces.create(&PostParams::default(),&serde_json::from_value(serde_json::json!({"metadata":{"name":ns,"labels":{"dev.webdock.test":"service-binding-fixture","pod-security.kubernetes.io/enforce":"restricted"}}})).unwrap()).await.unwrap();
        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        let pod:Pod=serde_json::from_value(serde_json::json!({"metadata":{"name":"postgres-probe","labels":{"cnpg.io/cluster":"postgres"}},"spec":{"automountServiceAccountToken":false,"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"probe","image":spec.image,"command":["sh","-c","echo fixture >/tmp/index.html; httpd -p 5433 -h /tmp; exec httpd -f -p 5432 -h /tmp"],"resources":{"requests":{"cpu":"10m","memory":"16777216"},"limits":{"cpu":"100m","memory":"33554432"}},"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"volumeMounts":[{"name":"tmp","mountPath":"/tmp"}]}],"volumes":[{"name":"tmp","emptyDir":{"medium":"Memory","sizeLimit":"16777216"}}]}})).unwrap();
        pods.create(&PostParams::default(), &pod).await.unwrap();
        let mut found = None;
        for _ in 0..120 {
            let p = pods.get("postgres-probe").await.unwrap();
            if p.status
                .as_ref()
                .is_some_and(|s| s.phase.as_deref() == Some("Running"))
            {
                found = p.status.and_then(|s| s.pod_ip);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        destination_ips.push(found.expect("owned service probe must run"));
    }
    let task = client.clone();
    let result = tokio::spawn(async move {
        let secrets: Api<Secret> = Api::namespaced(task.clone(), &namespace);
        let deployments: Api<Deployment> = Api::namespaced(task.clone(), &namespace);
        let mut expected = None;
        let mut expected_hash = String::new();
        for revision in 1..=4 {
            spec.revision = revision;

            let (refs, materials) = if revision <= 2 {
                let ca = rcgen::generate_simple_self_signed(vec!["fixture.database.test".into()])
                    .unwrap()
                    .cert
                    .pem();
                expected_hash = format!("{:x}", Sha256::digest(ca.as_bytes()));
                (
                    Some(vec![ServiceBindingReference::Postgres {
                        id: binding.clone(),
                        revision,
                        credential_revision: revision + 4,
                        backend_id: backend.clone(),
                        ca_sha256: expected_hash.clone(),
                    }]),
                    Some(vec![ServiceBindingMaterial::Postgres {
                        id: binding.clone(),
                        revision,
                        credential_revision: revision + 4,
                        ca_certificate: ca,
                    }]),
                )
            } else if revision == 3 {
                (None, None)
            } else {
                (Some(vec![]), Some(vec![]))
            };
            spec.hosted_access = Some(HostedAccess {
                environment: Some(EnvironmentReference {
                    revision,
                    source_sha256: format!("{revision:064x}"),
                }),
                service_bindings: refs,
            });
            let mut command = Command {
                operation_id: format!("services-{revision}"),
                generation: 1,
                lease_until: timestamp().unwrap() + 300,
                authorization: "fixture".into(),
                action: Action::Apply,
                spec: spec.clone(),
                expected_runtime_id: expected.clone(),
                material: None,
            };
            command.material = Some(RuntimeMaterial {
                operation_id: command.operation_id.clone(),
                request_sha256: command.fingerprint().unwrap(),
                environment: Some(EnvironmentMaterial {
                    source_sha256: format!("{revision:064x}"),
                    values: BTreeMap::new(),
                }),
                service_bindings: materials,
            });
            let observed = spitzli_agent::legacy::reconcile(
                task.clone(),
                &command,
                &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap_or_else(|error| panic!("service revision {revision}: {error}"));
            assert!(observed.ready);
            if expected.is_some() {
                assert_eq!(expected, observed.runtime_id);
            }
            expected = observed.runtime_id;
            if revision == 1 || revision == 4 {
                let pods: Api<Pod> = Api::namespaced(task.clone(), &namespace);
                let pods = pods
                    .list(
                        &kube::api::ListParams::default()
                            .labels(&format!("webdock.dev/app-id={}", spec.resource_id)),
                    )
                    .await
                    .unwrap();
                assert_eq!(pods.items.len(), 1);
                let pod = pods.items[0].metadata.name.as_deref().unwrap();
                for (ip, port, allowed) in [
                    (&destination_ips[0], 5432, revision == 1),
                    (&destination_ips[0], 5433, false),
                    (&destination_ips[1], 5432, false),
                ] {
                    let output = tokio::process::Command::new("kubectl")
                        .args([
                            "--kubeconfig",
                            &kubeconfig,
                            "--context",
                            &context,
                            "exec",
                            "-n",
                            &namespace,
                            pod,
                            "--",
                            "wget",
                            "-T",
                            "2",
                            "-qO-",
                            &format!("http://{ip}:{port}/"),
                        ])
                        .output()
                        .await
                        .unwrap();
                    assert_eq!(
                        output.status.success(),
                        allowed,
                        "service revision {revision}, destination {ip}:{port}"
                    );
                }
            }
            let logs = spitzli_agent::legacy::logs(task.clone(), &spec, 20)
                .await
                .unwrap();
            if revision < 4 {
                assert_eq!(logs, format!("{expected_hash}  {path}\n"));
            } else {
                assert_eq!(logs, "cleared\n");
            }
            let deployment = deployments
                .get(&spitzli_agent::legacy::name(&spec))
                .await
                .unwrap();
            let pod = deployment.spec.unwrap().template.spec.unwrap();
            let volume = pod
                .volumes
                .unwrap_or_default()
                .into_iter()
                .find(|v| v.name == "services");
            if revision < 4 {
                assert_eq!(
                    volume.unwrap().secret.unwrap().secret_name.unwrap(),
                    format!(
                        "{}-services-{}",
                        spitzli_agent::legacy::name(&spec),
                        revision.min(2)
                    )
                );
            } else {
                assert!(volume.is_none());
            }
            if revision <= 2 {
                let secret = secrets
                    .get(&format!(
                        "{}-services-{revision}",
                        spitzli_agent::legacy::name(&spec)
                    ))
                    .await
                    .unwrap();
                assert_eq!(secret.immutable, Some(true));
            }
            if revision == 1 {
                // Original service resources had labels/versioned names but no revision annotation.
                secrets
                    .patch(
                        &format!("{}-services-1", spitzli_agent::legacy::name(&spec)),
                        &kube::api::PatchParams::default(),
                        &kube::api::Patch::Merge(
                            serde_json::json!({"metadata":{"annotations":null}}),
                        ),
                    )
                    .await
                    .unwrap();
                let policies: Api<k8s_openapi::api::networking::v1::NetworkPolicy> =
                    Api::namespaced(task.clone(), &namespace);
                policies
                    .patch(
                        &format!("{}-services", spitzli_agent::legacy::name(&spec)),
                        &kube::api::PatchParams::default(),
                        &kube::api::Patch::Merge(
                            serde_json::json!({"metadata":{"annotations":null}}),
                        ),
                    )
                    .await
                    .unwrap();
            }
            if revision == 2 {
                assert!(
                    secrets
                        .get_opt(&format!(
                            "{}-services-1",
                            spitzli_agent::legacy::name(&spec)
                        ))
                        .await
                        .unwrap()
                        .is_none()
                );
            }
            if revision == 3 {
                assert!(
                    secrets
                        .get_opt(&format!(
                            "{}-services-2",
                            spitzli_agent::legacy::name(&spec)
                        ))
                        .await
                        .unwrap()
                        .is_some()
                );
            }
            if revision == 4 {
                assert!(
                    secrets
                        .get_opt(&format!(
                            "{}-services-2",
                            spitzli_agent::legacy::name(&spec)
                        ))
                        .await
                        .unwrap()
                        .is_none()
                );
            }
        }
    })
    .await;
    let namespaces: Api<Namespace> = Api::all(client);
    let _ = namespaces
        .delete(&format!("wd-{numeric}"), &DeleteParams::default())
        .await;
    for id in [numeric + 3, numeric + 4] {
        let _ = namespaces
            .delete(&format!("wd-db-{id}"), &DeleteParams::default())
            .await;
    }
    result.unwrap();
}
