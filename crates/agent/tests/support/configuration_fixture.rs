use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use webdock_agent_protocol::mail::{MailClaim, MailLease, MailMaterial, MailProof};
use webdock_supervisor::{ExecutionError, Result};
pub(super) struct ConfigurationAuthority {
    pub(super) allowed: AtomicBool,
    pub(super) drop_applied: AtomicBool,
    pub(super) checkpoints:
        Mutex<Vec<webdock_agent_protocol::mail_configuration::ConfigurationCheckpoint>>,
}
#[async_trait::async_trait]
impl webdock_agent::mail::configuration::ConfigurationAuthority for ConfigurationAuthority {
    async fn guard(
        &self,
        claim: &webdock_agent_protocol::mail_configuration::ConfigurationClaim,
    ) -> Result<()> {
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        if !self.allowed.load(Ordering::SeqCst) {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(
        &self,
        claim: &webdock_agent_protocol::mail_configuration::ConfigurationClaim,
        checkpoint: webdock_agent_protocol::mail_configuration::ConfigurationCheckpoint,
    ) -> Result<()> {
        self.guard(claim).await?;
        let mut stored = self.checkpoints.lock().await;
        let bytes = serde_json::to_vec(&checkpoint).unwrap();
        if !stored
            .iter()
            .any(|old| serde_json::to_vec(old).unwrap() == bytes)
        {
            stored.push(checkpoint.clone());
        }
        if matches!(
            checkpoint.phase,
            webdock_agent_protocol::mail_configuration::ConfigurationCheckpointPhase::Applied { .. }
        ) && self.drop_applied.swap(false, Ordering::SeqCst)
        {
            return Err(ExecutionError::Journal);
        }
        Ok(())
    }
}
pub(super) fn configuration_claim(
    lifecycle: &MailClaim,
    proof: &MailProof,
) -> webdock_agent_protocol::mail_configuration::ConfigurationClaim {
    use webdock_agent_protocol::mail_configuration::*;
    let MailMaterial::Credentials {
        username, password, ..
    } = &lifecycle.material
    else {
        panic!("fixture material")
    };
    let material = ConfigurationMaterial {
        administrator: ConfigurationCredential {
            username: username.clone(),
            password: password.clone(),
        },
        identities: std::collections::BTreeMap::from([(
            "7004".into(),
            ConfigurationCredential {
                username: format!("wd-7004@{}", lifecycle.spec.hostname),
                password: uuid::Uuid::new_v4().to_string(),
            },
        )]),
    };
    ConfigurationClaim {
        spec: ConfigurationSpec {
            version: 1,
            operation_id: "7005".into(),
            customer_id: lifecycle.spec.customer_id.clone(),
            cluster_id: lifecycle.spec.cluster_id.clone(),
            revision: "1".into(),
            service_revision: lifecycle.spec.revision.to_string(),
            agent_generation: lifecycle.spec.agent_generation,
            mode: ConfigurationMode::Apply,
            target: ConfigurationTarget {
                instance_key: lifecycle.spec.instance_key.clone(),
                hostname: lifecycle.spec.hostname.clone(),
                image: lifecycle.spec.image.clone(),
                resources: lifecycle.spec.resources,
                management_sha256: material.management_fingerprint().unwrap(),
                certificate_sha256: proof.submission_certificate_sha256.clone().unwrap(),
            },
            material_sha256: material.fingerprint().unwrap(),
            snapshot: ConfigurationSnapshot {
                domains: vec![ConfigurationDomain {
                    id: "7001".into(),
                    name: "executor.example.test".into(),
                    provider_id: None,
                    ownership_verified: true,
                }],
                mailboxes: vec![
                    ConfigurationMailbox {
                        id: "7002".into(),
                        domain_id: "7001".into(),
                        local_part: "first".into(),
                        quota_bytes: 4194304,
                        provider_id: None,
                    },
                    ConfigurationMailbox {
                        id: "7003".into(),
                        domain_id: "7001".into(),
                        local_part: "second".into(),
                        quota_bytes: 4194304,
                        provider_id: None,
                    },
                ],
                identities: vec![ConfigurationIdentity {
                    id: "7004".into(),
                    subject: "42".into(),
                    provider_id: None,
                    enabled: true,
                    mailbox_ids: vec!["7002".into(), "7003".into()],
                }],
                grants: vec![
                    ConfigurationGrant {
                        mailbox_id: "7002".into(),
                        subject: "42".into(),
                        revision: "1".into(),
                        enabled: true,
                    },
                    ConfigurationGrant {
                        mailbox_id: "7003".into(),
                        subject: "42".into(),
                        revision: "1".into(),
                        enabled: true,
                    },
                ],
            },
        },
        lease: MailLease {
            lease_token: uuid::Uuid::new_v4().to_string(),
        },
        lease_until: chrono::Utc::now() + chrono::Duration::seconds(120),
        material,
        checkpoints: Vec::new(),
    }
}
