use super::*;
use k8s_openapi::api::core::v1::{
    PersistentVolume, PersistentVolumeClaim, Service, ServiceAccount,
};
use k8s_openapi::api::networking::v1::NetworkPolicy;

impl KubernetesAdapter {
    pub(super) async fn inspect_retirement(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> Result<ObservedState> {
        let spec = &command.spec;
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        let namespace = namespaces
            .get(&spec.namespace())
            .await
            .map_err(|_| ExecutionError::Ownership)?;
        spec.check_owner(&namespace.metadata.labels.clone().unwrap_or_default())?;
        if namespace.metadata.deletion_timestamp.is_some() {
            return Err(ExecutionError::Ownership);
        }
        let services: Api<Service> = Api::namespaced(self.client.clone(), &spec.namespace());
        let accounts: Api<ServiceAccount> = Api::namespaced(self.client.clone(), &spec.namespace());
        let policies: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), &spec.namespace());
        if self.deployment(spec).await?.is_some()
            || services
                .get_opt(&spec.name())
                .await
                .map_err(|_| ExecutionError::Runtime)?
                .is_some()
            || accounts
                .get_opt(&spec.name())
                .await
                .map_err(|_| ExecutionError::Runtime)?
                .is_some()
            || policies
                .get_opt(&spec.name())
                .await
                .map_err(|_| ExecutionError::Runtime)?
                .is_some()
        {
            return Err(ExecutionError::Ownership);
        }
        self.check_release_ownership(spec, command.generation)
            .await?;
        let manifest = self.helm_read(spec, &["get", "manifest"], guard).await?;
        verify_retirement_manifest(spec, &manifest)?;
        let status: Value = serde_json::from_slice(
            &self
                .helm_read(spec, &["status", "--output", "json"], guard)
                .await?,
        )
        .map_err(|_| ExecutionError::Runtime)?;
        if status["info"]["status"] != "uninstalled" {
            return Err(ExecutionError::Ownership);
        }
        let claim_name = format!("{}-data", spec.name());
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &spec.namespace());
        let pods = pods
            .list(&ListParams::default().limit(1001))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if pods.items.len() > 1000
            || pods
                .metadata
                .continue_
                .is_some_and(|value| !value.is_empty())
            || pods.items.iter().any(|pod| {
                pod.metadata
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get("webdock.dev/resource-id"))
                    .is_some_and(|id| id == spec.resource_id.as_str())
                    || pod
                        .spec
                        .as_ref()
                        .and_then(|spec| spec.volumes.as_ref())
                        .is_some_and(|volumes| {
                            volumes.iter().any(|volume| {
                                volume
                                    .persistent_volume_claim
                                    .as_ref()
                                    .is_some_and(|claim| claim.claim_name == claim_name)
                            })
                        })
            })
        {
            return Err(ExecutionError::Ownership);
        }
        let claims: Api<PersistentVolumeClaim> =
            Api::namespaced(self.client.clone(), &spec.namespace());
        let volumes: Api<PersistentVolume> = Api::all(self.client.clone());
        let claim = claims
            .get_opt(&claim_name)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let storage = match &spec.volume {
            None => {
                if claim.is_some() {
                    return Err(ExecutionError::Ownership);
                }
                None
            }
            Some(volume) if command.action == Action::Retire => {
                let claim = claim.ok_or(ExecutionError::Ownership)?;
                spec.check_owner(&claim.metadata.labels.clone().unwrap_or_default())?;
                let claim_spec = claim.spec.as_ref().ok_or(ExecutionError::Ownership)?;
                let pv_name = claim_spec
                    .volume_name
                    .as_ref()
                    .filter(|name| !name.is_empty())
                    .ok_or(ExecutionError::Ownership)?;
                let pv = volumes
                    .get(pv_name)
                    .await
                    .map_err(|_| ExecutionError::Ownership)?;
                retained_storage(spec, &claim, &pv)?;
                Some(StorageObservation {
                    volume_bytes: volume.size_bytes,
                    retained: true,
                })
            }
            Some(_) => {
                if claim.is_some() {
                    return Err(ExecutionError::Ownership);
                }
                let annotations = namespace
                    .metadata
                    .annotations
                    .as_ref()
                    .ok_or(ExecutionError::Ownership)?;
                let uid = annotations
                    .get("webdock.dev/storage-claim-uid")
                    .filter(|value| !value.is_empty())
                    .ok_or(ExecutionError::Ownership)?;
                let pv = annotations
                    .get("webdock.dev/storage-pv")
                    .ok_or(ExecutionError::Ownership)?;
                if annotations
                    .get("webdock.dev/storage-purged")
                    .map(String::as_str)
                    != Some("true")
                    || uid.len() > 128
                    || !uid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    || pv.len() > 253
                    || !pv.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.'
                    })
                {
                    return Err(ExecutionError::Ownership);
                }
                if !pv.is_empty()
                    && volumes
                        .get_opt(pv)
                        .await
                        .map_err(|_| ExecutionError::Runtime)?
                        .is_some()
                {
                    return Err(ExecutionError::Ownership);
                }
                Some(StorageObservation {
                    volume_bytes: Some(0),
                    retained: false,
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
            storage,
        })
    }
}

