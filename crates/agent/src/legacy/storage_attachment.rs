//! Existing cross-node attachment proofs never grant privileged storage operations.
use super::*;
use k8s_openapi::api::core::v1::Node;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Proof {
    deployment: (String, String),
    pv: (String, String),
    pvc: (String, String),
    node: NodeIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NodeIdentity {
    name: String,
    uid: String,
    machine: String,
    system: uuid::Uuid,
}

fn identity(metadata: &ObjectMeta) -> Result<(String, String)> {
    if metadata.deletion_timestamp.is_some() {
        return Err(ExecutionError::Ownership);
    }
    let uid = metadata
        .uid
        .as_ref()
        .filter(|v| !v.is_empty())
        .ok_or(ExecutionError::Ownership)?;
    let revision = metadata
        .resource_version
        .as_ref()
        .filter(|v| !v.is_empty())
        .ok_or(ExecutionError::Ownership)?;
    Ok((uid.clone(), revision.clone()))
}

fn only_keys(value: &impl serde::Serialize, allowed: &[&str]) -> Result<()> {
    let value = serde_json::to_value(value).map_err(|_| ExecutionError::Runtime)?;
    if value
        .as_object()
        .is_none_or(|v| v.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}

fn affinity_node(pv: &PersistentVolume) -> Result<String> {
    let value = serde_json::to_value(
        pv.spec
            .as_ref()
            .ok_or(ExecutionError::Ownership)?
            .node_affinity
            .as_ref()
            .ok_or(ExecutionError::Ownership)?,
    )
    .map_err(|_| ExecutionError::Runtime)?;
    let node = value
        .pointer("/required/nodeSelectorTerms/0/matchExpressions/0/values/0")
        .and_then(Value::as_str)
        .ok_or(ExecutionError::Ownership)?;
    if node.is_empty()
        || node.len() > 253
        || !node.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part.as_bytes()[0].is_ascii_alphanumeric()
                && part.as_bytes()[part.len() - 1].is_ascii_alphanumeric()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        || value
            != json!({"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[node]}]}]}})
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(node.into())
}

fn ready_node(nodes: &[Node], name: &str) -> Result<NodeIdentity> {
    if nodes.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let matches = nodes
        .iter()
        .filter(|n| {
            n.metadata.name.as_deref() == Some(name)
                || n.metadata
                    .labels
                    .as_ref()
                    .and_then(|v| v.get("kubernetes.io/hostname"))
                    .map(String::as_str)
                    == Some(name)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let node = matches[0];
    let (uid, _) = identity(&node.metadata)?;
    if node
        .spec
        .as_ref()
        .is_some_and(|s| s.unschedulable == Some(true))
        || node.metadata.name.as_deref() != Some(name)
        || node
            .metadata
            .labels
            .as_ref()
            .and_then(|v| v.get("kubernetes.io/hostname"))
            .map(String::as_str)
            != Some(name)
    {
        return Err(ExecutionError::Ownership);
    }
    let status = node.status.as_ref().ok_or(ExecutionError::Ownership)?;
    if !status
        .conditions
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|v| v.type_ == "Ready" && v.status == "True")
    {
        return Err(ExecutionError::Ownership);
    }
    let info = status.node_info.as_ref().ok_or(ExecutionError::Ownership)?;
    let system = uuid::Uuid::parse_str(&info.system_uuid).map_err(|_| ExecutionError::Ownership)?;
    if info.operating_system != "linux"
        || !["amd64", "arm64"].contains(&info.architecture.as_str())
        || system.is_nil()
        || info.machine_id.len() != 32
        || !info.machine_id.bytes().all(|v| v.is_ascii_hexdigit())
        || info.machine_id.bytes().all(|v| v == b'0')
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(NodeIdentity {
        name: name.into(),
        uid,
        machine: info.machine_id.clone(),
        system,
    })
}

fn verify(
    command: &Command,
    config: &crate::storage::StorageConfig,
    deployment: &Deployment,
    pv: &PersistentVolume,
    pvc: &PersistentVolumeClaim,
    nodes: &[Node],
) -> Result<Proof> {
    command.validate()?;
    if !matches!(command.action, Action::Apply | Action::Stop) || command.spec.replicas > 1 {
        return Err(ExecutionError::Ownership);
    }
    let spec = &command.spec;
    spec.volume
        .as_ref()
        .filter(|v| v.mount_path == "/data" && v.size_bytes.is_some())
        .ok_or(ExecutionError::Ownership)?;
    if spec
        .hosted_access
        .as_ref()
        .is_some_and(|v| v.service_bindings.is_some())
    {
        return Err(ExecutionError::Unsupported(
            "cross-node retained service binding changes",
        ));
    }
    if spec
        .hosting_binding
        .as_ref()
        .is_some_and(|v| v.public_route.is_some())
    {
        return Err(ExecutionError::Unsupported(
            "cross-node retained public routing changes",
        ));
    }
    owner(&deployment.metadata, spec, true)?;
    let deployment_identity = identity(&deployment.metadata)?;
    if command.expected_runtime_id.as_ref() != Some(&deployment_identity.0)
        || deployment.metadata.name.as_deref() != Some(&name(spec))
        || deployment.metadata.namespace.as_deref() != Some(&namespace(spec))
    {
        return Err(ExecutionError::Ownership);
    }
    let deployed = deployment.spec.as_ref().ok_or(ExecutionError::Ownership)?;
    let template = deployed
        .template
        .metadata
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    owner(template, spec, true)?;
    let selector = &deployed.selector;
    if selector
        .match_labels
        .as_ref()
        .and_then(|v| v.get("webdock.dev/app-id"))
        != Some(&spec.resource_id.to_string())
        || selector
            .match_expressions
            .as_ref()
            .is_some_and(|v| !v.is_empty())
        || selector.match_labels.as_ref().is_some_and(|v| {
            v.iter().any(|(key, value)| {
                template.labels.as_ref().and_then(|labels| labels.get(key)) != Some(value)
            })
        })
    {
        return Err(ExecutionError::Ownership);
    }
    let pod = deployment
        .spec
        .as_ref()
        .and_then(|d| d.template.spec.as_ref())
        .ok_or(ExecutionError::Ownership)?;
    retained_pod_profile(pod)?;
    if pod.containers.len() != 1 || pod.containers[0].name != "app" {
        return Err(ExecutionError::Ownership);
    }
    let claim = format!("{}-data", name(spec));
    let volumes = pod.volumes.as_deref().unwrap_or_default();
    let data = volumes
        .iter()
        .filter(|v| v.name == "data")
        .collect::<Vec<_>>();
    if data.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    only_keys(data[0], &["name", "persistentVolumeClaim"])?;
    let source = data[0]
        .persistent_volume_claim
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    if source.claim_name != claim
        || source.read_only == Some(true)
        || volumes.iter().any(|v| {
            v.name != "data"
                && v.persistent_volume_claim
                    .as_ref()
                    .is_some_and(|p| p.claim_name == claim)
        })
    {
        return Err(ExecutionError::Ownership);
    }
    let mounts = pod.containers[0]
        .volume_mounts
        .as_deref()
        .unwrap_or_default();
    let data_mounts = mounts
        .iter()
        .filter(|v| v.name == "data")
        .collect::<Vec<_>>();
    if data_mounts.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let mount = data_mounts[0];
    if mount.mount_path != "/data"
        || mount.sub_path.is_some()
        || mount.sub_path_expr.is_some()
        || mount.mount_propagation.is_some()
        || mount.recursive_read_only.is_some()
        || mount.read_only == Some(true)
        || mounts
            .iter()
            .any(|m| m.name != "data" && m.mount_path == "/data")
    {
        return Err(ExecutionError::Ownership);
    }
    let pv_identity = identity(&pv.metadata)?;
    let pvc_identity = identity(&pvc.metadata)?;
    if pv.metadata.name.as_deref() != Some(&format!("wd-{}-data", spec.resource_id))
        || pv.metadata.namespace.is_some()
        || pvc.metadata.name.as_deref() != Some(&claim)
        || pvc.metadata.namespace.as_deref() != Some(&namespace(spec))
    {
        return Err(ExecutionError::Ownership);
    }
    let p = pv.spec.as_ref().ok_or(ExecutionError::Ownership)?;
    let c = pvc.spec.as_ref().ok_or(ExecutionError::Ownership)?;
    only_keys(
        p,
        &[
            "accessModes",
            "capacity",
            "claimRef",
            "local",
            "nodeAffinity",
            "persistentVolumeReclaimPolicy",
            "storageClassName",
            "volumeMode",
            "mountOptions",
        ],
    )?;
    only_keys(
        c,
        &[
            "accessModes",
            "resources",
            "storageClassName",
            "volumeName",
            "volumeMode",
        ],
    )?;
    if p.volume_mode.as_deref().is_some_and(|v| v != "Filesystem")
        || c.volume_mode.as_deref().is_some_and(|v| v != "Filesystem")
        || p.access_modes
            .as_deref()
            .is_none_or(|m| m.len() != 1 || m[0] != "ReadWriteOnce")
        || c.access_modes
            .as_deref()
            .is_none_or(|m| m.len() != 1 || m[0] != "ReadWriteOnce")
        || p.mount_options.as_ref().is_some_and(|v| !v.is_empty())
        || p.local.as_ref().is_none_or(|v| v.fs_type.is_some())
        || p.claim_ref.as_ref().and_then(|v| v.uid.as_ref()) != Some(&pvc_identity.0)
        || pv.status.as_ref().and_then(|v| v.phase.as_deref()) != Some("Bound")
        || pvc.status.as_ref().and_then(|v| v.phase.as_deref()) != Some("Bound")
    {
        return Err(ExecutionError::Ownership);
    }
    let reference = p.claim_ref.as_ref().ok_or(ExecutionError::Ownership)?;
    if reference
        .kind
        .as_deref()
        .is_some_and(|v| v != "PersistentVolumeClaim")
        || reference.api_version.as_deref().is_some_and(|v| v != "v1")
    {
        return Err(ExecutionError::Ownership);
    }
    let node_name = affinity_node(pv)?;
    storage::verify_objects(
        spec,
        &config.cluster_id,
        &config.root,
        &node_name,
        Some(pv),
        Some(pvc),
    )?;
    let node = ready_node(nodes, &node_name)?;
    Ok(Proof {
        deployment: deployment_identity,
        pv: pv_identity,
        pvc: pvc_identity,
        node,
    })
}

async fn nodes(client: Client, hostname: &str) -> Result<Vec<Node>> {
    let api: Api<Node> = Api::all(client);
    let result = api
        .list(
            &ListParams::default()
                .labels(&format!("kubernetes.io/hostname={hostname}"))
                .limit(2),
        )
        .await
        .map_err(api_failure)?;
    if result.metadata.continue_.is_some_and(|v| !v.is_empty()) || result.items.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    Ok(result.items)
}

pub(super) async fn reuse(
    client: Client,
    command: &Command,
    config: &crate::storage::StorageConfig,
    deployment: &Deployment,
) -> Result<Option<Proof>> {
    if command.spec.volume.is_none() {
        return Ok(None);
    }
    let pv_api: Api<PersistentVolume> = Api::all(client.clone());
    let pv = pv_api
        .get(&format!("wd-{}-data", command.spec.resource_id))
        .await
        .map_err(api_failure)?;
    let hostname = affinity_node(&pv)?;
    let pvc_api: Api<PersistentVolumeClaim> =
        Api::namespaced(client.clone(), &namespace(&command.spec));
    let pvc = pvc_api
        .get(&format!("{}-data", name(&command.spec)))
        .await
        .map_err(api_failure)?;
    if hostname == config.node {
        storage::verify(&command.spec, config, Some(&pv), Some(&pvc))?;
        return Ok(None);
    }
    Ok(Some(verify(
        command,
        config,
        deployment,
        &pv,
        &pvc,
        &nodes(client, &hostname).await?,
    )?))
}

pub(super) async fn recheck(
    client: Client,
    command: &Command,
    config: &crate::storage::StorageConfig,
    proof: &Proof,
) -> Result<()> {
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), &namespace(&command.spec));
    let deployment = deployments
        .get(&name(&command.spec))
        .await
        .map_err(api_failure)?;
    let actual = reuse(client, command, config, &deployment)
        .await?
        .ok_or(ExecutionError::Ownership)?;
    if &actual != proof {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
