//! Configuration acts only on a verified existing normal instance; lifecycle allocation is separate.
use async_trait::async_trait;
use webdock_agent_protocol::mail_configuration::{
    ConfigurationCheckpoint, ConfigurationClaim, ConfigurationProof,
};
use webdock_supervisor::{ExecutionError, Result};

#[async_trait]
pub trait ConfigurationAuthority: Send + Sync {
    async fn guard(&self, _claim: &ConfigurationClaim) -> Result<()> {
        Err(ExecutionError::Denied)
    }
    async fn checkpoint(
        &self,
        _claim: &ConfigurationClaim,
        _checkpoint: ConfigurationCheckpoint,
    ) -> Result<()> {
        Err(ExecutionError::Denied)
    }
}
#[async_trait]
pub trait ConfigurationRuntime: Send + Sync {
    async fn configuration_capable(&self) -> Result<bool> {
        Ok(false)
    }
    async fn configure(
        &self,
        claim: &ConfigurationClaim,
        authority: &dyn ConfigurationAuthority,
    ) -> Result<ConfigurationProof>;
}

#[cfg(any(feature = "docker", feature = "kubernetes"))]
pub(super) async fn guard(
    claim: &ConfigurationClaim,
    authority: &dyn ConfigurationAuthority,
) -> Result<()> {
    claim
        .validate(chrono::Utc::now())
        .map_err(|_| ExecutionError::Expired)?;
    authority.guard(claim).await?;
    claim
        .validate(chrono::Utc::now())
        .map_err(|_| ExecutionError::Expired)
}

#[cfg(any(feature = "docker", feature = "kubernetes"))]
mod engine;
#[cfg(any(feature = "docker", feature = "kubernetes"))]
mod inspection;
#[cfg(any(feature = "docker", feature = "kubernetes"))]
pub(super) use engine::execute;
#[cfg(feature = "kubernetes")]
pub(super) use inspection::InspectionAuthority;
#[cfg(any(feature = "docker", feature = "kubernetes"))]
pub(super) use inspection::{Inspection, Inspector, inspection_claim};
