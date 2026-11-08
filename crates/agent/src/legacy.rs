//! Preserve the existing wd-project/app-ID layout and retained resources without Helm adoption.
mod bootstrap;
mod environment;
mod services;
pub use services::objects as service_binding_objects;
mod lifecycle;
pub use lifecycle::inspect_completed;
mod storage;
mod storage_attachment;
pub use bootstrap::new_workload;
use k8s_openapi::{
    api::{
        apps::v1::Deployment,
        core::v1::{
            Capabilities as PodCapabilities, ContainerPort, HTTPGetAction, Namespace,
            PersistentVolume, PersistentVolumeClaim, Pod, PodSecurityContext, Probe, ResourceQuota,
            ResourceQuotaSpec, ResourceRequirements, SeccompProfile, SecurityContext, Service,
        },
    },
    apimachinery::pkg::{api::resource::Quantity, util::intstr::IntOrString},
};
use kube::{
    Api, Client,
    api::{ListParams, LogParams, PostParams},
    core::ObjectMeta,
};
use std::{collections::BTreeMap, time::Duration};
use webdock_supervisor::*;
pub fn namespace(spec: &RuntimeSpec) -> String {
    format!("wd-{}", spec.project_id)
}
pub fn name(spec: &RuntimeSpec) -> String {
    format!("app-{}", spec.resource_id)
}
pub fn owner_labels(spec: &RuntimeSpec, app: bool) -> Result<BTreeMap<String, String>> {
    let binding = spec
        .hosting_binding
        .as_ref()
        .ok_or(ExecutionError::Invalid("retained binding required"))?;
    let mut labels = BTreeMap::from([
        ("security.webdock.dev/managed-by".into(), "webdock".into()),
        ("webdock.dev/project-id".into(), spec.project_id.to_string()),
        (
            "webdock.dev/cluster-id".into(),
            binding.cluster_id.to_string(),
        ),
    ]);
    if app {
        labels.insert("webdock.dev/app-id".into(), spec.resource_id.to_string());
    }
    Ok(labels)
}
fn owner(metadata: &ObjectMeta, spec: &RuntimeSpec, app: bool) -> Result<()> {
    let labels = metadata.labels.as_ref().ok_or(ExecutionError::Ownership)?;
    if owner_labels(spec, app)?
        .iter()
        .any(|(key, value)| labels.get(key) != Some(value))
    {
        return Err(ExecutionError::Ownership);
    }
    if app && revision(metadata)? > spec.revision {
        return Err(ExecutionError::Fenced);
    }
    Ok(())
}
fn revision(metadata: &ObjectMeta) -> Result<u64> {
    metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("webdock.dev/revision"))
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .ok_or(ExecutionError::Ownership)
}
fn annotations(metadata: &mut ObjectMeta, command: &Command) {
    let annotations = metadata.annotations.get_or_insert_with(BTreeMap::new);
    annotations.insert(
        "webdock.dev/revision".into(),
        command.spec.revision.to_string(),
    );
    annotations.insert("webdock.dev/operation".into(), command.operation_id.clone());
}
fn resources(spec: &RuntimeSpec) -> ResourceRequirements {
    let limits = BTreeMap::from([
        ("cpu".into(), Quantity(format!("{}m", spec.cpu_millicores))),
        ("memory".into(), Quantity(spec.memory_bytes.to_string())),
        (
            "ephemeral-storage".into(),
            Quantity(spec.ephemeral_bytes.to_string()),
        ),
    ]);
    ResourceRequirements {
        limits: Some(limits.clone()),
        requests: Some(limits),
        ..Default::default()
    }
}
fn probe(spec: &RuntimeSpec) -> Result<Probe> {
    Ok(Probe {
        http_get: Some(HTTPGetAction {
            http_headers: spec.health_host.as_ref().map(|host| {
                vec![k8s_openapi::api::core::v1::HTTPHeader {
                    name: "Host".into(),
                    value: host.clone(),
                }]
            }),
            path: Some(spec.health_path.clone().ok_or(ExecutionError::Unsupported(
                "retained app HTTP health probe",
            ))?),
            port: IntOrString::Int(i32::from(spec.port)),
            ..Default::default()
        }),
        period_seconds: Some(3),
        timeout_seconds: Some(2),
        failure_threshold: Some(3),
        ..Default::default()
    })
}
fn retained_pod_profile(pod: &k8s_openapi::api::core::v1::PodSpec) -> Result<()> {
    if pod.containers.len() != 1
        || pod.containers[0].name != "app"
        || pod.init_containers.as_ref().is_some_and(|v| !v.is_empty())
        || pod
            .ephemeral_containers
            .as_ref()
            .is_some_and(|v| !v.is_empty())
        || pod
            .service_account_name
            .as_deref()
            .is_some_and(|v| v != "default")
        || pod
            .service_account
            .as_deref()
            .is_some_and(|v| v != "default")
        || pod.automount_service_account_token == Some(true)
        || pod.host_network == Some(true)
        || pod.host_pid == Some(true)
        || pod.host_ipc == Some(true)
        || pod
            .volumes
            .as_ref()
            .is_some_and(|v| v.iter().any(|v| v.host_path.is_some()))
        || pod.volumes.as_ref().is_some_and(|v| {
            v.iter().any(|v| {
                v.projected
                    .as_ref()
                    .and_then(|p| p.sources.as_ref())
                    .is_some_and(|s| s.iter().any(|s| s.service_account_token.is_some()))
            })
        })
    {
        return Err(ExecutionError::Unsupported(
            "unexpected retained pod layout",
        ));
    }
    Ok(())
}

