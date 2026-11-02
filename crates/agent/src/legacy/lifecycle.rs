use super::*;
use k8s_openapi::api::core::v1::Secret;
use kube::api::{DeleteParams, Preconditions, PropagationPolicy};
/// Verify the retained layout without SSA, cleanup, quota writes, or storage repair.
pub async fn inspect_completed(
    client: Client,
    command: &Command,
    guard: &LeaseGuard,
    storage: Option<&(crate::storage::StorageClient, crate::storage::StorageConfig)>,
) -> Result<ObservedState> {
    command.validate()?;
    if !matches!(command.action, Action::Retire | Action::PurgeStorage) {
        return Err(ExecutionError::Invalid("completed lifecycle inspection"));
    }
    guard.check_for(command).await?;
    let spec = &command.spec;
    let binding = spec
        .hosting_binding
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let ns = namespaces
        .get(&namespace(spec))
        .await
        .map_err(api_failure)?;
    owner(&ns.metadata, spec, false)?;
    if ns.metadata.deletion_timestamp.is_some()
        || existing_deployment(client.clone(), spec).await?.is_some()
    {
        return Err(ExecutionError::Ownership);
    }
    let services: Api<Service> = Api::namespaced(client.clone(), &namespace(spec));
    if services
        .get_opt(&name(spec))
        .await
        .map_err(api_failure)?
        .is_some()
    {
        return Err(ExecutionError::Ownership);
    }
    let claim = format!("{}-data", name(spec));
    let consumers = pods(client.clone(), spec, true, true).await?;
    if consumers.iter().any(|pod| {
        pod.metadata
            .labels
            .as_ref()
            .and_then(|values| values.get("webdock.dev/app-id"))
            .is_some_and(|id| id == spec.resource_id.as_str())
            || pod
                .spec
                .as_ref()
                .and_then(|value| value.volumes.as_ref())
                .is_some_and(|volumes| {
                    volumes.iter().any(|volume| {
                        volume
                            .persistent_volume_claim
                            .as_ref()
                            .is_some_and(|volume| volume.claim_name == claim)
                    })
                })
    }) {
        return Err(ExecutionError::Ownership);
    }
    let claims: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), &namespace(spec));
    let volumes: Api<PersistentVolume> = Api::all(client.clone());
    let pvc = claims.get_opt(&claim).await.map_err(api_failure)?;
    let pv = volumes
        .get_opt(&format!("wd-{}-data", spec.resource_id))
        .await
        .map_err(api_failure)?;
    let observation = match &spec.volume {
        None => {
            if pvc.is_some() || pv.is_some() {
                return Err(ExecutionError::Ownership);
            }
            None
        }
        Some(volume) => {
            let (helper, config) = storage.ok_or(ExecutionError::Unsupported(
                "verified retained storage inspection",
            ))?;
            if helper.information().await? != *config {
                return Err(ExecutionError::Ownership);
            }
            storage::verify(spec, config, pv.as_ref(), pvc.as_ref())?;
            let purged = command.action == Action::PurgeStorage;
            if purged && (pvc.is_some() || pv.is_some()) {
                return Err(ExecutionError::Ownership);
            }
            if !purged {
                let pvc = pvc.as_ref().ok_or(ExecutionError::Ownership)?;
                let pv = pv.as_ref().ok_or(ExecutionError::Ownership)?;
                if pvc.metadata.deletion_timestamp.is_some()
                    || pv.metadata.deletion_timestamp.is_some()
                    || pvc.metadata.uid.is_none()
                    || pv
                        .spec
                        .as_ref()
                        .and_then(|spec| spec.claim_ref.as_ref())
                        .and_then(|reference| reference.uid.as_ref())
                        != pvc.metadata.uid.as_ref()
                {
                    return Err(ExecutionError::Ownership);
                }
            }
            let packet = crate::storage::StoragePacket::new(
                spec.resource_id.as_str(),
                spec.project_id.as_str(),
                binding.cluster_id.as_str(),
                spec.revision,
                volume.size_bytes.ok_or(ExecutionError::Ownership)?,
                command.lease_until,
            )?;
            let expected = if purged {
                crate::storage::VolumeState::Purged
            } else {
                crate::storage::VolumeState::Ready
            };
            if helper.volume_state(&packet).await? != expected {
                return Err(ExecutionError::Ownership);
            }
            Some(StorageObservation {
                volume_bytes: if purged { Some(0) } else { volume.size_bytes },
                retained: !purged,
            })
        }
    };
    guard.check_for(command).await?;
    Ok(ObservedState {
        resource_id: spec.resource_id.to_string(),
        revision: spec.revision,
        phase: Phase::Absent,
        ready: true,
        observed_at: timestamp()?,
        runtime_id: None,
        conditions: vec![],
        storage: observation,
    })
}
fn deletion(metadata: &ObjectMeta) -> Result<DeleteParams> {
    Ok(DeleteParams {
        preconditions: Some(Preconditions {
            uid: Some(metadata.uid.clone().ok_or(ExecutionError::Ownership)?),
            resource_version: Some(
                metadata
                    .resource_version
                    .clone()
                    .ok_or(ExecutionError::Ownership)?,
            ),
        }),
        propagation_policy: Some(PropagationPolicy::Foreground),
        ..Default::default()
    })
}
async fn pods(
    client: Client,
    spec: &RuntimeSpec,
    namespace_exists: bool,
    all: bool,
) -> Result<Vec<Pod>> {
    if !namespace_exists {
        return Ok(vec![]);
    }
    let api: Api<Pod> = Api::namespaced(client, &namespace(spec));
    let selector = format!("webdock.dev/app-id={}", spec.resource_id);
    let params = if all {
        ListParams::default().limit(1001)
    } else {
        ListParams::default().labels(&selector).limit(1001)
    };
    let values = api.list(&params).await.map_err(api_failure)?;
    if values.items.len() > 1000 || values.metadata.continue_.is_some_and(|v| !v.is_empty()) {
        return Err(ExecutionError::Runtime);
    }
    Ok(values.items)
}
async fn cleanup_secrets(client: Client, spec: &RuntimeSpec, guard: &LeaseGuard) -> Result<()> {
    let api: Api<Secret> = Api::namespaced(client, &namespace(spec));
    let values = api
        .list(
            &ListParams::default()
                .labels(&format!("webdock.dev/app-id={}", spec.resource_id))
                .limit(201),
        )
        .await
        .map_err(api_failure)?;
    if values.items.len() > 200 || values.metadata.continue_.is_some_and(|v| !v.is_empty()) {
        return Err(ExecutionError::Runtime);
    }
    for secret in values.items {
        let secret_name = secret
            .metadata
            .name
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        let suffix = secret_name
            .strip_prefix(&format!("{}-env-", name(spec)))
            .or_else(|| secret_name.strip_prefix(&format!("{}-services-", name(spec))));
        let Some(suffix) = suffix else { continue };
        let secret_revision = suffix
            .parse::<u64>()
            .ok()
            .filter(|r| *r > 0 && *r <= spec.revision)
            .ok_or(ExecutionError::Ownership)?;
        if suffix != secret_revision.to_string() {
            return Err(ExecutionError::Ownership);
        }
        let labels = secret
            .metadata
            .labels
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        if owner_labels(spec, true)?
            .iter()
            .any(|(key, value)| labels.get(key) != Some(value))
        {
            return Err(ExecutionError::Ownership);
        }
        guard.check().await?;
        api.delete(secret_name, &deletion(&secret.metadata)?)
            .await
            .map_err(api_failure)?;
    }
    Ok(())
}
pub(super) async fn execute(
    client: Client,
    command: &Command,
    guard: &LeaseGuard,
    storage: Option<&(crate::storage::StorageClient, crate::storage::StorageConfig)>,
) -> Result<ObservedState> {
    command.validate()?;
    guard.check_for(command).await?;
    let spec = &command.spec;
    let binding = spec
        .hosting_binding
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let ns = namespaces
        .get_opt(&namespace(spec))
        .await
        .map_err(api_failure)?;
    if let Some(ns) = &ns {
        owner(&ns.metadata, spec, false)?;
    }
    let current = existing_deployment(client.clone(), spec).await?;
    if let Some(deployment) = &current {
        if command.action == Action::PurgeStorage
            || command.expected_runtime_id.is_none()
            || deployment.metadata.uid.as_ref() != command.expected_runtime_id.as_ref()
        {
            return Err(ExecutionError::Ownership);
        }
        retained_storage(client.clone(), spec, deployment).await?;
    }
    let claims: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), &namespace(spec));
    let volumes: Api<PersistentVolume> = Api::all(client.clone());
    let claim = format!("{}-data", name(spec));
    let pv_name = format!("wd-{}-data", spec.resource_id);
    let pvc = claims.get_opt(&claim).await.map_err(api_failure)?;
    let pv = volumes.get_opt(&pv_name).await.map_err(api_failure)?;
    let packet = if let Some(bytes) = spec.volume.as_ref().and_then(|v| v.size_bytes) {
        if let Some((helper, config)) = storage {
            if helper.information().await? != *config {
                return Err(ExecutionError::Ownership);
            }
            storage::verify(spec, config, pv.as_ref(), pvc.as_ref())?;
            let packet = crate::storage::StoragePacket::new(
                spec.resource_id.as_str(),
                spec.project_id.as_str(),
                binding.cluster_id.as_str(),
                spec.revision,
                bytes,
                command.lease_until,
            )?;
            helper.retained(&packet).await?;
            Some(packet)
        } else if command.action == Action::PurgeStorage {
            return Err(ExecutionError::Unsupported(
                "purge requires verified storage helper",
            ));
        } else {
            if let Some(pvc) = &pvc {
                owner(&pvc.metadata, spec, true)?;
            }
            if let Some(pv) = &pv {
                owner(&pv.metadata, spec, true)?;
            }
            None
        }
    } else {
        if pvc.is_some() || pv.is_some() {
            return Err(ExecutionError::Ownership);
        }
        None
    };
    let services: Api<Service> = Api::namespaced(client.clone(), &namespace(spec));
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), &namespace(spec));
    if command.action == Action::Retire {
        if let Some(service) = services.get_opt(&name(spec)).await.map_err(api_failure)? {
            owner(&service.metadata, spec, true)?;
            guard.check().await?;
            services
                .delete(&name(spec), &deletion(&service.metadata)?)
                .await
                .map_err(api_failure)?;
        }
        if let Some(deployment) = current {
            guard.check().await?;
            deployments
                .delete(&name(spec), &deletion(&deployment.metadata)?)
                .await
                .map_err(api_failure)?;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            guard.check().await?;
            let remaining = pods(client.clone(), spec, ns.is_some(), false).await?;
            for pod in &remaining {
                owner(&pod.metadata, spec, true)?;
            }
            if deployments
                .get_opt(&name(spec))
                .await
                .map_err(api_failure)?
                .is_none()
                && services
                    .get_opt(&name(spec))
                    .await
                    .map_err(api_failure)?
                    .is_none()
                && remaining.is_empty()
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if ns.is_some() {
            cleanup_secrets(client.clone(), spec, guard).await?;
        }
    } else {
        if current.is_some()
            || services
                .get_opt(&name(spec))
                .await
                .map_err(api_failure)?
                .is_some()
        {
            return Err(ExecutionError::Ownership);
        }
        let consumers = pods(client.clone(), spec, ns.is_some(), true).await?;
        if consumers.iter().any(|pod| {
            pod.spec
                .as_ref()
                .and_then(|s| s.volumes.as_ref())
                .is_some_and(|vs| {
                    vs.iter().any(|v| {
                        v.persistent_volume_claim
                            .as_ref()
                            .is_some_and(|v| v.claim_name == claim)
                    })
                })
        }) {
            return Err(ExecutionError::Ownership);
        }
        if let Some(pvc) = pvc {
            guard.check().await?;
            claims
                .delete(&claim, &deletion(&pvc.metadata)?)
                .await
                .map_err(api_failure)?;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while claims.get_opt(&claim).await.map_err(api_failure)?.is_some() {
            guard.check().await?;
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // The binder may change PV resourceVersion when the claim disappears; re-read and verify before CAS deletion.
        if let Some(pv) = volumes.get_opt(&pv_name).await.map_err(api_failure)? {
            let (_, config) = storage.ok_or(ExecutionError::Ownership)?;
            storage::verify(spec, config, Some(&pv), None)?;
            guard.check().await?;
            volumes
                .delete(&pv_name, &deletion(&pv.metadata)?)
                .await
                .map_err(api_failure)?;
        }
        while volumes
            .get_opt(&pv_name)
            .await
            .map_err(api_failure)?
            .is_some()
        {
            guard.check().await?;
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        guard.check().await?;
        storage
            .ok_or(ExecutionError::Ownership)?
            .0
            .purge(packet.as_ref().ok_or(ExecutionError::Ownership)?)
            .await?;
    }
    if let Some(ns) = &ns {
        quota(client, spec, &binding.final_quota, ns, guard).await?;
    }
    guard.check_for(command).await?;
    Ok(ObservedState {
        resource_id: spec.resource_id.to_string(),
        revision: spec.revision,
        phase: Phase::Absent,
        ready: true,
        observed_at: timestamp()?,
        runtime_id: None,
        conditions: vec![Condition {
            kind: "Retired".into(),
            satisfied: true,
            reason: "Owned runtime is absent".into(),
        }],
        storage: spec.volume.as_ref().map(|v| StorageObservation {
            volume_bytes: if command.action == Action::PurgeStorage {
                Some(0)
            } else {
                v.size_bytes
            },
            retained: command.action == Action::Retire,
        }),
    })
}
