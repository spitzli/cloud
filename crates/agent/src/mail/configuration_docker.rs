use super::*;
use crate::mail::configuration::{
    self, ConfigurationAuthority, ConfigurationRuntime, Inspection, Inspector, inspection_claim,
};
use webdock_agent_protocol::mail_configuration::{ConfigurationClaim, ConfigurationProof};

#[async_trait::async_trait]
impl Inspector for DockerMailRuntime {
    async fn inspect_configuration(
        &self,
        claim: &ConfigurationClaim,
        expected_runtime_id: Option<&str>,
        authority: &dyn ConfigurationAuthority,
    ) -> Result<Inspection> {
        configuration::guard(claim, authority).await?;
        let view = inspection_claim(claim)?;
        if view.spec.image != self.image {
            return Err(ExecutionError::Ownership);
        }
        let container = self
            .container(&view)
            .await?
            .ok_or(ExecutionError::Ownership)?;
        if self.recovery(&container)
            || container.state.as_ref().and_then(|state| state.running) != Some(true)
        {
            return Err(ExecutionError::Ownership);
        }
        let id = container.id.ok_or(ExecutionError::Ownership)?;
        if expected_runtime_id.is_some_and(|expected| expected != id) {
            return Err(ExecutionError::Ownership);
        }
        if view
            .checkpoint
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.runtime_id != id)
        {
            return Err(ExecutionError::Ownership);
        }
        let volume = self
            .docker
            .inspect_volume(&format!("{}-data", self.name(&view)))
            .await
            .map_err(|_| ExecutionError::Ownership)?;
        self.owner(&volume.labels, &view)?;
        if volume.driver != "local" || !volume.options.is_empty() {
            return Err(ExecutionError::Ownership);
        }
        let network = self
            .docker
            .inspect_network(&self.name(&view), None::<InspectNetworkOptions>)
            .await
            .map_err(|_| ExecutionError::Ownership)?;
        self.owner(
            network.labels.as_ref().ok_or(ExecutionError::Ownership)?,
            &view,
        )?;
        if network.internal != Some(true)
            || network.driver.as_deref() != Some("bridge")
            || network
                .options
                .as_ref()
                .is_some_and(|options| !options.is_empty())
            || network
                .containers
                .as_ref()
                .is_none_or(|peers| peers.len() != 1 || !peers.contains_key(&id))
        {
            return Err(ExecutionError::Ownership);
        }
        let (observed, ip) = self.address(&view).await?;
        if observed != id {
            return Err(ExecutionError::Ownership);
        }
        configuration::guard(claim, authority).await?;
        let pin = tls::certificate(ip).await?;
        if pin != claim.spec.target.certificate_sha256 {
            return Err(ExecutionError::Ownership);
        }
        let client = client::Client::new(
            ip,
            &claim.material.administrator.username,
            &claim.material.administrator.password,
        )?;
        configuration::guard(claim, authority).await?;
        client.verify().await?;
        if client.certificate_pin(&view.spec.hostname).await? != pin {
            return Err(ExecutionError::Ownership);
        }
        configuration::guard(claim, authority).await?;
        Ok(Inspection {
            runtime_id: id,
            client,
        })
    }
}
#[async_trait::async_trait]
impl ConfigurationRuntime for DockerMailRuntime {
    async fn configuration_capable(&self) -> Result<bool> {
        Ok(MailRuntime::capabilities(self).await?.is_some())
    }
    async fn configure(
        &self,
        claim: &ConfigurationClaim,
        authority: &dyn ConfigurationAuthority,
    ) -> Result<ConfigurationProof> {
        configuration::execute(self, claim, authority).await
    }
}
