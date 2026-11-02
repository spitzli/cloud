use super::*;
use std::collections::BTreeSet;
use tokio::io::AsyncReadExt;
pub fn verify_retirement_manifest(spec: &RuntimeSpec, bytes: &[u8]) -> Result<()> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(ExecutionError::Invalid("release manifest limit"));
    }
    let objects: Vec<serde_json::Value> = serde_saphyr::from_slice_multiple(bytes)
        .map_err(|_| ExecutionError::Invalid("release manifest"))?;
    let mut seen = BTreeSet::new();
    for object in objects {
        if object.is_null() {
            continue;
        }
        let kind = object["kind"].as_str().ok_or(ExecutionError::Ownership)?;
        let version = match kind {
            "Deployment" => "apps/v1",
            "NetworkPolicy" => "networking.k8s.io/v1",
            "Service" | "ServiceAccount" | "PersistentVolumeClaim" => "v1",
            _ => return Err(ExecutionError::Ownership),
        };
        let name = if kind == "PersistentVolumeClaim" {
            if spec.volume.is_none()
                || object["metadata"]["annotations"]["helm.sh/resource-policy"] != "keep"
            {
                return Err(ExecutionError::Ownership);
            }
            format!("{}-data", spec.name())
        } else {
            spec.name()
        };
        if object["apiVersion"] != version
            || object["metadata"]["name"] != name
            || object["metadata"]["namespace"]
                .as_str()
                .is_some_and(|n| n != spec.namespace())
            || !object["metadata"]["annotations"]["helm.sh/hook"].is_null()
            || !seen.insert(kind.to_string())
        {
            return Err(ExecutionError::Ownership);
        }
        let labels = serde_json::from_value(object["metadata"]["labels"].clone())
            .map_err(|_| ExecutionError::Ownership)?;
        spec.check_owner(&labels)?;
    }
    for kind in ["Deployment", "Service", "ServiceAccount", "NetworkPolicy"] {
        if !seen.contains(kind) {
            return Err(ExecutionError::Ownership);
        }
    }
    if seen.contains("PersistentVolumeClaim") != spec.volume.is_some() {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
impl KubernetesAdapter {
    pub(super) async fn helm_read(
        &self,
        spec: &RuntimeSpec,
        args: &[&str],
        guard: &LeaseGuard,
    ) -> Result<Vec<u8>> {
        guard.check().await?;
        let mut command = tokio::process::Command::new(&self.helm);
        command
            .args(args)
            .arg(spec.name())
            .args([
                "--namespace",
                &spec.namespace(),
                "--kube-context",
                &self.context,
            ])
            .arg("--kubeconfig")
            .arg(&self.kubeconfig)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| ExecutionError::Runtime)?;
        let stdout = child.stdout.take().ok_or(ExecutionError::Runtime)?;
        let work = async {
            let mut bytes = Vec::new();
            stdout
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            if bytes.len() > 2 * 1024 * 1024 {
                return Err(ExecutionError::Runtime);
            }
            if !child
                .wait()
                .await
                .map_err(|_| ExecutionError::Runtime)?
                .success()
            {
                return Err(ExecutionError::Runtime);
            }
            Ok(bytes)
        };
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(20), work)
            .await
            .map_err(|_| ExecutionError::Expired)??;
        guard.check().await?;
        Ok(bytes)
    }
    pub(super) async fn retire_release(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> Result<ObservedState> {
        command.validate()?;
        guard.check_for(command).await?;
        self.validate(&command.spec)?;
        self.check_expected(command).await?;
        let spec = &command.spec;
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        let namespace = namespaces
            .get_opt(&spec.namespace())
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if let Some(namespace) = &namespace {
            spec.check_owner(&namespace.metadata.labels.clone().unwrap_or_default())?;
        }
        self.check_release_ownership(spec, command.generation)
            .await?;
        if namespace.is_some() {
            let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &spec.namespace());
            let history = secrets
                .list(
                    &ListParams::default()
                        .labels(&format!("owner=helm,name={}", spec.name()))
                        .limit(21),
                )
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            if history.items.len() > 20 || history.metadata.continue_.is_some_and(|s| !s.is_empty())
            {
                return Err(ExecutionError::Ownership);
            }
            if !history.items.is_empty() {
                let manifest = self.helm_read(spec, &["get", "manifest"], guard).await?;
                verify_retirement_manifest(spec, &manifest)?;
                let status = self
                    .helm_read(spec, &["status", "--output", "json"], guard)
                    .await?;
                let status: serde_json::Value =
                    serde_json::from_slice(&status).map_err(|_| ExecutionError::Runtime)?;
                if status["info"]["status"] != "uninstalled" {
                    guard.check().await?;
                    let status = tokio::process::Command::new(&self.helm)
                        .arg("uninstall")
                        .arg(spec.name())
                        .args([
                            "--namespace",
                            &spec.namespace(),
                            "--kube-context",
                            &self.context,
                        ])
                        .arg("--kubeconfig")
                        .arg(&self.kubeconfig)
                        .args([
                            "--no-hooks",
                            "--keep-history",
                            "--ignore-not-found",
                            "--cascade",
                            "foreground",
                            "--wait",
                            "--timeout",
                            "120s",
                        ])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .kill_on_drop(true)
                        .status()
                        .await
                        .map_err(|_| ExecutionError::Runtime)?;
                    if !status.success() {
                        return Err(ExecutionError::Runtime);
                    }
                }
            } else if self.deployment(spec).await?.is_some() {
                return Err(ExecutionError::Ownership);
            }
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &spec.namespace());
        loop {
            guard.check_for(command).await?;
            let remaining = pods
                .list(
                    &ListParams::default()
                        .labels(&format!("webdock.dev/resource-id={}", spec.resource_id))
                        .limit(201),
                )
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            if remaining.items.is_empty()
                && remaining.metadata.continue_.is_none_or(|v| v.is_empty())
                && self.deployment(spec).await?.is_none()
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        Ok(ObservedState {
            resource_id: spec.resource_id.to_string(),
            revision: spec.revision,
            phase: Phase::Absent,
            ready: true,
            observed_at: timestamp()?,
            runtime_id: None,
            conditions: vec![],
            storage: spec.volume.as_ref().map(|v| StorageObservation {
                volume_bytes: v.size_bytes,
                retained: true,
            }),
        })
    }
}