fn retained_storage(
    spec: &RuntimeSpec,
    claim: &PersistentVolumeClaim,
    pv: &PersistentVolume,
) -> Result<()> {
    let claim_spec = claim.spec.as_ref().ok_or(ExecutionError::Ownership)?;
    let pv_spec = pv.spec.as_ref().ok_or(ExecutionError::Ownership)?;
    let reference = pv_spec
        .claim_ref
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    let expected = spec
        .volume
        .as_ref()
        .and_then(|volume| volume.size_bytes)
        .ok_or(ExecutionError::Ownership)?;
    let requested = claim_spec
        .resources
        .as_ref()
        .and_then(|value| value.requests.as_ref())
        .and_then(|values| values.get("storage"))
        .ok_or(ExecutionError::Ownership)?;
    let capacity = pv_spec
        .capacity
        .as_ref()
        .and_then(|values| values.get("storage"))
        .ok_or(ExecutionError::Ownership)?;
    if claim.metadata.deletion_timestamp.is_some()
        || pv.metadata.deletion_timestamp.is_some()
        || claim.metadata.uid.is_none()
        || reference.uid != claim.metadata.uid
        || reference.namespace.as_deref() != Some(&spec.namespace())
        || reference.name != claim.metadata.name
        || claim_spec.volume_name != pv.metadata.name
        || claim
            .status
            .as_ref()
            .and_then(|value| value.phase.as_deref())
            != Some("Bound")
        || parse_quantity(&requested.0, false)? != expected
        || parse_quantity(&capacity.0, false)? != expected
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_proof_requires_bound_claim_uid_and_exact_capacity() {
        let spec: RuntimeSpec = serde_json::from_value(json!({
            "version":1,"installation":"test-retirement","projectId":"123","resourceId":"456",
            "revision":3,"image":format!("docker.io/library/busybox@sha256:{}","a".repeat(64)),
            "arguments":[],"cpuMillicores":100,"memoryBytes":33554432,"ephemeralBytes":16777216,
            "replicas":1,"port":8080,"healthPath":null,
            "volume":{"mountPath":"/data","sizeBytes":67108864}
        }))
        .unwrap();
        let claim: PersistentVolumeClaim = serde_json::from_value(json!({
            "metadata":{"name":format!("{}-data",spec.name()),"namespace":spec.namespace(),"uid":"claim-original"},
            "spec":{"volumeName":"pv-original","resources":{"requests":{"storage":"64Mi"}}},
            "status":{"phase":"Bound"}
        })).unwrap();
        let pv: PersistentVolume = serde_json::from_value(json!({
            "metadata":{"name":"pv-original","uid":"pv-original-uid"},
            "spec":{"capacity":{"storage":"64Mi"},"claimRef":{"namespace":spec.namespace(),
                "name":format!("{}-data",spec.name()),"uid":"claim-original"}}
        }))
        .unwrap();
        retained_storage(&spec, &claim, &pv).unwrap();
        let mut replaced = claim.clone();
        replaced.metadata.uid = Some("claim-replaced".into());
        assert!(retained_storage(&spec, &replaced, &pv).is_err());
        let mut foreign = pv.clone();
        foreign
            .spec
            .as_mut()
            .unwrap()
            .claim_ref
            .as_mut()
            .unwrap()
            .namespace = Some("foreign".into());
        assert!(retained_storage(&spec, &claim, &foreign).is_err());
        let mut resized = pv.clone();
        resized
            .spec
            .as_mut()
            .unwrap()
            .capacity
            .as_mut()
            .unwrap()
            .insert(
                "storage".into(),
                k8s_openapi::apimachinery::pkg::api::resource::Quantity("128Mi".into()),
            );
        assert!(retained_storage(&spec, &claim, &resized).is_err());
        let mut unbound = claim;
        unbound.status.as_mut().unwrap().phase = Some("Pending".into());
        assert!(retained_storage(&spec, &unbound, &pv).is_err());
    }
}