/// ResourceVersion/UID are retained, so Kubernetes replace performs the ownership CAS.
pub fn deployment_update(mut existing: Deployment, command: &Command) -> Result<Deployment> {
    command.validate()?;
    let spec = &command.spec;
    owner(&existing.metadata, spec, true)?;
    if existing.metadata.name.as_deref() != Some(&name(spec))
        || existing.metadata.namespace.as_deref() != Some(&namespace(spec))
        || existing.metadata.uid.as_ref() != command.expected_runtime_id.as_ref()
        || command.expected_runtime_id.is_none()
    {
        return Err(ExecutionError::Ownership);
    }
    annotations(&mut existing.metadata, command);
    let deployment = existing.spec.as_mut().ok_or(ExecutionError::Ownership)?;
    deployment.replicas = Some(if command.action == Action::Stop || spec.replicas == 0 {
        0
    } else if let Some(policy) = &spec.autoscaling {
        // Preserve the HPA's count through configuration rollouts; only resume seeds min.
        deployment
            .replicas
            .filter(|count| *count > 0)
            .unwrap_or(i32::from(policy.min_replicas))
    } else {
        i32::from(spec.replicas)
    });
    let metadata = deployment
        .template
        .metadata
        .get_or_insert_with(ObjectMeta::default);
    annotations(metadata, command);
    let pod = deployment
        .template
        .spec
        .as_mut()
        .ok_or(ExecutionError::Ownership)?;
    retained_pod_profile(pod)?;
    pod.automount_service_account_token = Some(false);
    pod.security_context = Some(PodSecurityContext {
        run_as_non_root: Some(true),
        run_as_user: Some(65532),
        run_as_group: Some(65532),
        fs_group: Some(65532),
        seccomp_profile: Some(SeccompProfile {
            type_: "RuntimeDefault".into(),
            ..Default::default()
        }),
        ..pod.security_context.clone().unwrap_or_default()
    });
    let container = &mut pod.containers[0];
    container.image = Some(spec.image.clone());
    container.image_pull_policy = Some("IfNotPresent".into());
    container.args = Some(spec.arguments.clone());
    container.ports = Some(vec![ContainerPort {
        container_port: i32::from(spec.port),
        ..Default::default()
    }]);
    container.resources = Some(resources(spec));
    container.security_context = Some(SecurityContext {
        allow_privilege_escalation: Some(false),
        privileged: Some(false),
        read_only_root_filesystem: Some(true),
        run_as_non_root: Some(true),
        run_as_user: Some(65532),
        run_as_group: Some(65532),
        capabilities: Some(PodCapabilities {
            drop: Some(vec!["ALL".into()]),
            ..Default::default()
        }),
        ..Default::default()
    });
    let health = probe(spec)?;
    container.readiness_probe = Some(health.clone());
    container.liveness_probe = Some(health.clone());
    container.startup_probe = Some(Probe {
        failure_threshold: Some(20),
        ..health
    });
    existing.status = None;
    Ok(existing)
}
async fn retained_namespace(client: Client, spec: &RuntimeSpec) -> Result<Namespace> {
    let api: Api<Namespace> = Api::all(client);
    let ns = api
        .get(&namespace(spec))
        .await
        .map_err(|_| ExecutionError::Ownership)?;
    owner(&ns.metadata, spec, false)?;
    Ok(ns)
}
async fn existing_deployment(client: Client, spec: &RuntimeSpec) -> Result<Option<Deployment>> {
    let api: Api<Deployment> = Api::namespaced(client, &namespace(spec));
    let value = api.get_opt(&name(spec)).await.map_err(api_failure)?;
    if let Some(value) = &value {
        owner(&value.metadata, spec, true)?;
    }
    Ok(value)
}
async fn retained_storage(
    client: Client,
    spec: &RuntimeSpec,
    deployment: &Deployment,
) -> Result<()> {
    let pod = deployment
        .spec
        .as_ref()
        .and_then(|d| d.template.spec.as_ref())
        .ok_or(ExecutionError::Ownership)?;
    let existing = pod
        .volumes
        .as_ref()
        .and_then(|volumes| volumes.iter().find(|v| v.name == "data"));
    match (&spec.volume, existing) {
        (None, None) => return Ok(()),
        (Some(volume), Some(existing)) => {
            let claim = format!("{}-data", name(spec));
            if existing
                .persistent_volume_claim
                .as_ref()
                .map(|v| v.claim_name.as_str())
                != Some(&claim)
            {
                return Err(ExecutionError::Ownership);
            }
            let capacity = volume
                .size_bytes
                .ok_or(ExecutionError::Invalid("retained capacity required"))?;
            let claims: Api<PersistentVolumeClaim> =
                Api::namespaced(client.clone(), &namespace(spec));
            let pvc = claims
                .get(&claim)
                .await
                .map_err(|_| ExecutionError::Ownership)?;
            owner(&pvc.metadata, spec, true)?;
            let pvc_spec = pvc.spec.ok_or(ExecutionError::Ownership)?;
            let pv_name = format!("wd-{}-data", spec.resource_id);
            if pvc_spec.volume_name.as_deref() != Some(&pv_name)
                || pvc_spec.storage_class_name.as_deref() != Some("")
            {
                return Err(ExecutionError::Ownership);
            }
            let requested = pvc_spec
                .resources
                .and_then(|r| r.requests)
                .and_then(|mut r| r.remove("storage"))
                .ok_or(ExecutionError::Ownership)?;
            let volumes: Api<PersistentVolume> = Api::all(client);
            let pv = volumes
                .get(&pv_name)
                .await
                .map_err(|_| ExecutionError::Ownership)?;
            owner(&pv.metadata, spec, true)?;
            let pv_spec = pv.spec.ok_or(ExecutionError::Ownership)?;
            let reference = pv_spec.claim_ref.ok_or(ExecutionError::Ownership)?;
            if reference.name.as_deref() != Some(&claim)
                || reference.namespace.as_deref() != Some(&namespace(spec))
                || reference.uid != pvc.metadata.uid
                || pv_spec.persistent_volume_reclaim_policy.as_deref() != Some("Retain")
                || pv_spec.local.is_none()
            {
                return Err(ExecutionError::Ownership);
            }
            let allocated = pv_spec
                .capacity
                .and_then(|mut c| c.remove("storage"))
                .ok_or(ExecutionError::Ownership)?;
            if crate::kubernetes::parse_quantity(&requested.0, false)? != capacity
                || crate::kubernetes::parse_quantity(&allocated.0, false)? != capacity
            {
                return Err(ExecutionError::Unsupported(
                    "retained storage size changes are unsupported",
                ));
            }
        }
        _ => {
            return Err(ExecutionError::Unsupported(
                "retained storage allocation or removal requires the storage helper",
            ));
        }
    }
    Ok(())
}
async fn quota(
    client: Client,
    spec: &RuntimeSpec,
    allowance: &ResourceAllowance,
    namespace_object: &Namespace,
    guard: &LeaseGuard,
) -> Result<()> {
    let solver = namespace_object
        .metadata
        .labels
        .as_ref()
        .and_then(|l| l.get("webdock.dev/certificate-budget"))
        .is_some_and(|v| v == "1");
    let extra = |enabled: u64| if solver { enabled } else { 0 };
    let hard = BTreeMap::from([
        (
            "requests.cpu".into(),
            Quantity(format!("{}m", allowance.cpu_millicores + extra(25))),
        ),
        (
            "limits.cpu".into(),
            Quantity(format!("{}m", allowance.cpu_millicores + extra(200))),
        ),
        (
            "requests.memory".into(),
            Quantity((allowance.memory_bytes + extra(33_554_432)).to_string()),
        ),
        (
            "limits.memory".into(),
            Quantity((allowance.memory_bytes + extra(67_108_864)).to_string()),
        ),
        (
            "requests.ephemeral-storage".into(),
            Quantity((allowance.ephemeral_bytes + extra(8_388_608)).to_string()),
        ),
        (
            "limits.ephemeral-storage".into(),
            Quantity((allowance.ephemeral_bytes + extra(67_108_864)).to_string()),
        ),
        (
            "pods".into(),
            Quantity((allowance.apps * 20).max(10).to_string()),
        ),
        (
            "services".into(),
            Quantity((allowance.apps.max(1) + extra(1)).to_string()),
        ),
        ("services.nodeports".into(), Quantity("0".into())),
        ("services.loadbalancers".into(), Quantity("0".into())),
        (
            "persistentvolumeclaims".into(),
            Quantity((allowance.volume_bytes / 67_108_864).to_string()),
        ),
        (
            "requests.storage".into(),
            Quantity(allowance.volume_bytes.to_string()),
        ),
        ("count/jobs.batch".into(), Quantity("0".into())),
        (
            "count/secrets".into(),
            Quantity((allowance.apps * 4 + 2).max(20).to_string()),
        ),
        ("count/configmaps".into(), Quantity("20".into())),
    ]);
    let api: Api<ResourceQuota> = Api::namespaced(client, &namespace(spec));
    let params = PostParams {
        field_manager: Some("webdock".into()),
        ..Default::default()
    };
    let current = api
        .get_opt("webdock-allocation")
        .await
        .map_err(api_failure)?;
    let mut desired = current.clone().unwrap_or_default();
    if current.is_some() {
        owner(&desired.metadata, spec, false)?;
    } else {
        desired.metadata = ObjectMeta {
            name: Some("webdock-allocation".into()),
            namespace: Some(namespace(spec)),
            labels: Some(owner_labels(spec, false)?),
            ..Default::default()
        };
    }
    desired.spec = Some(ResourceQuotaSpec {
        hard: Some(hard),
        ..Default::default()
    });
    desired.status = None;
    guard.check().await?;
    if current.is_some() {
        api.replace("webdock-allocation", &params, &desired)
            .await
            .map_err(api_failure)?;
    } else {
        api.create(&params, &desired).await.map_err(api_failure)?;
    }
    Ok(())
}
pub async fn reconcile(
    client: Client,
    command: &Command,
    guard: &LeaseGuard,
) -> Result<ObservedState> {
    reconcile_with_storage(client, command, guard, None).await
}
pub async fn reconcile_with_storage(
    client: Client,
    command: &Command,
    guard: &LeaseGuard,
    storage: Option<&(crate::storage::StorageClient, crate::storage::StorageConfig)>,
) -> Result<ObservedState> {
    reconcile_with_registry(
        client,
        command,
        guard,
        storage,
        crate::registry::PullSelection::Unconfigured,
    )
    .await
}
pub(crate) async fn reconcile_with_registry(
    client: Client,
    command: &Command,
    guard: &LeaseGuard,
    storage: Option<&(crate::storage::StorageClient, crate::storage::StorageConfig)>,
    registry: crate::registry::PullSelection,
) -> Result<ObservedState> {
    guard.check_for(command).await?;
    let spec = &command.spec;
    let automatic =
        command.action == Action::Apply && spec.replicas > 0 && spec.autoscaling.is_some();
    if automatic && !crate::kubernetes::autoscaling::enabled() {
        return Err(ExecutionError::Unsupported("qualified automatic scaling"));
    }
    if !automatic {
        crate::kubernetes::autoscaling::remove(client.clone(), command, guard).await?;
    }
    if matches!(command.action, Action::Retire | Action::PurgeStorage) {
        return lifecycle::execute(client, command, guard, storage).await;
    }
    let binding = spec
        .hosting_binding
        .as_ref()
        .ok_or(ExecutionError::Invalid("retained binding"))?;
    if command.action == Action::Apply {
        command.verify_material()?;
    }
    let current = existing_deployment(client.clone(), spec).await?;
    let initial = current.is_none() && command.expected_runtime_id.is_none();
    if let Some(deployment) = &current {
        if let Some(expected) = &command.expected_runtime_id {
            if deployment.metadata.uid.as_ref() != Some(expected) {
                return Err(ExecutionError::Ownership);
            }
        } else if revision(&deployment.metadata)? != spec.revision
            || deployment
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("webdock.dev/operation"))
                != Some(&command.operation_id)
        {
            return Err(ExecutionError::Ownership);
        }
    } else if !initial || command.action != Action::Apply {
        return Err(ExecutionError::Ownership);
    }
    if initial && spec.volume.is_some() && storage.is_none() {
        return Err(ExecutionError::Unsupported(
            "retained allocation requires verified storage helper",
        ));
    }
    let mut desired = if let Some(deployment) = current.clone() {
        retained_storage(client.clone(), spec, &deployment).await?;
        let mut bound = command.clone();
        bound.expected_runtime_id = deployment.metadata.uid.clone();
        deployment_update(deployment, &bound)?
    } else {
        new_workload(command)?.0
    };
    environment::bind(&mut desired, command)?;
    services::bind(&mut desired, command)?;
    match registry {
        crate::registry::PullSelection::Unconfigured => {}
        crate::registry::PullSelection::Anonymous => {
            desired
                .spec
                .as_mut()
                .and_then(|s| s.template.spec.as_mut())
                .ok_or(ExecutionError::Ownership)?
                .image_pull_secrets = Some(vec![]);
        }
        crate::registry::PullSelection::Secret(name) => {
            desired
                .spec
                .as_mut()
                .and_then(|s| s.template.spec.as_mut())
                .ok_or(ExecutionError::Ownership)?
                .image_pull_secrets =
                Some(vec![k8s_openapi::api::core::v1::LocalObjectReference {
                    name,
                }]);
        }
    }

    let attachment = if let (Some((_, config)), Some(deployment)) = (storage, &current) {
        storage_attachment::reuse(client.clone(), command, config, deployment).await?
    } else {
        None
    };
    guard.check_for(command).await?;
    let namespace_object = if initial || binding.public_route.is_some() {
        bootstrap::namespace_ready(client.clone(), command, guard).await?
    } else {
        retained_namespace(client.clone(), spec).await?
    };
    if attachment.is_none()
        && let Some(storage) = storage
    {
        storage::prepare(client.clone(), command, guard, storage, initial).await?;
    }
    let services: Api<Service> = Api::namespaced(client.clone(), &namespace(spec));
    let existing_service = services.get_opt(&name(spec)).await.map_err(api_failure)?;
    if !initial && existing_service.is_none() {
        return Err(ExecutionError::Ownership);
    }
    let mut service = if let Some(service) = existing_service.clone() {
        service
    } else {
        new_workload(command)?.1
    };
    owner(&service.metadata, spec, true)?;
    let service_spec = service.spec.as_mut().ok_or(ExecutionError::Ownership)?;
    if service_spec.type_.as_deref() != Some("ClusterIP")
        || service_spec
            .selector
            .as_ref()
            .and_then(|s| s.get("webdock.dev/app-id"))
            != Some(&spec.resource_id.to_string())
    {
        return Err(ExecutionError::Ownership);
    }
    let ports = service_spec
        .ports
        .as_mut()
        .ok_or(ExecutionError::Ownership)?;
    if ports.len() != 1 {
        return Err(ExecutionError::Unsupported("retained service ports"));
    }
    ports[0].port = i32::from(spec.port);
    ports[0].target_port = Some(IntOrString::Int(i32::from(spec.port)));
    annotations(&mut service.metadata, command);
    service.status = None;
    if let (Some(proof), Some((_, config))) = (&attachment, storage) {
        guard.check_for(command).await?;
        storage_attachment::recheck(client.clone(), command, config, proof).await?;
        guard.check_for(command).await?;
    }
    quota(
        client.clone(),
        spec,
        &binding.quota,
        &namespace_object,
        guard,
    )
    .await?;
    // A rejected attachment proof must not remove existing environment material.
    // Successful readiness below still performs the ordinary referenced-secret cleanup.
    if attachment.is_none() {
        environment::cleanup(client.clone(), spec, guard).await?;
        services::cleanup(client.clone(), spec, guard).await?;
    }
    environment::prepare(client.clone(), command, guard).await?;
    services::prepare(client.clone(), command, guard).await?;
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), &namespace(spec));
    let params = PostParams {
        field_manager: Some("webdock".into()),
        ..Default::default()
    };
    guard.check_for(command).await?;
    if let (Some(proof), Some((_, config))) = (&attachment, storage) {
        storage_attachment::recheck(client.clone(), command, config, proof).await?;
        guard.check_for(command).await?;
    }
    if current.is_some() {
        deployments
            .replace(&name(spec), &params, &desired)
            .await
            .map_err(api_failure)?;
    } else {
        deployments
            .create(&params, &desired)
            .await
            .map_err(api_failure)?;
    }
    guard.check().await?;
    if existing_service.is_some() {
        services
            .replace(&name(spec), &params, &service)
            .await
            .map_err(api_failure)?;
    } else {
        services
            .create(&params, &service)
            .await
            .map_err(api_failure)?;
    }
    if automatic {
        let actual = existing_deployment(client.clone(), spec)
            .await?
            .ok_or(ExecutionError::Ownership)?;
        let uid = actual
            .metadata
            .uid
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        crate::kubernetes::autoscaling::apply(client.clone(), command, guard, uid).await?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        guard.check().await?;
        let mut observed = inspect(client.clone(), spec).await?;
        let stopped = command.action == Action::Stop || spec.replicas == 0;
        if (stopped && observed.phase == Phase::Stopped) || (!stopped && observed.ready) {
            if attachment.is_some() {
                observed.conditions.push(Condition {
                    kind: "StorageAttachment".into(),
                    satisfied: true,
                    reason: "ExistingVerifiedAttachment".into(),
                });
            }
            environment::cleanup(client.clone(), spec, guard).await?;
            services::cleanup(client.clone(), spec, guard).await?;
            quota(
                client.clone(),
                spec,
                &binding.final_quota,
                &namespace_object,
                guard,
            )
            .await?;
            guard.check().await?;
            return Ok(observed);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ExecutionError::Runtime);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
pub async fn inspect(client: Client, spec: &RuntimeSpec) -> Result<ObservedState> {
    spec.validate()?;
    let mut observed = ObservedState {
        autoscaling: None,
        resource_id: spec.resource_id.to_string(),
        revision: spec.revision,
        phase: Phase::Absent,
        ready: false,
        observed_at: timestamp()?,
        runtime_id: None,
        conditions: vec![],
        storage: None,
    };
    let Some(deployment) = existing_deployment(client.clone(), spec).await? else {
        return Ok(observed);
    };
    if let Some(volume) = &spec.volume {
        retained_storage(client.clone(), spec, &deployment).await?;
        observed.storage = Some(StorageObservation {
            volume_bytes: volume.size_bytes,
            retained: false,
        });
    }
    observed.revision = revision(&deployment.metadata)?;
    observed.runtime_id = deployment.metadata.uid;
    let replicas = deployment
        .spec
        .as_ref()
        .and_then(|d| d.replicas)
        .unwrap_or(1);
    let status = deployment.status.unwrap_or_default();
    let current =
        status.observed_generation.unwrap_or(0) >= deployment.metadata.generation.unwrap_or(1);
    let pod_api: Api<Pod> = Api::namespaced(client.clone(), &namespace(spec));
    let pods = pod_api
        .list(
            &ListParams::default()
                .labels(&format!("webdock.dev/app-id={}", spec.resource_id))
                .limit(201),
        )
        .await
        .map_err(api_failure)?;
    let complete = pods.metadata.continue_.is_none_or(|v| v.is_empty());
    let stopped = replicas == 0
        && current
        && status.replicas.unwrap_or(0) == 0
        && complete
        && pods.items.is_empty();
    observed.ready = replicas > 0
        && current
        && status.updated_replicas == Some(replicas)
        && status.available_replicas == Some(replicas)
        && complete
        && pods.items.len() == replicas as usize
        && pods.items.iter().all(|p| {
            p.metadata.deletion_timestamp.is_none()
                && p.status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .is_some_and(|c| c.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
        });
    observed.phase = if stopped {
        Phase::Stopped
    } else if observed.ready {
        Phase::Running
    } else {
        Phase::Starting
    };
    observed.conditions.push(Condition {
        kind: "Ready".into(),
        satisfied: observed.ready,
        reason: if stopped {
            "RetainedDeploymentStopped"
        } else if observed.ready {
            "RetainedDeploymentAvailable"
        } else {
            "RetainedDeploymentProgressing"
        }
        .into(),
    });
    crate::kubernetes::autoscaling::observe(client.clone(), spec, &mut observed).await?;
    Ok(observed)
}
pub async fn logs(client: Client, spec: &RuntimeSpec, tail: u32) -> Result<String> {
    if !(1..=1000).contains(&tail) {
        return Err(ExecutionError::Invalid("log tail"));
    }
    let api: Api<Pod> = Api::namespaced(client, &namespace(spec));
    let mut pods = api
        .list(
            &ListParams::default()
                .labels(&format!("webdock.dev/app-id={}", spec.resource_id))
                .limit(201),
        )
        .await
        .map_err(api_failure)?;
    if pods.items.len() > 200 || pods.metadata.continue_.is_some_and(|v| !v.is_empty()) {
        return Err(ExecutionError::Runtime);
    }
    pods.items
        .sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
    for pod in &pods.items {
        owner(&pod.metadata, spec, true)?;
    }
    let mut bytes = Vec::new();
    for pod in pods.items {
        let remaining = crate::logs::LIMIT.saturating_sub(bytes.len());
        if remaining == 0 {
            break;
        }
        let logs = api
            .logs(
                pod.metadata
                    .name
                    .as_deref()
                    .ok_or(ExecutionError::Ownership)?,
                &LogParams {
                    container: Some("app".into()),
                    tail_lines: Some(i64::from(tail)),
                    limit_bytes: Some(remaining as i64),
                    ..Default::default()
                },
            )
            .await
            .map_err(api_failure)?;
        bytes.extend_from_slice(&logs.as_bytes()[..logs.len().min(remaining)]);
    }
    Ok(crate::logs::bounded(&bytes))
}

#[track_caller]
fn api_failure(error: kube::Error) -> ExecutionError {
    let location = std::panic::Location::caller();
    if let kube::Error::Api(response) = error {
        tracing::debug!(code=response.code,reason=%response.reason,source=%location,"Retained Kubernetes API request failed");
    }
    ExecutionError::Runtime
}
