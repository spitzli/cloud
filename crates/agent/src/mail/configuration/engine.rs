use super::*;
use crate::mail::client::Rows;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use webdock_agent_protocol::mail_configuration::*;
#[path = "types.rs"]
mod types;
use types::*;
#[path = "resources.rs"]
mod resources;

struct Execution<'a> {
    runtime: &'a dyn Inspector,
    claim: &'a ConfigurationClaim,
    authority: &'a dyn ConfigurationAuthority,
    runtime_id: String,
}
impl Execution<'_> {
    async fn inspect(&self) -> Result<Inspection> {
        super::guard(self.claim, self.authority).await?;
        let inspected = self
            .runtime
            .inspect_configuration(self.claim, Some(&self.runtime_id), self.authority)
            .await?;
        if inspected.runtime_id != self.runtime_id {
            return Err(ExecutionError::Ownership);
        }
        super::guard(self.claim, self.authority).await?;
        Ok(inspected)
    }
    async fn call<T: DeserializeOwned>(
        &self,
        client: &crate::mail::client::Client,
        method: &str,
        arguments: Value,
    ) -> Result<T> {
        super::guard(self.claim, self.authority).await?;
        let result = client.call(method, arguments).await?;
        super::guard(self.claim, self.authority).await?;
        Ok(result)
    }
    async fn mutation<T: DeserializeOwned>(
        &self,
        client: &crate::mail::client::Client,
        method: &str,
        arguments: Value,
    ) -> Result<Option<T>> {
        match self.call(client, method, arguments).await {
            Ok(value) => Ok(Some(value)),
            Err(ExecutionError::Runtime) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn checkpoint(&self, phase: ConfigurationCheckpointPhase) -> Result<()> {
        super::guard(self.claim, self.authority).await?;
        self.authority
            .checkpoint(
                self.claim,
                ConfigurationCheckpoint {
                    lease: self.claim.lease.clone(),
                    runtime_id: self.runtime_id.clone(),
                    phase,
                },
            )
            .await?;
        super::guard(self.claim, self.authority).await
    }
}

pub(in crate::mail) async fn execute(
    runtime: &dyn Inspector,
    claim: &ConfigurationClaim,
    authority: &dyn ConfigurationAuthority,
) -> Result<ConfigurationProof> {
    super::guard(claim, authority).await?;
    let inspected = runtime
        .inspect_configuration(claim, None, authority)
        .await?;
    if claim
        .checkpoints
        .iter()
        .any(|checkpoint| checkpoint.runtime_id != inspected.runtime_id)
    {
        return Err(ExecutionError::Ownership);
    }
    let execution = Execution {
        runtime,
        claim,
        authority,
        runtime_id: inspected.runtime_id,
    };
    if claim.checkpoints.is_empty() {
        execution
            .checkpoint(ConfigurationCheckpointPhase::Bound {})
            .await?;
    }
    let mut proof = ConfigurationProof {
        runtime_id: execution.runtime_id.clone(),
        domains: BTreeMap::new(),
        mailboxes: BTreeMap::new(),
        identities: BTreeMap::new(),
        dkim: BTreeMap::new(),
    };
    let domains: Rows<Domain> = execution
        .call(
            &inspected.client,
            "x:Domain/get",
            json!({"ids":null,"properties":["id","name","description","dkimManagement"]}),
        )
        .await?;
    if domains.list.len() > 256 {
        return Err(ExecutionError::Ownership);
    }
    let internal = domains
        .list
        .iter()
        .filter(|domain| domain.name == claim.spec.target.hostname)
        .collect::<Vec<_>>();
    if internal.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let internal_id = internal[0].id.clone();
    for domain in &claim.spec.snapshot.domains {
        let (id, dkim) = execution.domain(domain).await?;
        proof.domains.insert(domain.id.clone(), id);
        proof.dkim.insert(domain.id.clone(), dkim);
    }
    for mailbox in &claim.spec.snapshot.mailboxes {
        let id = if claim.spec.mode == ConfigurationMode::Restrict {
            execution.retained_mailbox(mailbox).await?
        } else {
            let domain = proof
                .domains
                .get(&mailbox.domain_id)
                .ok_or(ExecutionError::Ownership)?;
            execution.mailbox(mailbox, domain).await?
        };
        proof.mailboxes.insert(mailbox.id.clone(), id);
    }
    for identity in &claim.spec.snapshot.identities {
        let id = execution
            .identity(identity, &internal_id, &proof.mailboxes)
            .await?;
        proof.identities.insert(identity.id.clone(), id);
    }
    execution.inspect().await?;
    proof
        .validate_for(&claim.spec)
        .map_err(|_| ExecutionError::Ownership)?;
    Ok(proof)
}

#[cfg(test)]
#[path = "fault_tests.rs"]
mod fault_tests;
