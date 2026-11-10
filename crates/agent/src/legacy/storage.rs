use super::*;
use serde_json::json;
pub(super) async fn prepare(
    client: Client,
    command: &Command,
    guard: &LeaseGuard,
    binding: &(crate::storage::StorageClient, crate::storage::StorageConfig),
    initial: bool,
) -> Result<()> {
    let spec = &command.spec;
    let Some(volume) = &spec.volume else {
        return Ok(());
    };
    let bytes = volume
        .size_bytes
        .ok_or(ExecutionError::Invalid("retained byte size"))?;
    if spec.replicas > 1 {
        return Err(ExecutionError::Unsupported("single-node retained volume"));
    }
    let (helper, config) = binding;
    if spec.hosting_binding.as_ref().map(|b| &b.cluster_id) != Some(&config.cluster_id) {
        return Err(ExecutionError::Ownership);
    }
    let pv_name = format!("wd-{}-data", spec.resource_id);
    let claim = format!("{}-data", name(spec));
    let path = config
        .root
        .join(spec.resource_id.as_str())
        .join("mount/data");
    let volumes: Api<PersistentVolume> = Api::all(client.clone());
    let claims: Api<PersistentVolumeClaim> = Api::namespaced(client, &namespace(spec));
    let existing_pv = volumes.get_opt(&pv_name).await.map_err(api_failure)?;
    let existing_claim = claims.get_opt(&claim).await.map_err(api_failure)?;
    if !initial && (existing_pv.is_none() || existing_claim.is_none()) {
        return Err(ExecutionError::Ownership);
    }
    verify(spec, config, existing_pv.as_ref(), existing_claim.as_ref())?;
    guard.check().await?;
    let packet = crate::storage::StoragePacket::new(
        spec.resource_id.as_str(),
        spec.project_id.as_str(),
        config.cluster_id.as_str(),
        if initial { 1 } else { spec.revision },
        bytes,
        command.lease_until,
    )?;
    let (actual, actual_path) = helper.ensure(&packet).await?;
    guard.check().await?;
    if actual != *config || actual_path != path {
        return Err(ExecutionError::Ownership);
    }
    let labels = owner_labels(spec, true)?;
    let annotations = json!({"webdock.dev/revision":spec.revision.to_string()});
    if existing_pv.is_none() {
        let pv:PersistentVolume=serde_json::from_value(json!({"apiVersion":"v1","kind":"PersistentVolume","metadata":{"name":pv_name,"labels":labels,"annotations":annotations},"spec":{"capacity":{"storage":bytes.to_string()},"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","persistentVolumeReclaimPolicy":"Retain","storageClassName":"","claimRef":{"namespace":namespace(spec),"name":claim},"local":{"path":path},"nodeAffinity":{"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[config.node]}]}]}}}})).map_err(|_|ExecutionError::Runtime)?;
        guard.check().await?;
        volumes
            .create(&PostParams::default(), &pv)
            .await
            .map_err(api_failure)?;
    }
    if existing_claim.is_none() {
        let pvc:PersistentVolumeClaim=serde_json::from_value(json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":claim,"namespace":namespace(spec),"labels":labels,"annotations":annotations},"spec":{"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","storageClassName":"","volumeName":pv_name,"resources":{"requests":{"storage":bytes.to_string()}}}})).map_err(|_|ExecutionError::Runtime)?;
        guard.check().await?;
        claims
            .create(&PostParams::default(), &pvc)
            .await
            .map_err(api_failure)?;
    }
    Ok(())
}

pub(super) fn verify(
    spec: &RuntimeSpec,
    config: &crate::storage::StorageConfig,
    existing_pv: Option<&PersistentVolume>,
    existing_claim: Option<&PersistentVolumeClaim>,
) -> Result<()> {
    verify_objects(
        spec,
        &config.cluster_id,
        &config.root,
        &config.node,
        existing_pv,
        existing_claim,
    )
}

/// Pure object checks shared with an independently proved existing attachment.
/// The local helper still always passes its own fixed node/root above.
pub(super) fn verify_objects(
    spec: &RuntimeSpec,
    cluster_id: &spitzli_core::id::RecordId,
    root: &std::path::Path,
    node: &str,
    existing_pv: Option<&PersistentVolume>,
    existing_claim: Option<&PersistentVolumeClaim>,
) -> Result<()> {
    if spec.hosting_binding.as_ref().map(|b| &b.cluster_id) != Some(cluster_id) {
        return Err(ExecutionError::Ownership);
    }
    let bytes = spec
        .volume
        .as_ref()
        .and_then(|v| v.size_bytes)
        .ok_or(ExecutionError::Invalid("retained byte size"))?;
    let pv_name = format!("wd-{}-data", spec.resource_id);
    let claim = format!("{}-data", name(spec));
    let path = root.join(spec.resource_id.as_str()).join("mount/data");
    if let Some(pv) = existing_pv {
        owner(&pv.metadata, spec, true)?;
        let p = pv.spec.as_ref().ok_or(ExecutionError::Ownership)?;
        let reference = p.claim_ref.as_ref().ok_or(ExecutionError::Ownership)?;
        let affinity = json!({"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[node]}]}]}});
        if p.local.as_ref().map(|s| s.path.as_str()) != path.to_str()
            || p.persistent_volume_reclaim_policy.as_deref() != Some("Retain")
            || !p.storage_class_name.as_deref().unwrap_or("").is_empty()
            || reference.namespace.as_deref() != Some(&namespace(spec))
            || reference.name.as_deref() != Some(&claim)
            || serde_json::to_value(&p.node_affinity).map_err(|_| ExecutionError::Runtime)?
                != affinity
            || p.capacity
                .as_ref()
                .and_then(|c| c.get("storage"))
                .map(|q| crate::kubernetes::parse_quantity(&q.0, false))
                .transpose()?
                != Some(bytes)
        {
            return Err(ExecutionError::Ownership);
        }
        if let Some(pvc) = existing_claim
            && reference.uid.is_some()
            && reference.uid != pvc.metadata.uid
        {
            return Err(ExecutionError::Ownership);
        }
    }
    if let Some(pvc) = existing_claim {
        owner(&pvc.metadata, spec, true)?;
        let p = pvc.spec.as_ref().ok_or(ExecutionError::Ownership)?;
        if p.volume_name.as_deref() != Some(&pv_name)
            || p.storage_class_name.as_deref() != Some("")
            || p.resources
                .as_ref()
                .and_then(|r| r.requests.as_ref())
                .and_then(|r| r.get("storage"))
                .map(|q| crate::kubernetes::parse_quantity(&q.0, false))
                .transpose()?
                != Some(bytes)
        {
            return Err(ExecutionError::Ownership);
        }
    }
    Ok(())
}
