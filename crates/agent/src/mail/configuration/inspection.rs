use super::*;
#[cfg(feature = "kubernetes")]
use crate::mail::MailAuthority;
use crate::mail::client::Client;
use webdock_agent_protocol::mail::{MailAction, MailCheckpoint, MailClaim, MailMaterial, MailSpec};
use webdock_agent_protocol::mail_configuration::ConfigurationCheckpointPhase;

pub(in crate::mail) struct Inspection {
    pub runtime_id: String,
    pub client: Client,
}
#[async_trait]
pub(in crate::mail) trait Inspector: Send + Sync {
    async fn inspect_configuration(
        &self,
        claim: &ConfigurationClaim,
        expected_runtime_id: Option<&str>,
        authority: &dyn ConfigurationAuthority,
    ) -> Result<Inspection>;
}

/// A read-only view for the existing ownership/profile inspectors. It is never executed.
pub(in crate::mail) fn inspection_claim(claim: &ConfigurationClaim) -> Result<MailClaim> {
    claim
        .validate(chrono::Utc::now())
        .map_err(|_| ExecutionError::Denied)?;
    let checkpoint = claim
        .checkpoints
        .iter()
        .find(|checkpoint| matches!(checkpoint.phase, ConfigurationCheckpointPhase::Bound {}))
        .map(|checkpoint| MailCheckpoint {
            lease: claim.lease.clone(),
            runtime_id: checkpoint.runtime_id.clone(),
            material_sha256: claim.spec.target.management_sha256.clone(),
        });
    Ok(MailClaim {
        spec: MailSpec {
            version: claim.spec.version,
            operation_id: claim.spec.operation_id.clone(),
            customer_id: claim.spec.customer_id.clone(),
            cluster_id: claim.spec.cluster_id.clone(),
            revision: claim
                .spec
                .service_revision
                .parse()
                .map_err(|_| ExecutionError::Denied)?,
            agent_generation: claim.spec.agent_generation,
            instance_key: claim.spec.target.instance_key.clone(),
            hostname: claim.spec.target.hostname.clone(),
            image: claim.spec.target.image.clone(),
            resources: claim.spec.target.resources,
            material_sha256: claim.spec.target.management_sha256.clone(),
            action: MailAction::Ensure,
        },
        material: MailMaterial::Credentials {
            bootstrap_password: None,
            username: claim.material.administrator.username.clone(),
            password: claim.material.administrator.password.clone(),
        },
        lease: claim.lease.clone(),
        lease_until: claim.lease_until,
        checkpoint,
    })
}

#[cfg(feature = "kubernetes")]
pub(in crate::mail) struct InspectionAuthority<'a> {
    pub claim: &'a ConfigurationClaim,
    pub authority: &'a dyn ConfigurationAuthority,
}
#[cfg(feature = "kubernetes")]
#[async_trait]
impl MailAuthority for InspectionAuthority<'_> {
    async fn guard(&self, view: &MailClaim) -> Result<()> {
        if view
            .spec
            .fingerprint()
            .map_err(|_| ExecutionError::Denied)?
            != inspection_claim(self.claim)?
                .spec
                .fingerprint()
                .map_err(|_| ExecutionError::Denied)?
            || view.lease.lease_token != self.claim.lease.lease_token
        {
            return Err(ExecutionError::Denied);
        }
        super::guard(self.claim, self.authority).await
    }
}
