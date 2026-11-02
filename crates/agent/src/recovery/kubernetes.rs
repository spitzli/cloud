//! Observed offline source capture; no existing cluster is selected implicitly.
use super::config::KubernetesSource;
use crate::storage::StorageGuard;
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{PersistentVolumeClaim, Pod},
};
use kube::{
    Api, Client,
    api::{AttachParams, DeleteParams, ListParams, Patch, PatchParams, PostParams, Preconditions},
    config::{KubeConfigOptions, Kubeconfig},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};
use webdock_supervisor::{ExecutionError, Result};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoppedDeployment {
    pub namespace: String,
    pub name: String,
    pub uid: String,
    pub resource_version: String,
    pub replicas: i32,
    pub labels: BTreeMap<String, String>,
    pub ce: bool,
    pub customer_id: Option<String>,
    pub claim: Option<String>,
    pub image: Option<String>,
    pub data_mount: Option<String>,
    pub template_sha256: String,
    pub service_revision: Option<i64>,
}
async fn observe(
    client: Client,
    namespace: &str,
    name: &str,
    owner: BTreeMap<String, String>,
    ce: bool,
    customer_id: Option<String>,
) -> Result<StoppedDeployment> {
    let deployment = Api::<Deployment>::namespaced(client, namespace)
        .get(name)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if deployment.metadata.deletion_timestamp.is_some()
        || !owner
            .iter()
            .all(|(k, v)| deployment.metadata.labels.as_ref().and_then(|l| l.get(k)) == Some(v))
    {
        return Err(ExecutionError::Ownership);
    }
    replica_manager(&deployment)?;
    let spec = deployment.spec.as_ref().ok_or(ExecutionError::Ownership)?;
    let pod = spec
        .template
        .spec
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    let container = pod.containers.first().ok_or(ExecutionError::Ownership)?;
    let mount = container.volume_mounts.as_ref().and_then(|mounts| {
        mounts
            .iter()
            .find(|m| ["/data", "/var/lib/stalwart"].contains(&m.mount_path.as_str()))
    });
    let claim = mount.and_then(|m| {
        pod.volumes
            .as_ref()
            .and_then(|vs| vs.iter().find(|v| v.name == m.name))
            .and_then(|v| v.persistent_volume_claim.as_ref())
            .map(|v| v.claim_name.clone())
    });
    Ok(StoppedDeployment {
        namespace: namespace.into(),
        name: name.into(),
        uid: deployment.metadata.uid.ok_or(ExecutionError::Ownership)?,
        resource_version: deployment
            .metadata
            .resource_version
            .ok_or(ExecutionError::Ownership)?,
        replicas: spec.replicas.unwrap_or(1),
        labels: owner,
        ce,
        customer_id,
        claim,
        image: container.image.clone(),
        data_mount: mount.map(|m| m.mount_path.clone()),
        template_sha256: super::config::fingerprint(spec)?,
        service_revision: None,
    })
}
pub async fn observe_mail(
    client: Client,
    config: &KubernetesSource,
    installation: &str,
    plan: &Value,
) -> Result<Vec<StoppedDeployment>> {
    let mut observed = Vec::new();
    for runtime in plan["source"]["runtimes"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?
    {
        let namespace = runtime["namespace"]
            .as_str()
            .ok_or(ExecutionError::Ownership)?;
        let owner = BTreeMap::from([
            ("webdock.dev/platform".into(), config.platform_id.clone()),
            ("webdock.dev/plugin".into(), "mail".into()),
            (
                "webdock.dev/runtime-generation".into(),
                runtime["generation"]
                    .as_u64()
                    .ok_or(ExecutionError::Ownership)?
                    .to_string(),
            ),
        ]);
        observed.push(observe(client.clone(), namespace, "runtime", owner, false, None).await?);
    }
    for workload in plan["source"]["workloads"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?
    {
        let namespace = workload["namespace"]
            .as_str()
            .ok_or(ExecutionError::Ownership)?;
        let unit = workload["unit"].as_str().ok_or(ExecutionError::Ownership)?;
        let owner = BTreeMap::from([
            ("webdock.dev/plugin".into(), "mail".into()),
            ("webdock.dev/unit".into(), unit.into()),
            ("webdock.dev/managed-by".into(), "webdock".into()),
        ]);
        let objects = Api::<Deployment>::namespaced(client.clone(), namespace)
            .list(
                &ListParams::default().labels(
                    &owner
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            )
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if objects.items.len() != 1 {
            return Err(ExecutionError::Ownership);
        }
        let name = objects.items[0]
            .metadata
            .name
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        observed.push(
            observe(
                client.clone(),
                namespace,
                name,
                owner,
                true,
                Some(
                    workload["customerID"]
                        .as_str()
                        .ok_or(ExecutionError::Ownership)?
                        .into(),
                ),
            )
            .await?,
        );
    }
    for service in plan["source"]["services"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?
    {
        let Some(uid) = service["nativeRuntimeID"].as_str() else {
            continue;
        };
        let customer = service["customerID"]
            .as_str()
            .filter(|id| !id.is_empty() && id.bytes().all(|v| v.is_ascii_digit()))
            .ok_or(ExecutionError::Ownership)?;
        let namespace = format!("wd-mail-{customer}");
        let name = format!("app-{customer}");
        let owner = BTreeMap::from([
            ("security.webdock.dev/managed-by".into(), "webdock".into()),
            ("webdock.dev/app-id".into(), customer.into()),
            ("webdock.dev/project-id".into(), customer.into()),
            (
                "webdock.dev/cluster-id".into(),
                service["hostID"]
                    .as_str()
                    .ok_or(ExecutionError::Ownership)?
                    .into(),
            ),
            ("webdock.dev/mail-installation".into(), installation.into()),
        ]);
        let mut native = observe(
            client.clone(),
            &namespace,
            &name,
            owner,
            true,
            Some(customer.into()),
        )
        .await?;
        if native.uid != uid {
            return Err(ExecutionError::Ownership);
        }
        native.service_revision = Some(
            service["revision"]
                .as_i64()
                .ok_or(ExecutionError::Ownership)?,
        );
        observed.push(native);
    }
    Ok(observed)
}
pub async fn stop_observed(
    client: Client,
    observed: &[StoppedDeployment],
    guard: &StorageGuard,
) -> Result<Vec<StoppedDeployment>> {
    let mut stopped = Vec::new();
    for original in observed {
        let mut result = stop(client.clone(), original, guard).await?;
        result.replicas = original.replicas;
        stopped.push(result);
    }
    Ok(stopped)
}
fn replica_manager(deployment: &Deployment) -> Result<String> {
    let managers = deployment
        .metadata
        .managed_fields
        .as_ref()
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry.fields_v1.as_ref().is_some_and(|fields| {
                fields
                    .0
                    .get("f:spec")
                    .and_then(|spec| spec.get("f:replicas"))
                    .is_some()
            })
        })
        .filter_map(|entry| entry.manager.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    if managers.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let manager = *managers.first().ok_or(ExecutionError::Ownership)?;
    if ![
        "webdock",
        "webdock-workloads",
        "webdock-plugin-runtime",
        "webdock-agent",
        "kube-client",
    ]
    .contains(&manager)
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(manager.into())
}
pub async fn connect(config: &KubernetesSource) -> Result<Client> {
    super::config::private(&config.kubeconfig, 1024 * 1024)?;
    let kubeconfig =
        Kubeconfig::read_from(&config.kubeconfig).map_err(|_| ExecutionError::Ownership)?;
    let options = KubeConfigOptions {
        context: Some(config.context.clone()),
        ..Default::default()
    };
    let mut config = kube::Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    config.connect_timeout = Some(Duration::from_secs(5));
    config.read_timeout = Some(Duration::from_secs(15));
    config.write_timeout = Some(Duration::from_secs(15));
    Client::try_from(config).map_err(|_| ExecutionError::Runtime)
}
async fn stop(
    client: Client,
    original: &StoppedDeployment,
    guard: &StorageGuard,
) -> Result<StoppedDeployment> {
    let namespace = original.namespace.as_str();
    let name = original.name.as_str();
    let owner = &original.labels;
    guard.check()?;
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), namespace);
    let d = deployments
        .get(name)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let uid = d.metadata.uid.clone().ok_or(ExecutionError::Ownership)?;
    if uid != original.uid
        || d.metadata.resource_version.as_deref() != Some(&original.resource_version)
        || super::config::fingerprint(d.spec.as_ref().ok_or(ExecutionError::Ownership)?)?
            != original.template_sha256
        || d.metadata.deletion_timestamp.is_some()
        || !owner
            .iter()
            .all(|(k, v)| d.metadata.labels.as_ref().and_then(|labels| labels.get(k)) == Some(v))
    {
        return Err(ExecutionError::Ownership);
    }
    let spec = d.spec.as_ref().ok_or(ExecutionError::Ownership)?;
    let template = spec
        .template
        .spec
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    let primary = template
        .containers
        .first()
        .ok_or(ExecutionError::Ownership)?;
    let data = primary.volume_mounts.as_ref().and_then(|mounts| {
        mounts
            .iter()
            .find(|v| ["/data", "/var/lib/stalwart"].contains(&v.mount_path.as_str()))
    });
    let claim = data.and_then(|m| {
        template
            .volumes
            .as_ref()
            .and_then(|volumes| volumes.iter().find(|v| v.name == m.name))
            .and_then(|v| v.persistent_volume_claim.as_ref())
            .map(|v| v.claim_name.clone())
    });
    let rv = d
        .metadata
        .resource_version
        .clone()
        .ok_or(ExecutionError::Ownership)?;
    let manager = replica_manager(&d)?;
    deployments
        .patch(
            name,
            &PatchParams {
                field_manager: Some(manager),
                ..Default::default()
            },
            &Patch::Merge(
                json!({"metadata":{"uid":uid,"resourceVersion":rv},"spec":{"replicas":0}}),
            ),
        )
        .await
        .map_err(|_| ExecutionError::Fenced)?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        guard.check()?;
        let active = pods
            .list(&ListParams::default())
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let mut consumers = false;
        for pod in &active.items {
            let own = pod.metadata.labels.as_ref().is_some_and(|labels| {
                spec.selector
                    .match_labels
                    .as_ref()
                    .unwrap()
                    .iter()
                    .all(|(k, v)| labels.get(k) == Some(v))
            });
            let volume = claim.as_ref().is_some_and(|claim| {
                pod.spec
                    .as_ref()
                    .and_then(|p| p.volumes.as_ref())
                    .is_some_and(|vs| {
                        vs.iter().any(|v| {
                            v.persistent_volume_claim
                                .as_ref()
                                .is_some_and(|p| &p.claim_name == claim)
                        })
                    })
            });
            if own || volume {
                consumers = true;
            }
        }
        if !consumers {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ExecutionError::Expired);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let after = deployments
        .get(name)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if after.metadata.uid.as_deref() != Some(&uid)
        || after.spec.as_ref().and_then(|s| s.replicas) != Some(0)
    {
        return Err(ExecutionError::Fenced);
    }
    Ok(StoppedDeployment {
        namespace: namespace.into(),
        name: name.into(),
        uid,
        resource_version: after
            .metadata
            .resource_version
            .ok_or(ExecutionError::Ownership)?,
        replicas: spec.replicas.unwrap_or(1),
        labels: owner.clone(),
        ce: original.ce,
        customer_id: original.customer_id.clone(),
        claim,
        image: primary.image.clone(),
        data_mount: data.map(|m| m.mount_path.clone()),
        template_sha256: super::config::fingerprint(spec)?,
        service_revision: original.service_revision,
    })
}
pub async fn resume(
    client: Client,
    stopped: &StoppedDeployment,
    guard: &StorageGuard,
) -> Result<()> {
    guard.check()?;
    let api: Api<Deployment> = Api::namespaced(client, &stopped.namespace);
    let current = api
        .get(&stopped.name)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if current.metadata.uid.as_deref() != Some(&stopped.uid)
        || current.metadata.resource_version.as_deref() != Some(&stopped.resource_version)
        || current.spec.as_ref().and_then(|s| s.replicas) != Some(0)
    {
        return Err(ExecutionError::Fenced);
    }
    let manager = replica_manager(&current)?;
    api.patch(&stopped.name,&PatchParams{field_manager:Some(manager),..Default::default()},&Patch::Merge(json!({"metadata":{"uid":stopped.uid,"resourceVersion":stopped.resource_version},"spec":{"replicas":stopped.replicas}}))).await.map_err(|_|ExecutionError::Fenced)?;
    Ok(())
}
async fn offline(client: Client, stopped: &StoppedDeployment, helper: Option<&str>) -> Result<()> {
    let deployment = Api::<Deployment>::namespaced(client.clone(), &stopped.namespace)
        .get(&stopped.name)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if deployment.metadata.uid.as_deref() != Some(&stopped.uid)
        || deployment.spec.as_ref().and_then(|s| s.replicas) != Some(0)
        || !stopped
            .labels
            .iter()
            .all(|(k, v)| deployment.metadata.labels.as_ref().and_then(|l| l.get(k)) == Some(v))
    {
        return Err(ExecutionError::Fenced);
    }
    let claim = stopped.claim.as_deref().ok_or(ExecutionError::Ownership)?;
    let pods = Api::<Pod>::namespaced(client, &stopped.namespace)
        .list(&ListParams::default())
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if pods.items.iter().any(|pod| {
        pod.metadata.uid.as_deref() != helper
            && pod
                .spec
                .as_ref()
                .and_then(|s| s.volumes.as_ref())
                .is_some_and(|volumes| {
                    volumes.iter().any(|v| {
                        v.persistent_volume_claim
                            .as_ref()
                            .is_some_and(|p| p.claim_name == claim)
                    })
                })
    }) {
        return Err(ExecutionError::Fenced);
    }
    Ok(())
}
async fn delete_snapshot_helper(pods: &Api<Pod>, name: &str, uid: &str) -> Result<()> {
    let actual = pods.get(name).await.map_err(|_| ExecutionError::Runtime)?;
    if actual.metadata.uid.as_deref() != Some(uid) {
        return Err(ExecutionError::Ownership);
    }
    pods.delete(
        name,
        &DeleteParams {
            // The completed read-only helper has no application state to drain.
            grace_period_seconds: Some(0),
            preconditions: Some(Preconditions {
                uid: Some(uid.to_owned()),
                resource_version: actual.metadata.resource_version,
            }),
            ..Default::default()
        },
    )
    .await
    .map_err(|_| ExecutionError::Runtime)?;
    let cleanup_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match pods.get(name).await {
            Err(kube::Error::Api(error)) if error.code == 404 => break,
            Ok(pod) if pod.metadata.uid.as_deref() == Some(uid) => {}
            _ => return Err(ExecutionError::Ownership),
        }
        if tokio::time::Instant::now() >= cleanup_deadline {
            return Err(ExecutionError::Expired);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

pub async fn snapshot_ce(
    client: Client,
    config: &KubernetesSource,
    stopped: &StoppedDeployment,
    output: &Path,
    capacity: u64,
    guard: &StorageGuard,
) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    guard.check()?;
    offline(client.clone(), stopped, None).await?;
    let claim = stopped.claim.as_deref().ok_or(ExecutionError::Ownership)?;
    let claims: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), &stopped.namespace);
    let pvc = claims
        .get(claim)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let claim_uid = pvc.metadata.uid.ok_or(ExecutionError::Ownership)?;
    let name = format!("recovery-offline-{}", uuid::Uuid::new_v4().simple());
    let labels: BTreeMap<String, String> =
        BTreeMap::from([("webdock.dev/recovery-helper".into(), name.clone())]);
    let pod:Pod=serde_json::from_value(json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":name,"namespace":stopped.namespace,"labels":labels},"spec":{"automountServiceAccountToken":false,"restartPolicy":"Never","activeDeadlineSeconds":150,"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"snapshot","image":config.helper_image,"command":["/bin/sleep","150"],"securityContext":{"readOnlyRootFilesystem":true,"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"100m","memory":"128Mi"},"limits":{"cpu":"500m","memory":"128Mi","ephemeral-storage":"16Mi"}},"volumeMounts":[{"name":"data","mountPath":"/snapshot","readOnly":true}]}],"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":claim,"readOnly":true}}]}})).map_err(|_|ExecutionError::Runtime)?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), &stopped.namespace);
    let created = pods
        .create(&PostParams::default(), &pod)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let uid = created.metadata.uid.ok_or(ExecutionError::Ownership)?;
    let result = async {
        let limit = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            guard.check()?;
            let p = pods.get(&name).await.map_err(|_| ExecutionError::Runtime)?;
            if p.metadata.uid.as_deref() != Some(&uid) {
                return Err(ExecutionError::Ownership);
            }
            if p.status.as_ref().is_some_and(|s| {
                s.conditions
                    .as_ref()
                    .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
            }) {
                break;
            }
            if tokio::time::Instant::now() >= limit {
                return Err(ExecutionError::Expired);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if claims
            .get(claim)
            .await
            .map_err(|_| ExecutionError::Runtime)?
            .metadata
            .uid
            .as_deref()
            != Some(&claim_uid)
        {
            return Err(ExecutionError::Ownership);
        }
        offline(client.clone(), stopped, Some(&uid)).await?;
        let mut execution = pods
            .exec(
                &name,
                ["/bin/tar", "-C", "/snapshot", "-cf", "-", "."],
                &AttachParams::default().stdout(true).stderr(false),
            )
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let status = execution.take_status().ok_or(ExecutionError::Runtime)?;
        let mut source = execution.stdout().ok_or(ExecutionError::Runtime)?;
        let mut target = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(output)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let mut total = 0;
        let mut buffer = [0; 65536];
        loop {
            guard.check()?;
            let n = source
                .read(&mut buffer)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > capacity {
                return Err(ExecutionError::Invalid("CE snapshot capacity"));
            }
            target
                .write_all(&buffer[..n])
                .await
                .map_err(|_| ExecutionError::Runtime)?;
        }
        target
            .sync_all()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        offline(client.clone(), stopped, Some(&uid)).await?;
        if status
            .await
            .is_none_or(|status| status.status.as_deref() != Some("Success"))
        {
            return Err(ExecutionError::Runtime);
        }
        execution
            .join()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        Ok(())
    }
    .await;
    delete_snapshot_helper(&pods, &name, &uid).await?;
    // A terminated helper or another pod still naming the PVC is not an offline set.
    offline(client, stopped, None).await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, atomic::AtomicBool};
    struct Provider {
        deployment: Value,
        pods: Value,
        patches: Vec<Value>,
    }
    async fn provider(
        axum::extract::State(state): axum::extract::State<Arc<Mutex<Provider>>>,
        request: axum::extract::Request,
    ) -> axum::Json<Value> {
        let patch = request.method() == axum::http::Method::PATCH;
        let pods = request.uri().path().ends_with("/pods");
        let body = axum::body::to_bytes(request.into_body(), 65536)
            .await
            .unwrap();
        let mut state = state.lock().unwrap();
        if patch {
            let value: Value = serde_json::from_slice(&body).unwrap();
            state.patches.push(value.clone());
            state.deployment["spec"]["replicas"] = value["spec"]["replicas"].clone();
            state.deployment["metadata"]["resourceVersion"] = json!("2");
        }
        axum::Json(if pods {
            state.pods.clone()
        } else {
            state.deployment.clone()
        })
    }
    async fn fixture() -> (Client, Arc<Mutex<Provider>>, tokio::task::JoinHandle<()>) {
        let state = Arc::new(Mutex::new(Provider {
            deployment: json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"engine","namespace":"owned","uid":"owned-deployment","resourceVersion":"1","labels":{"webdock.dev/plugin":"mail"},"managedFields":[{"manager":"webdock-workloads","operation":"Apply","apiVersion":"apps/v1","fieldsType":"FieldsV1","fieldsV1":{"f:spec":{"f:replicas":{}}}}]},"spec":{"replicas":1,"selector":{"matchLabels":{"app":"engine"}},"template":{"metadata":{"labels":{"app":"engine"}},"spec":{"containers":[{"name":"app","image":"example.test/ce@sha256:abc","volumeMounts":[{"name":"data","mountPath":"/var/lib/stalwart"}]}],"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"data"}}]}}}}),
            pods: json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[]}),
            patches: Vec::new(),
        }));
        let router = axum::Router::new()
            .fallback(provider)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        (client, state, server)
    }
    // Kubernetes keeps a terminating Pod for its requested grace period. The
    // read-only sleep helper has no writer to drain after tar has completed.
    #[tokio::test]
    async fn snapshot_helper_cleanup_does_not_spend_its_deadline_on_default_grace() {
        use axum::{
            extract::{Request, State},
            http::StatusCode,
            response::IntoResponse,
        };
        let removed = Arc::new(Mutex::new(false));
        let state = removed.clone();
        let router = axum::Router::new().fallback(move |State(state): State<Arc<Mutex<bool>>>, request: Request| async move {
            if request.method() == axum::http::Method::DELETE {
                let body = axum::body::to_bytes(request.into_body(), 4096).await.unwrap();
                let params: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(params["preconditions"]["uid"], "own-helper");
                assert_eq!(params["preconditions"]["resourceVersion"], "1");
                *state.lock().unwrap() = params["gracePeriodSeconds"] == 0;
                return axum::Json(json!({"apiVersion":"v1","kind":"Status","status":"Success"})).into_response();
            }
            if *state.lock().unwrap() {
                (StatusCode::NOT_FOUND, axum::Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404}))).into_response()
            } else {
                axum::Json(json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":"helper","namespace":"owned","uid":"own-helper","resourceVersion":"1"},"spec":{"containers":[]}})).into_response()
            }
        }).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::try_from(kube::Config::new(
            format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
        ))
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let pods = Api::<Pod>::namespaced(client, "owned");
        let result = tokio::time::timeout(
            Duration::from_millis(300),
            delete_snapshot_helper(&pods, "helper", "own-helper"),
        )
        .await;
        server.abort();
        assert!(
            result.is_ok_and(|result| result.is_ok()),
            "default Kubernetes grace must not consume the cleanup deadline"
        );
        assert!(*removed.lock().unwrap());
    }

    #[tokio::test]
    async fn exact_observation_is_required_before_conditional_stop_without_ssa_takeover() {
        let (client, provider, server) = fixture().await;
        let original = observe(
            client.clone(),
            "owned",
            "engine",
            BTreeMap::from([("webdock.dev/plugin".into(), "mail".into())]),
            true,
            Some("123".into()),
        )
        .await
        .unwrap();
        let guard = StorageGuard::new(
            chrono::Utc::now() + chrono::Duration::seconds(30),
            Arc::new(AtomicBool::new(false)),
        );
        provider.lock().unwrap().deployment["metadata"]["resourceVersion"] = json!("changed");
        assert!(
            stop_observed(client.clone(), std::slice::from_ref(&original), &guard)
                .await
                .is_err()
        );
        assert!(provider.lock().unwrap().patches.is_empty());
        provider.lock().unwrap().deployment["metadata"]["resourceVersion"] = json!("1");
        provider.lock().unwrap().deployment["spec"]["template"]["spec"]["containers"][0]["image"] =
            json!("changed");
        assert!(
            stop_observed(client.clone(), &[original], &guard)
                .await
                .is_err()
        );
        assert!(provider.lock().unwrap().patches.is_empty());
        let current = observe(
            client.clone(),
            "owned",
            "engine",
            BTreeMap::from([("webdock.dev/plugin".into(), "mail".into())]),
            true,
            Some("123".into()),
        )
        .await
        .unwrap();
        let stopped = stop_observed(client, &[current], &guard).await.unwrap();
        {
            let state = provider.lock().unwrap();
            assert_eq!(state.patches.len(), 1);
            assert_eq!(
                state.patches[0],
                json!({"metadata":{"uid":"owned-deployment","resourceVersion":"1"},"spec":{"replicas":0}})
            );
            assert_eq!(stopped[0].replicas, 1);
            assert_eq!(stopped[0].data_mount.as_deref(), Some("/var/lib/stalwart"));
        }
        server.abort();
        let _ = server.await;
    }
    #[tokio::test]
    async fn offline_copy_rejects_other_pvc_pods_even_when_terminal_or_readonly() {
        let (client, provider, server) = fixture().await;
        let mut observed = observe(
            client.clone(),
            "owned",
            "engine",
            BTreeMap::from([("webdock.dev/plugin".into(), "mail".into())]),
            true,
            Some("123".into()),
        )
        .await
        .unwrap();
        provider.lock().unwrap().deployment["spec"]["replicas"] = json!(0);
        observed.replicas = 0;
        assert!(offline(client.clone(), &observed, None).await.is_ok());
        provider.lock().unwrap().pods["items"] = json!([{"metadata":{"name":"other","uid":"foreign-pod"},"spec":{"containers":[],"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"data","readOnly":true}}]},"status":{"phase":"Succeeded"}}]);
        assert!(offline(client.clone(), &observed, None).await.is_err());
        assert!(
            offline(client.clone(), &observed, Some("different-helper"))
                .await
                .is_err()
        );
        assert!(
            offline(client, &observed, Some("foreign-pod"))
                .await
                .is_ok()
        );
        server.abort();
        let _ = server.await;
    }
}
