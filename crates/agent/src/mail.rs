//! Incoming Mail runtimes receive only a current authenticated Core claim and guard.
pub mod configuration;
#[cfg(feature = "docker")]
pub mod docker;
#[cfg(feature = "kubernetes")]
pub mod kubernetes;
use serde::Serialize;
use webdock_agent_protocol::mail::{MailCheckpoint, MailClaim, MailProof};
use webdock_supervisor::{ExecutionError, Result};

#[derive(Clone, Serialize)]
pub struct MailCapabilities {
    pub version: u8,
    pub image: String,
}
#[async_trait::async_trait]
pub trait MailAuthority: Send + Sync {
    async fn guard(&self, _claim: &MailClaim) -> Result<()> {
        Err(ExecutionError::Denied)
    }
    async fn checkpoint(&self, _claim: &MailClaim, _checkpoint: MailCheckpoint) -> Result<()> {
        Err(ExecutionError::Denied)
    }
}
#[async_trait::async_trait]
pub trait MailRuntime: Send + Sync {
    async fn capabilities(&self) -> Result<Option<MailCapabilities>> {
        Ok(None)
    }
    async fn execute(&self, claim: &MailClaim, authority: &dyn MailAuthority) -> Result<MailProof>;
}

#[cfg(any(feature = "docker", feature = "kubernetes"))]
mod client;
#[cfg(any(feature = "docker", feature = "kubernetes"))]
mod engine;
#[cfg(any(feature = "docker", feature = "kubernetes"))]
mod tls;