impl KubernetesAdapter {
    pub(super) async fn purge_release_storage(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> Result<ObservedState> {
        use k8s_openapi::api::{
            core::v1::{PersistentVolume, PersistentVolumeClaim, Service},
            storage::v1::StorageClass,
        };
        use kube::api::{DeleteParams, Patch, PatchParams, Preconditions};
        command.validate()?;
        guard.check_for(command).await?;
        let spec = &command.spec;
        self.validate(spec)?;
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        let mut ns = namespaces
            .get(&spec.namespace())
            .await
            .map_err(|_| ExecutionError::Ownership)?;
        spec.check_owner(&ns.metadata.labels.clone().unwrap_or_default())?;
        let namespace_uid = ns.metadata.uid.clone().ok_or(ExecutionError::Ownership)?;
        let services: Api<Service> = Api::namespaced(self.client.clone(), &spec.namespace());
        if self.deployment(spec).await?.is_some()
            || services
                .get_opt(&spec.name())
                .await
                .map_err(|_| ExecutionError::Runtime)?
                .is_some()
        {
            return Err(ExecutionError::Ownership);
        }
        let claim_name = format!("{}-data", spec.name());
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &spec.namespace());
        let pods = pods
            .list(&ListParams::default().limit(1001))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if pods.items.len() > 1000
            || pods.metadata.continue_.is_some_and(|s| !s.is_empty())
            || pods.items.iter().any(|p| {
                p.spec
                    .as_ref()
                    .and_then(|s| s.volumes.as_ref())
                    .is_some_and(|vs| {
                        vs.iter().any(|v| {
                            v.persistent_volume_claim
                                .as_ref()
                                .is_some_and(|v| v.claim_name == claim_name)
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
        let (claim_uid, pv_name) = if let Some(claim) = &claim {
            spec.check_owner(&claim.metadata.labels.clone().unwrap_or_default())?;
            let claim_spec = claim.spec.as_ref().ok_or(ExecutionError::Ownership)?;
            let requested = claim_spec
                .resources
                .as_ref()
                .and_then(|r| r.requests.as_ref())
                .and_then(|r| r.get("storage"))
                .ok_or(ExecutionError::Ownership)?;
            if Some(parse_quantity(&requested.0, false)?)
                != spec.volume.as_ref().and_then(|v| v.size_bytes)
            {
                return Err(ExecutionError::Ownership);
            }
            let uid = claim
                .metadata
                .uid
                .clone()
                .ok_or(ExecutionError::Ownership)?;
            let pv = claim_spec.volume_name.clone().unwrap_or_default();
            let annotations = ns.metadata.annotations.get_or_insert_default();
            if annotations
                .get("webdock.dev/storage-claim-uid")
                .is_some_and(|old| old != &uid)
                && annotations
                    .get("webdock.dev/storage-purged")
                    .is_none_or(|p| p != "true")
            {
                return Err(ExecutionError::Ownership);
            }
            annotations.insert("webdock.dev/storage-claim-uid".into(), uid.clone());
            annotations.insert("webdock.dev/storage-pv".into(), pv.clone());
            annotations.insert("webdock.dev/storage-purged".into(), "false".into());
            ns.status = None;
            guard.check().await?;
            namespaces
                .replace(&spec.namespace(), &PostParams::default(), &ns)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            (uid, pv)
        } else {
            let annotations = ns
                .metadata
                .annotations
                .as_ref()
                .ok_or(ExecutionError::Ownership)?;
            let uid = annotations
                .get("webdock.dev/storage-claim-uid")
                .cloned()
                .ok_or(ExecutionError::Ownership)?;
            let pv = annotations
                .get("webdock.dev/storage-pv")
                .cloned()
                .ok_or(ExecutionError::Ownership)?;
            (uid, pv)
        };
        if claim_uid.is_empty()
            || claim_uid.len() > 128
            || !claim_uid
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || pv_name.len() > 253
            || !pv_name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        {
            return Err(ExecutionError::Ownership);
        }
        if !pv_name.is_empty()
            && let Some(pv) = volumes
                .get_opt(&pv_name)
                .await
                .map_err(|_| ExecutionError::Runtime)?
        {
            let pv_spec = pv.spec.as_ref().ok_or(ExecutionError::Ownership)?;
            let reference = pv_spec
                .claim_ref
                .as_ref()
                .ok_or(ExecutionError::Ownership)?;
            if reference.uid.as_ref() != Some(&claim_uid)
                || reference.namespace.as_deref() != Some(&spec.namespace())
                || reference.name.as_deref() != Some(&claim_name)
            {
                return Err(ExecutionError::Ownership);
            }
            let class = pv_spec
                .storage_class_name
                .as_deref()
                .filter(|v| !v.is_empty())
                .ok_or(ExecutionError::Unsupported(
                    "static volume requires its storage helper",
                ))?;
            let classes: Api<StorageClass> = Api::all(self.client.clone());
            let class = classes
                .get(class)
                .await
                .map_err(|_| ExecutionError::Ownership)?;
            let provisioner = pv
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("pv.kubernetes.io/provisioned-by"))
                .map(String::as_str)
                .or_else(|| pv_spec.csi.as_ref().map(|c| c.driver.as_str()));
            if provisioner != Some(class.provisioner.as_str())
                || !matches!(
                    pv_spec.persistent_volume_reclaim_policy.as_deref(),
                    Some("Delete" | "Retain")
                )
            {
                return Err(ExecutionError::Ownership);
            }
            let mut labels = pv.metadata.labels.clone().unwrap_or_default();
            for (key, value) in spec.labels() {
                if labels.get(&key).is_some_and(|v| v != &value) {
                    return Err(ExecutionError::Ownership);
                }
                labels.insert(key, value);
            }
            // Explicit purge authorizes native dynamic-volume reclamation. Never remove finalizers
            // or manually delete the PV: the provisioner must finish backing-storage deletion.
            guard.check().await?;
            volumes.patch(&pv_name,&PatchParams::default(),&Patch::Merge(serde_json::json!({"metadata":{"uid":pv.metadata.uid,"resourceVersion":pv.metadata.resource_version,"labels":labels},"spec":{"persistentVolumeReclaimPolicy":"Delete"}}))).await.map_err(|_|ExecutionError::Runtime)?;
        }
        if let Some(claim) = claim {
            guard.check().await?;
            claims
                .delete(
                    &claim_name,
                    &DeleteParams {
                        preconditions: Some(Preconditions {
                            uid: Some(claim_uid.clone()),
                            resource_version: Some(
                                claim
                                    .metadata
                                    .resource_version
                                    .ok_or(ExecutionError::Ownership)?,
                            ),
                        }),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| ExecutionError::Runtime)?;
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            guard.check().await?;
            let claim = claims
                .get_opt(&claim_name)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            if claim
                .as_ref()
                .is_some_and(|c| c.metadata.uid.as_ref() != Some(&claim_uid))
            {
                return Err(ExecutionError::Ownership);
            }
            let pv = if pv_name.is_empty() {
                None
            } else {
                volumes
                    .get_opt(&pv_name)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?
            };
            if let Some(pv) = &pv {
                spec.check_owner(&pv.metadata.labels.clone().unwrap_or_default())?;
                if pv
                    .spec
                    .as_ref()
                    .and_then(|p| p.claim_ref.as_ref())
                    .and_then(|r| r.uid.as_ref())
                    != Some(&claim_uid)
                {
                    return Err(ExecutionError::Ownership);
                }
            }
            if claim.is_none() && pv.is_none() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        guard.check_for(command).await?;
        if self.deployment(spec).await?.is_some() {
            return Err(ExecutionError::Ownership);
        }
        let mut ns = namespaces
            .get(&spec.namespace())
            .await
            .map_err(|_| ExecutionError::Ownership)?;
        spec.check_owner(&ns.metadata.labels.clone().unwrap_or_default())?;
        if ns.metadata.uid.as_ref() != Some(&namespace_uid) {
            return Err(ExecutionError::Ownership);
        }
        ns.metadata
            .annotations
            .get_or_insert_default()
            .insert("webdock.dev/storage-purged".into(), "true".into());
        ns.status = None;
        guard.check().await?;
        namespaces
            .replace(&spec.namespace(), &PostParams::default(), &ns)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        Ok(ObservedState {
            resource_id: spec.resource_id.to_string(),
            revision: spec.revision,
            phase: Phase::Absent,
            ready: true,
            observed_at: timestamp()?,
            runtime_id: None,
            conditions: vec![],
            storage: Some(StorageObservation {
                volume_bytes: Some(0),
                retained: false,
            }),
        })
    }
}
