//! Fixed managed Mail profile on an explicitly verified Kubernetes/storage connection.
mod config;
pub use config::{KubernetesMailConfig, RetainedBinding, RetainedPolicy, RetainedSecret};
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{
        LimitRange, Namespace, PersistentVolume, PersistentVolumeClaim, ResourceQuota, Secret,
        Service, ServiceAccount,
    },
    networking::v1::NetworkPolicy,
};
use webdock_agent_protocol::mail::MailClaim;
use webdock_supervisor::{ExecutionError, Result};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MailPhase {
    Recovery,
    Configure,
    Normal,
}
pub struct MailObjects {
    pub namespace: Namespace,
    pub deployment: Deployment,
    pub service: Service,
    pub volume: PersistentVolume,
    pub claim: PersistentVolumeClaim,
    pub secret: Option<Secret>,
    pub quota: ResourceQuota,
    pub limits: LimitRange,
    pub account: ServiceAccount,
    pub policies: Vec<NetworkPolicy>,
}
mod manifests;
pub use manifests::mail_objects;

use super::{MailAuthority, MailCapabilities, MailRuntime};
use crate::storage::{StorageClient, StorageConfig};
use kube::Client;
use std::path::PathBuf;
pub struct KubernetesMailRuntime {
    client: Client,
    storage: (StorageClient, StorageConfig),
    config: KubernetesMailConfig,
    config_path: PathBuf,
}
mod configuration;
mod lifecycle;
mod operations;
mod profile;
impl KubernetesMailRuntime {
    pub(crate) fn installation(&self) -> &str {
        &self.config.installation
    }

    fn verify_configuration(&self) -> Result<()> {
        let current = KubernetesMailConfig::load(&self.config_path)?;
        if serde_json::to_vec(&current).map_err(|_| ExecutionError::Runtime)?
            != serde_json::to_vec(&self.config).map_err(|_| ExecutionError::Runtime)?
        {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn capability(&self) -> Result<Option<MailCapabilities>> {
        use k8s_openapi::api::core::v1::Node;
        use kube::{Api, api::ListParams};
        self.verify_configuration()?;
        let nodes = Api::<Node>::all(self.client.clone())
            .list(&ListParams::default().limit(2))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if nodes.items.len() != 1
            || nodes
                .metadata
                .continue_
                .as_ref()
                .is_some_and(|v| !v.is_empty())
        {
            return Ok(None);
        }
        let node = &nodes.items[0];
        let ready = node
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .is_some_and(|conditions| {
                conditions
                    .iter()
                    .any(|condition| condition.type_ == "Ready" && condition.status == "True")
            });
        if node.metadata.name.as_deref() != Some(&self.config.node)
            || !ready
            || self.storage.0.information().await? != self.storage.1
        {
            return Ok(None);
        }
        let storage = self
            .storage
            .0
            .capability(std::slice::from_ref(&self.config.node))
            .await?;
        if !storage.ready || storage.capacity_bytes != self.storage.1.capacity_bytes {
            return Ok(None);
        }
        Ok(Some(MailCapabilities {
            version: 1,
            image: self.config.image.clone(),
        }))
    }

    pub fn open(
        client: Client,
        storage: (StorageClient, StorageConfig),
        config_path: PathBuf,
    ) -> Result<Self> {
        let config = KubernetesMailConfig::load(&config_path)?;
        storage.1.validate()?;
        if storage.1.node != config.node
            || storage.1.capacity_bytes
                < webdock_agent_protocol::mail::MailResources::FIXED.volume_bytes
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(Self {
            client,
            storage,
            config,
            config_path,
        })
    }
}
#[async_trait::async_trait]
impl MailRuntime for KubernetesMailRuntime {
    async fn capabilities(&self) -> Result<Option<MailCapabilities>> {
        tokio::time::timeout(std::time::Duration::from_secs(10), self.capability())
            .await
            .map_err(|_| ExecutionError::Expired)?
    }
    async fn execute(
        &self,
        claim: &MailClaim,
        authority: &dyn MailAuthority,
    ) -> Result<webdock_agent_protocol::mail::MailProof> {
        let execution = operations::Execution::new(self, claim, authority);
        execution.guard().await?;
        if execution.guarded(self.capabilities()).await?.is_none() {
            return Err(ExecutionError::Denied);
        }
        execution.run().await
    }
}
#[cfg(test)]
mod profile_tests;
