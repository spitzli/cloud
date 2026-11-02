use super::operations::{Execution, Kind};
use super::*;
use crate::mail::{
    configuration::{
        self, ConfigurationAuthority, ConfigurationRuntime, Inspection, InspectionAuthority,
        Inspector, inspection_claim,
    },
    tls,
};
use webdock_agent_protocol::mail_configuration::{ConfigurationClaim, ConfigurationProof};
#[async_trait::async_trait]
impl Inspector for KubernetesMailRuntime {
    async fn inspect_configuration(
        &self,
        claim: &ConfigurationClaim,
        expected_runtime_id: Option<&str>,
        authority: &dyn ConfigurationAuthority,
    ) -> Result<Inspection> {
        configuration::guard(claim, authority).await?;
        let mut view = inspection_claim(claim)?;
        if let Some(expected) = expected_runtime_id {
            view.checkpoint = Some(webdock_agent_protocol::mail::MailCheckpoint {
                lease: claim.lease.clone(),
                runtime_id: expected.into(),
                material_sha256: claim.spec.target.management_sha256.clone(),
            });
        }
        let guard = InspectionAuthority { claim, authority };
        let execution = Execution::new(self, &view, &guard);
        execution.guard().await?;
        execution
            .get(Kind::Namespace, &execution.namespace())
            .await?
            .ok_or(ExecutionError::Ownership)?;
        let deployment = execution
            .get(Kind::Deployment, &execution.name())
            .await?
            .ok_or(ExecutionError::Ownership)?;
        profile::validate_workload(&deployment, &view, &self.config)?;
        if deployment.data["spec"]["replicas"] != 1
            || lifecycle::recovery_env(&deployment.data["spec"]["template"]["spec"])
        {
            return Err(ExecutionError::Ownership);
        }
        let deployment = execution.ready(1, None).await?;
        let id = deployment.metadata.uid.ok_or(ExecutionError::Ownership)?;
        let ip = execution.address().await?;
        let pin = execution.guarded(tls::certificate(ip)).await?;
        if pin != claim.spec.target.certificate_sha256 {
            return Err(ExecutionError::Ownership);
        }
        let client = execution
            .client(
                &claim.material.administrator.username,
                &claim.material.administrator.password,
            )
            .await?;
        execution.guarded(client.verify()).await?;
        if execution
            .guarded(client.certificate_pin(&view.spec.hostname))
            .await?
            != pin
        {
            return Err(ExecutionError::Ownership);
        }
        execution.guard().await?;
        Ok(Inspection {
            runtime_id: id,
            client,
        })
    }
}
#[async_trait::async_trait]
impl ConfigurationRuntime for KubernetesMailRuntime {
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
