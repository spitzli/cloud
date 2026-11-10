#![cfg(feature = "kubernetes")]
mod support;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::{
    Api, Client, Config,
    api::{DeleteParams, PostParams},
    config::{KubeConfigOptions, Kubeconfig},
};
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
async fn immutable_environment_rotates_clears_and_preserves_other_scoped_secrets() {
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("kind-webdock-rust-"));
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap()).unwrap(),
        &KubeConfigOptions {
            context: Some(context),
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
    spec.arguments=vec!["sh".into(),"-c".into(),"printf '%s\\n' \"$FIXTURE_TOKEN\"; printf %s \"$FIXTURE_TOKEN\" >/tmp/index.html; exec httpd -f -p 8080 -h /tmp".into()];
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
    let namespace = spitzli_agent::legacy::namespace(&spec);
    let task_client = client.clone();
    let task_namespace = namespace.clone();
    let result = tokio::spawn(async move {
        let secrets: Api<Secret> = Api::namespaced(task_client.clone(), &task_namespace);
        let mut expected = None;
        for (revision, value, replicas) in [
            (1, Some("fixture-first"), 0),
            (2, Some("fixture-second"), 0),
            (3, Some("fixture-third"), 1),
            (4, None, 1),
        ] {
            spec.revision = revision;
            spec.replicas = replicas;
            spec.hosted_access = Some(HostedAccess {
                service_bindings: None,
                environment: Some(EnvironmentReference {
                    revision,
                    source_sha256: format!("{revision:064x}"),
                }),
            });
            let mut command = Command {
                operation_id: format!("env-{revision}"),
                generation: 1,
                lease_until: timestamp().unwrap() + 300,
                authorization: "fixture".into(),
                action: Action::Apply,
                spec: spec.clone(),
                expected_runtime_id: expected.clone(),
                material: None,
            };
            command.material = Some(RuntimeMaterial {
                service_bindings: None,
                operation_id: command.operation_id.clone(),
                request_sha256: command.fingerprint().unwrap(),
                environment: Some(EnvironmentMaterial {
                    source_sha256: format!("{revision:064x}"),
                    values: value
                        .map(|v| BTreeMap::from([("FIXTURE_TOKEN".into(), v.into())]))
                        .unwrap_or_default(),
                }),
            });
            let observed = spitzli_agent::legacy::reconcile(
                task_client.clone(),
                &command,
                &LeaseGuard::new(command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap();
            if replicas == 0 {
                assert_eq!(observed.phase, Phase::Stopped);
            } else {
                assert!(observed.ready);
            }
            if expected.is_some() {
                assert_eq!(observed.runtime_id, expected);
            }
            expected = observed.runtime_id;
            if replicas > 0 {
                assert_eq!(
                    spitzli_agent::legacy::logs(task_client.clone(), &spec, 10)
                        .await
                        .unwrap(),
                    format!("{}\n", value.unwrap_or(""))
                );
            }
            let secret_name = format!("{}-env-{revision}", spitzli_agent::legacy::name(&spec));
            if let Some(value) = value {
                let secret = secrets.get(&secret_name).await.unwrap();
                assert_eq!(secret.immutable, Some(true));
                assert_eq!(secret.data.unwrap()["FIXTURE_TOKEN"].0, value.as_bytes());
            }
            if revision == 1 {
                let other = Secret {
                    metadata: kube::core::ObjectMeta {
                        name: Some(format!("{}-services-1", spitzli_agent::legacy::name(&spec))),
                        labels: Some(spitzli_agent::legacy::owner_labels(&spec, true).unwrap()),
                        ..Default::default()
                    },
                    string_data: Some(BTreeMap::from([("ca".into(), "fixture-ca".into())])),
                    ..Default::default()
                };
                secrets
                    .create(&PostParams::default(), &other)
                    .await
                    .unwrap();
            } else {
                assert!(
                    secrets
                        .get_opt(&format!(
                            "{}-env-{}",
                            spitzli_agent::legacy::name(&spec),
                            revision - 1
                        ))
                        .await
                        .unwrap()
                        .is_none()
                );
                secrets
                    .get(&format!(
                        "{}-services-1",
                        spitzli_agent::legacy::name(&spec)
                    ))
                    .await
                    .unwrap();
            }
        }
    })
    .await;
    let namespaces: Api<Namespace> = Api::all(client);
    let _ = namespaces
        .delete(&namespace, &DeleteParams::default())
        .await;
    result.unwrap();
}
