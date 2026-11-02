use super::operations::{Execution, Kind, contains};
use super::*;
use crate::mail::{client::Client as MailClient, engine, tls};
use kube::api::DynamicObject;
use serde_json::{Value, json};
use std::{net::Ipv4Addr, time::Duration};
use webdock_agent_protocol::mail::{
    MailAction, MailCheckpoint, MailEdition, MailMaterial, MailProof,
};
pub(super) fn recovery_env(pod: &Value) -> bool {
    pod["containers"][0]["env"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["name"] == "STALWART_RECOVERY_ADMIN")
    })
}
pub(super) use super::profile::validate_workload;

fn node_ready(observed: Option<&str>, expected: &str) -> Result<bool> {
    match observed {
        None | Some("") => Ok(false),
        Some(node) if node == expected => Ok(true),
        Some(_) => Err(ExecutionError::Ownership),
    }
}
pub(super) fn validate_observed_pod(
    deployment: &DynamicObject,
    pod: &DynamicObject,
    claim: &MailClaim,
    config: &KubernetesMailConfig,
) -> Result<()> {
    let mut observed = deployment.clone();
    let reviewed_transition = config
        .retained
        .get(&claim.spec.customer_id)
        .is_some_and(|binding| deployment.metadata.uid.as_ref() == Some(&binding.deployment_uid))
        && deployment.data["spec"]["template"]["spec"]["containers"][0]["imagePullPolicy"]
            == "IfNotPresent"
        && deployment.data["spec"]["template"]["spec"]["nodeSelector"]
            == json!({"kubernetes.io/hostname":config.node})
        && (pod.data["spec"]["containers"][0]["imagePullPolicy"] == "Never"
            || pod.data["spec"].get("nodeSelector").is_none());
    if reviewed_transition {
        // Owner-chain verification precedes this check. Only the reviewed old profile may
        // wait through Recreate; ready() still requires the final native template exactly.
        if let Some(labels) = &mut observed.metadata.labels {
            labels.remove("webdock.dev/mail-installation");
        }
    }
    observed.data["spec"]["template"]["spec"] = pod.data["spec"].clone();
    observed.data["spec"]["template"]["metadata"]["labels"] =
        serde_json::to_value(&pod.metadata.labels).map_err(|_| ExecutionError::Runtime)?;
    validate_workload(&observed, claim, config)
}
impl Execution<'_> {
    fn objects(&self, phase: MailPhase) -> Result<MailObjects> {
        mail_objects(
            self.claim,
            &self.runtime.config,
            &self.runtime.storage.1,
            phase,
        )
    }
    pub(super) async fn client(&self, user: &str, password: &str) -> Result<MailClient> {
        self.ready(1, None).await?;
        let service = self
            .get(Kind::Service, &self.name())
            .await?
            .ok_or(ExecutionError::Ownership)?;
        let expected = serde_json::to_value(self.objects(MailPhase::Normal)?.service)
            .map_err(|_| ExecutionError::Runtime)?;
        self.validate_existing(Kind::Service, &service, &expected)?;
        let spec = &service.data["spec"];
        if spec["type"] != "ClusterIP"
            || spec["selector"] != json!({"webdock.dev/app-id":self.claim.spec.customer_id})
            || spec["externalIPs"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        {
            return Err(ExecutionError::Ownership);
        }
        let ip = spec["clusterIP"]
            .as_str()
            .and_then(|value| value.parse::<Ipv4Addr>().ok())
            .filter(Ipv4Addr::is_private)
            .ok_or(ExecutionError::Ownership)?;
        MailClient::new(ip, user, password)
    }
    pub(super) async fn address(&self) -> Result<Ipv4Addr> {
        let service = self
            .get(Kind::Service, &self.name())
            .await?
            .ok_or(ExecutionError::Ownership)?;
        let expected = serde_json::to_value(self.objects(MailPhase::Normal)?.service)
            .map_err(|_| ExecutionError::Runtime)?;
        self.validate_existing(Kind::Service, &service, &expected)?;
        service.data["spec"]["clusterIP"]
            .as_str()
            .and_then(|v| v.parse::<Ipv4Addr>().ok())
            .filter(Ipv4Addr::is_private)
            .ok_or(ExecutionError::Ownership)
    }
    async fn owned_pods(&self, deployment: &DynamicObject) -> Result<Vec<DynamicObject>> {
        let pods = self
            .list(
                Kind::Pod,
                Some(&format!(
                    "webdock.dev/app-id={}",
                    self.claim.spec.customer_id
                )),
            )
            .await?;
        for pod in &pods {
            self.capture(Kind::Pod, pod)?;
            if pod.metadata.deletion_timestamp.is_some() {
                continue;
            }
            let reference = pod
                .metadata
                .owner_references
                .as_ref()
                .and_then(|r| {
                    r.iter()
                        .find(|r| r.controller == Some(true) && r.kind == "ReplicaSet")
                })
                .ok_or(ExecutionError::Ownership)?;
            let rs = self
                .guarded(async {
                    self.api(Kind::ReplicaSet)
                        .get(&reference.name)
                        .await
                        .map_err(|_| ExecutionError::Runtime)
                })
                .await?;
            self.capture(Kind::ReplicaSet, &rs)?;
            if rs.metadata.uid.as_ref() != Some(&reference.uid)
                || !rs.metadata.owner_references.as_ref().is_some_and(|r| {
                    r.iter().any(|r| {
                        r.controller == Some(true)
                            && r.kind == "Deployment"
                            && Some(&r.uid) == deployment.metadata.uid.as_ref()
                    })
                })
            {
                return Err(ExecutionError::Ownership);
            }
            validate_observed_pod(deployment, pod, self.claim, &self.runtime.config)?;
            node_ready(
                pod.data["spec"]["nodeName"].as_str(),
                &self.runtime.config.node,
            )?;
        }
        Ok(pods)
    }
    pub(super) async fn ready(
        &self,
        replicas: i64,
        previous_pod: Option<&str>,
    ) -> Result<DynamicObject> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(65);
        loop {
            self.guard().await?;
            let deployment = self
                .get(Kind::Deployment, &self.name())
                .await?
                .ok_or(ExecutionError::Ownership)?;
            validate_workload(&deployment, self.claim, &self.runtime.config)?;
            let pods = self.owned_pods(&deployment).await?;
            let current = deployment.data["status"]["observedGeneration"]
                .as_i64()
                .unwrap_or(0)
                >= deployment.metadata.generation.unwrap_or(1)
                && deployment.data["status"]["updatedReplicas"]
                    .as_i64()
                    .unwrap_or(0)
                    == replicas
                && deployment.data["status"]["availableReplicas"]
                    .as_i64()
                    .unwrap_or(0)
                    == replicas;
            if current
                && i64::try_from(pods.len()).ok() == Some(replicas)
                && pods.iter().all(|pod| {
                    pod.metadata.deletion_timestamp.is_none()
                        && pod.data["spec"]["nodeName"].as_str()
                            == Some(self.runtime.config.node.as_str())
                        && contains(
                            &pod.data["spec"],
                            &deployment.data["spec"]["template"]["spec"],
                        )
                        && pod.metadata.uid.as_deref() != previous_pod
                        && pod.data["status"]["conditions"]
                            .as_array()
                            .is_some_and(|conditions| {
                                conditions
                                    .iter()
                                    .any(|c| c["type"] == "Ready" && c["status"] == "True")
                            })
                })
            {
                return Ok(deployment);
            }
            if tokio::time::Instant::now() >= deadline {
                let states=pods.iter().map(|pod|json!({"uid":pod.metadata.uid,"phase":pod.data["status"]["phase"],"node":pod.data["spec"]["nodeName"],"conditions":pod.data["status"]["conditions"].as_array().map(|rows|rows.iter().map(|row|json!({"type":row["type"],"status":row["status"],"reason":row["reason"]})).collect::<Vec<_>>()),"waiting":pod.data["status"]["containerStatuses"].as_array().map(|rows|rows.iter().map(|row|json!({"reason":row["state"]["waiting"]["reason"],"exitCode":row["state"]["terminated"]["exitCode"]})).collect::<Vec<_>>()),"profileMismatch":super::operations::mismatch_path(&pod.data["spec"],&deployment.data["spec"]["template"]["spec"],"pod")})).collect::<Vec<_>>();
                tracing::warn!(phase="ready-timeout",desired_replicas=replicas,generation=?deployment.metadata.generation,observed_generation=?deployment.data["status"]["observedGeneration"],updated=?deployment.data["status"]["updatedReplicas"],available=?deployment.data["status"]["availableReplicas"],pods=?states,"Mail readiness was not observed");
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    async fn restart_recovery(&self, deployment: &DynamicObject) -> Result<()> {
        let pods = self.owned_pods(deployment).await?;
        if pods.len() != 1 {
            return Err(ExecutionError::Ownership);
        }
        let pod = &pods[0];
        let old = pod
            .metadata
            .uid
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        self.delete(Kind::Pod, pod).await?;
        self.ready(1, Some(old)).await?;
        Ok(())
    }
    async fn preflight(
        &self,
        objects: &MailObjects,
        namespace_exists: bool,
        initial: bool,
    ) -> Result<()> {
        for (kind, desired) in [
            (Kind::Volume, serde_json::to_value(&objects.volume)),
            (Kind::Claim, serde_json::to_value(&objects.claim)),
            (Kind::Service, serde_json::to_value(&objects.service)),
        ] {
            let desired = desired.map_err(|_| ExecutionError::Runtime)?;
            let name = desired["metadata"]["name"]
                .as_str()
                .ok_or(ExecutionError::Invalid("Mail resource"))?;
            if let Some(current) = self.get(kind, name).await? {
                self.validate_existing(kind, &current, &desired)?;
            } else if !initial {
                return Err(ExecutionError::Ownership);
            }
        }
        if namespace_exists {
            self.verify_policies(objects).await?;
        }
        Ok(())
    }
    async fn namespace_objects(&self, objects: &MailObjects) -> Result<()> {
        let mut namespace = objects.namespace.clone();
        namespace
            .metadata
            .labels
            .as_mut()
            .ok_or(ExecutionError::Ownership)?
            .insert("security.webdock.dev/ready".into(), "false".into());
        self.apply(Kind::Namespace, &namespace).await?;
        self.apply(Kind::Quota, &objects.quota).await?;
        self.apply(Kind::Limits, &objects.limits).await?;
        self.apply(Kind::Account, &objects.account).await?;
        self.verify_policies(objects).await?;
        for policy in &objects.policies {
            self.apply(Kind::Policy, policy).await?;
        }
        self.apply(Kind::Namespace, &objects.namespace).await?;
        Ok(())
    }
    async fn verify_policies(&self, objects: &MailObjects) -> Result<()> {
        for policy in self.list(Kind::Policy, None).await? {
            self.owner(Kind::Policy, &policy)?;
            let name = policy
                .metadata
                .name
                .as_deref()
                .ok_or(ExecutionError::Ownership)?;
            if objects
                .policies
                .iter()
                .any(|p| p.metadata.name.as_deref() == Some(name))
            {
                continue;
            }
            let expected = self
                .runtime
                .config
                .retained
                .get(&self.claim.spec.customer_id)
                .and_then(|b| b.policies.get(name))
                .ok_or(ExecutionError::Ownership)?;
            if policy.metadata.uid.as_ref() != Some(&expected.uid)
                || manifests::hash(&policy.data["spec"])? != expected.spec_sha256
            {
                return Err(ExecutionError::Ownership);
            }
        }
        Ok(())
    }
    async fn storage(&self, objects: &MailObjects, initial: bool) -> Result<()> {
        let pv = objects
            .volume
            .metadata
            .name
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        let pvc = objects
            .claim
            .metadata
            .name
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        let volume = self.get(Kind::Volume, pv).await?;
        let current_claim = self.get(Kind::Claim, pvc).await?;
        if !initial && (volume.is_none() || current_claim.is_none()) {
            return Err(ExecutionError::Ownership);
        }
        if let (Some(volume), Some(current_claim)) = (&volume, &current_claim)
            && volume.data["spec"]["claimRef"]["uid"]
                .as_str()
                .is_some_and(|uid| current_claim.metadata.uid.as_deref() != Some(uid))
        {
            return Err(ExecutionError::Ownership);
        }
        let packet = crate::storage::StoragePacket::new(
            &self.claim.spec.customer_id,
            &self.claim.spec.customer_id,
            &self.claim.spec.cluster_id,
            if initial {
                1
            } else {
                u64::try_from(self.claim.spec.revision)
                    .map_err(|_| ExecutionError::Invalid("Mail revision"))?
            },
            self.claim.spec.resources.volume_bytes,
            u64::try_from(self.claim.lease_until.timestamp())
                .map_err(|_| ExecutionError::Expired)?,
        )?;
        if !initial {
            self.guarded(self.runtime.storage.0.retained(&packet))
                .await?;
        }
        let (config, path) = self.guarded(self.runtime.storage.0.ensure(&packet)).await?;
        if config != self.runtime.storage.1
            || path
                != config
                    .root
                    .join(&self.claim.spec.customer_id)
                    .join("mount/data")
        {
            return Err(ExecutionError::Ownership);
        }
        self.apply(Kind::Volume, &objects.volume).await?;
        self.apply(Kind::Claim, &objects.claim).await?;
        Ok(())
    }
    async fn bootstrap_secret(
        &self,
        objects: &mut MailObjects,
        current: Option<&DynamicObject>,
    ) -> Result<()> {
        let Some(secret) = objects.secret.as_mut() else {
            return Ok(());
        };
        if let Some(current) = current
            && recovery_env(&current.data["spec"]["template"]["spec"])
        {
            let env = current.data["spec"]["template"]["spec"]["containers"][0]["env"]
                .as_array()
                .ok_or(ExecutionError::Ownership)?;
            let name = env
                .iter()
                .find(|row| row["name"] == "STALWART_RECOVERY_ADMIN")
                .and_then(|row| row["valueFrom"]["secretKeyRef"]["name"].as_str())
                .ok_or(ExecutionError::Ownership)?;
            self.get(Kind::Secret, name)
                .await?
                .ok_or(ExecutionError::Ownership)?;
            secret.metadata.name = Some(name.into());
            let pod = objects
                .deployment
                .spec
                .as_mut()
                .ok_or(ExecutionError::Ownership)?
                .template
                .spec
                .as_mut()
                .ok_or(ExecutionError::Ownership)?;
            let env = pod.containers[0]
                .env
                .as_mut()
                .ok_or(ExecutionError::Ownership)?
                .iter_mut()
                .find(|e| e.name == "STALWART_RECOVERY_ADMIN")
                .ok_or(ExecutionError::Ownership)?;
            env.value_from
                .as_mut()
                .and_then(|e| e.secret_key_ref.as_mut())
                .ok_or(ExecutionError::Ownership)?
                .name = name.into();
        }
        self.apply(Kind::Secret, secret).await?;
        Ok(())
    }
    pub(super) async fn run(&self) -> Result<MailProof> {
        self.guard().await?;
        if matches!(self.claim.material, MailMaterial::Unallocated {}) {
            if self.claim.spec.action != MailAction::Suspend {
                return Err(ExecutionError::Denied);
            }
            let namespace = self
                .guarded(async {
                    self.api(Kind::Namespace)
                        .get_opt(&self.namespace())
                        .await
                        .map_err(|_| ExecutionError::Runtime)
                })
                .await?;
            if namespace.is_some() {
                return Err(ExecutionError::Ownership);
            }
            return Ok(self.proof("unallocated".into(), false, None));
        }
        let namespace = self.get(Kind::Namespace, &self.namespace()).await?;
        let current = if namespace.is_some() {
            self.get(Kind::Deployment, &self.name()).await?
        } else {
            None
        };
        if let Some(current) = &current {
            validate_workload(current, self.claim, &self.runtime.config)?;
        }
        if self.claim.spec.action == MailAction::Suspend {
            let Some(current) = current else {
                if namespace.is_some() {
                    return Err(ExecutionError::Ownership);
                }
                return Ok(self.proof("unallocated".into(), false, None));
            };
            validate_workload(&current, self.claim, &self.runtime.config)?;
            let objects = self.objects(MailPhase::Normal)?;
            self.apply(Kind::Deployment, &objects.deployment).await?;
            let observed = self.ready(0, None).await?;
            return Ok(self.proof(
                observed.metadata.uid.ok_or(ExecutionError::Ownership)?,
                false,
                None,
            ));
        }
        let MailMaterial::Credentials {
            bootstrap_password,
            username,
            password,
        } = &self.claim.material
        else {
            return Err(ExecutionError::Denied);
        };
        let initial = bootstrap_password.is_some() && self.claim.checkpoint.is_none();
        if !initial && (namespace.is_none() || current.is_none()) {
            return Err(ExecutionError::Ownership);
        }
        let recovery = current
            .as_ref()
            .is_some_and(|d| recovery_env(&d.data["spec"]["template"]["spec"]))
            || (initial
                && current
                    .as_ref()
                    .is_none_or(|d| d.data["spec"]["replicas"] == 0));
        if current.as_ref().is_some_and(|d| {
            d.data["spec"]["replicas"] == 1
                && d.metadata
                    .labels
                    .as_ref()
                    .is_none_or(|l| !l.contains_key("webdock.dev/mail-installation"))
        }) {
            let client = self.client(username, password).await?;
            self.guarded(client.verify()).await?;
        }
        let phase = if recovery {
            MailPhase::Recovery
        } else {
            MailPhase::Normal
        };
        let mut objects = self.objects(phase)?;
        self.preflight(&objects, namespace.is_some(), initial)
            .await?;
        tracing::debug!(phase = "namespace", "Mail lifecycle stage");
        self.namespace_objects(&objects).await?;
        tracing::debug!(phase = "storage", "Mail lifecycle stage");
        self.storage(&objects, initial).await?;
        if recovery {
            self.bootstrap_secret(&mut objects, current.as_ref())
                .await?;
        }
        self.apply(Kind::Service, &objects.service).await?;
        let deployment = self.apply(Kind::Deployment, &objects.deployment).await?;
        let uid = deployment
            .metadata
            .uid
            .clone()
            .ok_or(ExecutionError::Ownership)?;
        tracing::debug!(phase = "initial-ready", "Mail lifecycle stage");
        let mut deployment = self.ready(1, None).await?;
        if recovery {
            let bootstrap = bootstrap_password
                .as_deref()
                .ok_or(ExecutionError::Ownership)?;
            if self.claim.checkpoint.is_none() {
                if current.is_some() {
                    self.restart_recovery(&deployment).await?;
                }
                let admin = self.client("admin", bootstrap).await?;
                if admin
                    .call::<crate::mail::client::Rows<crate::mail::client::Domain>>(
                        "x:Domain/get",
                        json!({"ids":null}),
                    )
                    .await
                    .is_err()
                {
                    self.guard().await?;
                    let _:Value=admin.call("x:Bootstrap/set",json!({"update":{"singleton":{"serverHostname":self.claim.spec.hostname,"defaultDomain":self.claim.spec.hostname,"requestTlsCertificate":false,"generateDkimKeys":false,"dataStore":{"@type":"RocksDb","path":"/data/store"},"tracer":{"@type":"Stdout","level":"warn"}}}})).await?;
                    deployment = self
                        .get(Kind::Deployment, &self.name())
                        .await?
                        .ok_or(ExecutionError::Ownership)?;
                    self.restart_recovery(&deployment).await?;
                }
                let admin = self.client("admin", bootstrap).await?;
                tracing::debug!(phase = "engine-account", "Mail lifecycle stage");
                engine::configure_account(self.claim, self, &admin, username, password).await?;
                tracing::debug!(phase = "engine-listeners", "Mail lifecycle stage");
                engine::configure_listeners(self.claim, self, &admin).await?;
                tracing::debug!(phase = "engine-certificate", "Mail lifecycle stage");
                engine::configure_certificate(self.claim, self, &admin).await?;
                self.authority
                    .checkpoint(
                        self.claim,
                        MailCheckpoint {
                            lease: self.claim.lease.clone(),
                            runtime_id: uid.clone(),
                            material_sha256: self.claim.spec.material_sha256.clone(),
                        },
                    )
                    .await?;
            } else {
                tracing::debug!(
                    phase = "checkpoint-readback",
                    "Mail preparation already has accepted custody"
                );
                let admin = self.client("admin", bootstrap).await?;
                self.guarded(admin.certificate_pin(&self.claim.spec.hostname))
                    .await?;
            }
            self.remove_recovery().await?;
            let normal = self.objects(MailPhase::Normal)?;
            self.apply(Kind::Deployment, &normal.deployment).await?;
            self.ready(1, None).await?;
        }
        let permanent = self.client(username, password).await?;
        self.guarded(permanent.verify()).await?;
        if let Some(bootstrap) = bootstrap_password
            && self
                .client("admin", bootstrap)
                .await?
                .verify()
                .await
                .is_ok()
        {
            return Err(ExecutionError::Ownership);
        }
        let ip = self.address().await?;
        let pin = self.guarded(tls::certificate(ip)).await?;
        if self
            .guarded(permanent.certificate_pin(&self.claim.spec.hostname))
            .await?
            != pin
        {
            return Err(ExecutionError::Ownership);
        }
        let observed = self.ready(1, None).await?;
        if observed.metadata.uid.as_ref() != Some(&uid)
            || recovery_env(&observed.data["spec"]["template"]["spec"])
        {
            return Err(ExecutionError::Ownership);
        }
        if !recovery && self.claim.checkpoint.is_none() {
            self.authority
                .checkpoint(
                    self.claim,
                    MailCheckpoint {
                        lease: self.claim.lease.clone(),
                        runtime_id: uid.clone(),
                        material_sha256: self.claim.spec.material_sha256.clone(),
                    },
                )
                .await?;
        }
        if let Some(secret) = objects.secret.as_ref() {
            let name = secret
                .metadata
                .name
                .as_deref()
                .ok_or(ExecutionError::Ownership)?;
            if let Some(current) = self.get(Kind::Secret, name).await? {
                self.delete(Kind::Secret, &current).await?;
            }
        }
        self.guard().await?;
        Ok(self.proof(uid, true, Some(pin)))
    }
    fn proof(&self, uid: String, running: bool, pin: Option<String>) -> MailProof {
        MailProof {
            runtime_id: uid,
            namespace: self.namespace(),
            revision: self.claim.spec.revision,
            running,
            edition: MailEdition::Community,
            recovery_disabled: true,
            submission_certificate_sha256: pin,
            private_ipv4: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unscheduled_pods_wait_but_a_different_node_is_not_adopted() {
        assert!(!node_ready(None, "owned-node").unwrap());
        assert!(!node_ready(Some(""), "owned-node").unwrap());
        assert!(node_ready(Some("owned-node"), "owned-node").unwrap());
        assert!(matches!(
            node_ready(Some("foreign-node"), "owned-node"),
            Err(ExecutionError::Ownership)
        ));
    }
}
