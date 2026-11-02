//! Enrolled-agent lifecycle authority. The caller supplies Hosting's authenticated transaction;
//! neither browser data nor an unsigned worker scope can issue or acknowledge runtime work.
use super::*;
use crate::{outbox::run, secrets::MailCipher};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use webdock_agent_protocol::mail::*;
use webdock_hosting::AgentIdentity;
mod authorization;
mod configuration;
use authorization::ActorGuard;

pub struct MailAgentConfig {
    pub image: String,
    pub hostname_suffix: String,
    pub encryption_key: String,
}
pub struct IncomingAgent {
    incoming: Arc<NativeIncoming>,
    image: String,
    hostname_suffix: String,
    cipher: MailCipher,
    configuration_enabled: bool,
}
const RECORD:&str="SELECT n.*,o.revision,o.desired_enabled,o.state AS operation_state,o.lease_until,o.lease_token,o.agent_generation AS operation_agent_generation,
 s.revision AS service_revision,s.desired_enabled AS service_enabled,s.instance_key
 FROM webdock_mail.native_operation n JOIN webdock_mail.operation o ON o.id=n.operation_id AND o.customer_id=n.customer_id
 JOIN webdock_mail.service s ON s.customer_id=n.customer_id AND s.host_id=n.cluster_id";
struct CustomerFence {
    customer_active: bool,
    mail_enabled: bool,
    module_available: bool,
}
struct Record {
    row: QueryResult,
    customer: String,
    operation: String,
    revision: i64,
    enabled: bool,
    fence: CustomerFence,
}
impl Record {
    fn from(row: QueryResult, fence: CustomerFence) -> Result<Self> {
        Ok(Self {
            customer: field(&row, "customer_id")?,
            operation: field(&row, "operation_id")?,
            revision: field(&row, "revision")?,
            enabled: field(&row, "desired_enabled")?,
            row,
            fence,
        })
    }
    fn current(&self) -> Result<bool> {
        Ok(
            field::<i64>(&self.row, "service_revision")? == self.revision
                && field::<bool>(&self.row, "service_enabled")? == self.enabled,
        )
    }
    fn live_lease(&self) -> Result<bool> {
        Ok(self.current()?
            && field::<String>(&self.row, "operation_state")? == "running"
            && field::<Option<DateTime<Utc>>>(&self.row, "lease_until")?
                .is_some_and(|until| until > Utc::now()))
    }
}
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Credentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bootstrap_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}
fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn random() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| Error::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
impl IncomingAgent {
    pub fn new(incoming: Arc<NativeIncoming>, config: MailAgentConfig) -> Result<Self> {
        let spec = MailSpec {
            version: 1,
            operation_id: "1".into(),
            customer_id: "1".into(),
            cluster_id: "1".into(),
            revision: 1,
            agent_generation: 1,
            instance_key: "mail-1".into(),
            hostname: format!("mail-1.{}", config.hostname_suffix),
            image: config.image.clone(),
            resources: MailResources::FIXED,
            material_sha256: "0".repeat(64),
            action: MailAction::Ensure,
        };
        spec.validate().map_err(|_| Error::Invalid)?;
        Ok(Self {
            incoming,
            image: config.image,
            hostname_suffix: config.hostname_suffix,
            cipher: MailCipher::new(&config.encryption_key)?,
            configuration_enabled: false,
        })
    }
    async fn scope(&self, tx: &DatabaseTransaction, agent: &AgentIdentity) -> Result<()> {
        crate::recovery::ensure_open(tx).await?;
        let row = one(
            tx,
            "SELECT webdock_rls.worker(ARRAY['agent']::text[],$1) AS allowed",
            vec![agent.cluster_id().into()],
        )
        .await?
        .ok_or(Error::Unavailable)?;
        if !field::<bool>(&row, "allowed")?
            || agent.cluster_id() != self.incoming.config.managed_cluster
        {
            return Err(Error::Unavailable);
        }
        if one(tx,"SELECT 1 FROM webdock_auth.hosting_agent WHERE cluster_id=$1 AND generation=$2 AND NOT revoked FOR SHARE",vec![agent.cluster_id().into(),agent.generation().into()]).await?.is_none(){return Err(Error::Unavailable);}
        Ok(())
    }
    async fn qualified(&self, tx: &DatabaseTransaction, agent: &AgentIdentity) -> Result<bool> {
        self.scope(tx, agent).await?;
        if !self.incoming.config.enabled {
            return Ok(false);
        }
        let row=one(tx,"SELECT 1 FROM webdock_auth.hosting_agent a JOIN webdock_auth.hosting_cluster c ON c.id=a.cluster_id WHERE a.cluster_id=$1 AND a.generation=$2 AND NOT a.revoked AND c.verified AND c.ownership='platform' AND (c.runtime_kind='kubernetes' OR ($4 AND c.runtime_kind='docker')) AND a.last_seen>now()-interval '90 seconds' AND a.last_seen<=now() AND a.observation#>>'{capabilities,nativeMail,version}'='1' AND a.observation#>>'{capabilities,nativeMail,image}'=$3 AND (a.observation#>>'{capabilities,storageVersion}'='1' OR ($4 AND c.runtime_kind='docker')) FOR SHARE OF a,c",vec![agent.cluster_id().into(),agent.generation().into(),self.image.clone().into(),self.incoming.local_docker.into()]).await?;
        if row.is_none() {
            return Ok(false);
        }
        Ok(true)
    }
    async fn customer_fence(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        customer: &str,
    ) -> Result<CustomerFence> {
        let module_available =
            authority::agent_module(tx, &self.incoming.config.rls_secret).await?;
        let row=one(tx,"SELECT c.status='active' AS active FROM webdock_admin.customers c WHERE c.id=$1 AND EXISTS(SELECT 1 FROM webdock_mail.service s JOIN webdock_mail.cluster_reservation r ON r.customer_id=s.customer_id AND r.cluster_id::text=$2 WHERE s.customer_id=c.id AND s.host_id=$2) FOR SHARE OF c",vec![customer.into(),agent.cluster_id().into()]).await?.ok_or(Error::Unavailable)?;
        let settings=one(tx,"SELECT settings->>'mailEnabled'='true' AS enabled FROM webdock_auth.platform_settings WHERE id=true FOR SHARE",vec![]).await?;
        let mail_enabled = settings
            .as_ref()
            .map(|row| field::<Option<bool>>(row, "enabled"))
            .transpose()?
            .flatten()
            .unwrap_or(false);
        Ok(CustomerFence {
            customer_active: field(&row, "active")?,
            mail_enabled,
            module_available,
        })
    }
    async fn load(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        id: &str,
    ) -> Result<Record> {
        let target=one(tx,"SELECT customer_id FROM webdock_mail.native_operation WHERE operation_id=$1 AND cluster_id=$2",vec![id.into(),agent.cluster_id().into()]).await?.ok_or(Error::Unavailable)?;
        let fence = self
            .customer_fence(tx, agent, &field::<String>(&target, "customer_id")?)
            .await?;
        let query =
            format!("{RECORD} WHERE n.operation_id=$1 AND n.cluster_id=$2 FOR UPDATE OF s,o,n");
        Record::from(
            one(tx, &query, vec![id.into(), agent.cluster_id().into()])
                .await?
                .ok_or(Error::Unavailable)?,
            fence,
        )
    }
    async fn expire(&self, tx: &DatabaseTransaction, agent: &AgentIdentity) -> Result<bool> {
        let candidate=one(tx,"SELECT o.id,s.customer_id FROM webdock_mail.service s JOIN webdock_mail.operation o ON o.customer_id=s.customer_id JOIN webdock_mail.cluster_reservation r ON r.customer_id=s.customer_id AND r.cluster_id::text=s.host_id WHERE s.host_id=$1 AND o.state='running' AND o.lease_until<now() ORDER BY o.created_at,o.id LIMIT 1",vec![agent.cluster_id().into()]).await?;
        let Some(candidate) = candidate else {
            return Ok(false);
        };
        let customer: String = field(&candidate, "customer_id")?;
        self.customer_fence(tx, agent, &customer).await?;
        one(tx,"SELECT customer_id FROM webdock_mail.service WHERE customer_id=$1 AND host_id=$2 FOR UPDATE",vec![customer.clone().into(),agent.cluster_id().into()]).await?.ok_or(Error::Unavailable)?;
        let expired=one(tx,"UPDATE webdock_mail.operation SET state='needs_review',finished_at=now() WHERE id=$1 AND customer_id=$2 AND state='running' AND lease_until<now() RETURNING revision",vec![field::<String>(&candidate,"id")?.into(),customer.clone().into()]).await?;
        if let Some(expired) = expired {
            run(tx,"UPDATE webdock_mail.service SET state='needs_review',updated_at=now() WHERE customer_id=$1 AND revision=$2",vec![customer.into(),field::<i64>(&expired,"revision")?.into()]).await?;
        }
        Ok(true)
    }
    pub async fn claim(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
    ) -> Result<Option<MailClaim>> {
        self.scope(&tx, agent).await?;
        if self.expire(&tx, agent).await? {
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(None);
        }
        let candidate=one(&tx,"SELECT o.id FROM webdock_mail.service s JOIN webdock_mail.operation o ON o.customer_id=s.customer_id AND o.revision=s.revision JOIN webdock_mail.native_operation n ON n.operation_id=o.id WHERE s.host_id=$1 AND o.state IN ('pending','running') AND s.state<>'needs_review' AND (o.state='pending' OR (n.agent_generation=$2 AND o.lease_until>now())) AND NOT EXISTS(SELECT 1 FROM webdock_mail.operation p WHERE p.customer_id=s.customer_id AND p.id<>o.id AND p.state IN ('running','needs_review')) ORDER BY o.created_at,o.id LIMIT 1",vec![agent.cluster_id().into(),agent.generation().into()]).await?;
        let Some(candidate) = candidate else {
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(None);
        };
        let record = self
            .load(&tx, agent, &field::<String>(&candidate, "id")?)
            .await?;
        if !self.qualified(&tx, agent).await? {
            return Err(Error::Unavailable);
        }
        let Some(access) = self.actor(&record).await? else {
            self.review(&tx, &record).await?;
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(None);
        };
        if !record.current()? {
            return Err(Error::Conflict);
        }
        let claim = if field::<String>(&record.row, "operation_state")? == "running" {
            self.recorded_claim(&record)?
        } else {
            self.issue(&tx, agent, &record, access.expires_at).await?
        };
        claim.validate(Utc::now()).map_err(|_| Error::Conflict)?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        access.commit().await?;
        Ok(Some(claim))
    }
    fn recorded_claim(&self, record: &Record) -> Result<MailClaim> {
        let spec: MailSpec = decode(&record.row, "spec")?;
        let lease = MailLease {
            lease_token: field::<Option<String>>(&record.row, "lease_token")?
                .ok_or(Error::Conflict)?,
        };
        let request_hash = spec.fingerprint().map_err(|_| Error::Conflict)?;
        if field::<Option<String>>(&record.row, "request_sha256")?.as_deref() != Some(&request_hash)
            || field::<Option<String>>(&record.row, "lease_token_sha256")?.as_deref()
                != Some(&digest(&lease.lease_token))
            || field::<Option<i32>>(&record.row, "agent_generation")? != Some(spec.agent_generation)
            || field::<Option<i32>>(&record.row, "operation_agent_generation")?
                != Some(spec.agent_generation)
            || spec.operation_id != record.operation
            || spec.customer_id != record.customer
            || spec.cluster_id != field::<String>(&record.row, "cluster_id")?
            || spec.revision != record.revision
            || (spec.action == MailAction::Ensure) != record.enabled
            || spec.image != self.image
            || spec.hostname != format!("mail-{}.{}", record.customer, self.hostname_suffix)
        {
            return Err(Error::Conflict);
        }
        let encrypted: String = field(&record.row, "encrypted_material")?;
        let material = self
            .cipher
            .open(&encrypted, &format!("operation:{}", record.operation))?;
        let checkpoint = if field::<Option<bool>>(&record.row, "checkpoint_live")? == Some(true) {
            Some(MailCheckpoint {
                lease: lease.clone(),
                runtime_id: field::<Option<String>>(&record.row, "checkpoint_runtime_id")?
                    .ok_or(Error::Conflict)?,
                material_sha256: spec.material_sha256.clone(),
            })
        } else {
            None
        };
        Ok(MailClaim {
            spec,
            lease,
            lease_until: field::<Option<DateTime<Utc>>>(&record.row, "lease_until")?
                .ok_or(Error::Conflict)?,
            material,
            checkpoint,
        })
    }
    async fn material(&self, tx: &DatabaseTransaction, record: &Record) -> Result<MailMaterial> {
        let row=one(tx,"SELECT encrypted_credentials,verified_at FROM webdock_mail.instance WHERE customer_id=$1 FOR UPDATE",vec![record.customer.clone().into()]).await?;
        if row.is_none() && !record.enabled {
            return Ok(MailMaterial::Unallocated {});
        }
        let mut credentials = match &row {
            Some(row) => self.cipher.open::<Credentials>(
                &field::<String>(row, "encrypted_credentials")?,
                &format!("instance:{}", record.customer),
            )?,
            None => Credentials::default(),
        };
        if credentials.username.is_some() != credentials.password.is_some() {
            return Err(Error::Conflict);
        }
        if credentials.username.is_none() {
            if row.as_ref().is_some_and(|row| {
                field::<Option<DateTime<Utc>>>(row, "verified_at").is_ok_and(|v| v.is_some())
            }) {
                return Err(Error::Conflict);
            }
            credentials.username = Some(format!(
                "webdock-admin@mail-{}.{}",
                record.customer, self.hostname_suffix
            ));
            credentials.password = Some(random()?);
            if credentials.bootstrap_password.is_none() {
                credentials.bootstrap_password = Some(random()?);
            }
            let encrypted = self
                .cipher
                .seal(&credentials, &format!("instance:{}", record.customer))?;
            run(tx,"INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials) VALUES($1,$2) ON CONFLICT(customer_id) DO UPDATE SET encrypted_credentials=excluded.encrypted_credentials,updated_at=now()",vec![record.customer.clone().into(),encrypted.into()]).await?;
        }
        let material = MailMaterial::Credentials {
            bootstrap_password: credentials.bootstrap_password,
            username: credentials.username.ok_or(Error::Conflict)?,
            password: credentials.password.ok_or(Error::Conflict)?,
        };
        material.validate().map_err(|_| Error::Conflict)?;
        Ok(material)
    }
    async fn issue(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        record: &Record,
        authority_until: DateTime<Utc>,
    ) -> Result<MailClaim> {
        if field::<String>(&record.row, "operation_state")? != "pending"
            || field::<Option<String>>(&record.row, "request_sha256")?.is_some()
        {
            return Err(Error::Conflict);
        }
        let material = self.material(tx, record).await?;
        let spec = MailSpec {
            version: 1,
            operation_id: record.operation.clone(),
            customer_id: record.customer.clone(),
            cluster_id: agent.cluster_id().into(),
            revision: record.revision,
            agent_generation: agent.generation(),
            instance_key: field(&record.row, "instance_key")?,
            hostname: format!("mail-{}.{}", record.customer, self.hostname_suffix),
            image: self.image.clone(),
            resources: MailResources::FIXED,
            material_sha256: material.fingerprint().map_err(|_| Error::Invalid)?,
            action: if record.enabled {
                MailAction::Ensure
            } else {
                MailAction::Suspend
            },
        };
        let lease = MailLease {
            lease_token: uuid::Uuid::new_v4().to_string(),
        };
        let until=one(tx,"UPDATE webdock_mail.operation SET state='running',lease_token=$2,lease_until=least(clock_timestamp()+interval '120 seconds',$4::timestamptz),agent_generation=$3 WHERE id=$1 AND state='pending' AND $4::timestamptz>clock_timestamp() RETURNING lease_until",vec![record.operation.clone().into(),lease.lease_token.clone().into(),agent.generation().into(),authority_until.into()]).await?.ok_or(Error::Conflict)?;
        run(tx,"UPDATE webdock_mail.native_operation SET request_sha256=$2,spec=$3,encrypted_material=$4,material_sha256=$5,lease_token_sha256=$6,agent_generation=$7 WHERE operation_id=$1 AND request_sha256 IS NULL",vec![record.operation.clone().into(),spec.fingerprint().map_err(|_|Error::Invalid)?.into(),serde_json::to_value(&spec).map_err(|_|Error::Invalid)?.into(),self.cipher.seal(&material,&format!("operation:{}",record.operation))?.into(),spec.material_sha256.clone().into(),digest(&lease.lease_token).into(),agent.generation().into()]).await?;
        run(tx,"UPDATE webdock_mail.service SET state='provisioning',updated_at=now() WHERE customer_id=$1 AND revision=$2",vec![record.customer.clone().into(),record.revision.into()]).await?;
        Ok(MailClaim {
            spec,
            lease,
            lease_until: field(&until, "lease_until")?,
            material,
            checkpoint: None,
        })
    }
    async fn issued(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &MailSpec,
        lease: &MailLease,
    ) -> Result<Record> {
        self.scope(tx, agent).await?;
        spec.validate().map_err(|_| Error::Invalid)?;
        lease.validate().map_err(|_| Error::Invalid)?;
        let record = self.load(tx, agent, &spec.operation_id).await?;
        if spec.agent_generation != agent.generation()
            || spec.cluster_id != agent.cluster_id()
            || spec.customer_id != record.customer
            || spec.revision != record.revision
            || field::<Option<i32>>(&record.row, "agent_generation")? != Some(agent.generation())
            || field::<Option<String>>(&record.row, "request_sha256")?.as_deref()
                != Some(&spec.fingerprint().map_err(|_| Error::Invalid)?)
            || field::<Option<String>>(&record.row, "lease_token_sha256")?.as_deref()
                != Some(&digest(&lease.lease_token))
        {
            return Err(Error::Conflict);
        }
        Ok(record)
    }
    async fn live(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        record: &Record,
        lease: &MailLease,
    ) -> Result<Option<ActorGuard>> {
        if !self.qualified(tx, agent).await? {
            return Ok(None);
        }
        if !record.live_lease()?
            || field::<Option<String>>(&record.row, "lease_token")?.as_deref()
                != Some(&lease.lease_token)
            || field::<Option<i32>>(&record.row, "operation_agent_generation")?
                != Some(agent.generation())
        {
            return Ok(None);
        }
        self.recorded_claim(record)?;
        let current=one(tx,"SELECT encrypted_credentials FROM webdock_mail.instance WHERE customer_id=$1 FOR SHARE",vec![record.customer.clone().into()]).await?;
        let material = match current {
            Some(row) => {
                let credentials: Credentials = self.cipher.open(
                    &field::<String>(&row, "encrypted_credentials")?,
                    &format!("instance:{}", record.customer),
                )?;
                MailMaterial::Credentials {
                    bootstrap_password: credentials.bootstrap_password,
                    username: credentials.username.ok_or(Error::Conflict)?,
                    password: credentials.password.ok_or(Error::Conflict)?,
                }
            }
            None => MailMaterial::Unallocated {},
        };
        if field::<Option<String>>(&record.row, "material_sha256")?.as_deref()
            != Some(&material.fingerprint().map_err(|_| Error::Conflict)?)
        {
            return Ok(None);
        }
        let Some(access) = self.actor(record).await? else {
            return Ok(None);
        };
        // Recheck the database clock after every potentially blocking authority/material read.
        let current=one(tx,"SELECT state='running' AND lease_until>clock_timestamp() AND $2::timestamptz>clock_timestamp() AS current FROM webdock_mail.operation WHERE id=$1",vec![record.operation.clone().into(),access.expires_at.into()]).await?.ok_or(Error::Unavailable)?;
        if !field::<bool>(&current, "current")? {
            return Ok(None);
        }
        Ok(Some(access))
    }
    pub async fn guard(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &MailSpec,
        lease: &MailLease,
    ) -> Result<()> {
        let record = self.issued(&tx, agent, spec, lease).await?;
        let access = self
            .live(&tx, agent, &record, lease)
            .await?
            .ok_or(Error::Conflict)?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        access.commit().await
    }
    async fn review(&self, tx: &DatabaseTransaction, record: &Record) -> Result<()> {
        run(tx,"UPDATE webdock_mail.operation SET state='needs_review',finished_at=coalesce(finished_at,now()) WHERE id=$1 AND state IN ('pending','running')",vec![record.operation.clone().into()]).await?;
        run(tx,"UPDATE webdock_mail.service SET state='needs_review',updated_at=now() WHERE customer_id=$1 AND revision=$2 AND state NOT IN ('ready','suspended')",vec![record.customer.clone().into(),record.revision.into()]).await?;
        Ok(())
    }
    pub async fn checkpoint(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &MailSpec,
        data: MailCheckpoint,
    ) -> Result<()> {
        data.validate().map_err(|_| Error::Invalid)?;
        let record = self.issued(&tx, agent, spec, &data.lease).await?;
        if spec.action != MailAction::Ensure || data.material_sha256 != spec.material_sha256 {
            return Err(Error::Conflict);
        }
        let hash = fingerprint(&data).map_err(|_| Error::Invalid)?;
        if let Some(previous) = field::<Option<String>>(&record.row, "checkpoint_sha256")? {
            if previous != hash {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(());
        }
        let access = self.live(&tx, agent, &record, &data.lease).await?;
        // Custody and permission are recorded together at the database clock boundary.
        let until = access.as_ref().map(|actor| actor.expires_at);
        let recorded=one(&tx,"UPDATE webdock_mail.native_operation n SET checkpoint_sha256=$2,checkpoint_runtime_id=$3,checkpoint_live=coalesce($4::timestamptz>clock_timestamp() AND EXISTS(SELECT 1 FROM webdock_mail.operation o WHERE o.id=n.operation_id AND o.state='running' AND o.lease_until>clock_timestamp() AND o.lease_token=$5 AND o.agent_generation=$6),false) WHERE n.operation_id=$1 RETURNING checkpoint_live",vec![record.operation.clone().into(),hash.into(),data.runtime_id.into(),until.into(),data.lease.lease_token.into(),agent.generation().into()]).await?.ok_or(Error::Unavailable)?;
        if !field::<bool>(&recorded, "checkpoint_live")? {
            self.review(&tx, &record).await?;
        }
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        if let Some(access) = access {
            access.commit().await?;
        }
        Ok(())
    }
    pub async fn complete(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &MailSpec,
        data: MailCompletion,
    ) -> Result<()> {
        data.validate().map_err(|_| Error::Invalid)?;
        let record = self.issued(&tx, agent, spec, &data.lease).await?;
        if let Some(proof) = &data.proof
            && (proof.namespace != format!("wd-mail-{}", record.customer)
                || proof.revision != record.revision
                || proof.running != record.enabled
                || (proof.private_ipv4.is_some() && !self.incoming.local_docker))
        {
            return Err(Error::Conflict);
        }
        let hash = fingerprint(&data).map_err(|_| Error::Invalid)?;
        if let Some(previous) = field::<Option<String>>(&record.row, "receipt_sha256")? {
            if previous != hash {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(());
        }
        let access = self.live(&tx, agent, &record, &data.lease).await?;
        let success = match (access.as_ref(), data.outcome) {
            (Some(actor), MailOutcome::Succeeded) => {
                if record.enabled
                    && field::<Option<bool>>(&record.row, "checkpoint_live")? != Some(true)
                {
                    return Err(Error::Conflict);
                }
                self.succeed(
                    &tx,
                    &record,
                    data.proof.as_ref().ok_or(Error::Invalid)?,
                    actor.expires_at,
                )
                .await?
            }
            (None, MailOutcome::Succeeded) | (_, MailOutcome::NeedsReview) => false,
        };
        if !success {
            self.review(&tx, &record).await?;
        }
        if !run(&tx,"UPDATE webdock_mail.native_operation n SET receipt_sha256=$2,receipt_live=$3 WHERE operation_id=$1 AND (NOT $3 OR EXISTS(SELECT 1 FROM webdock_mail.operation o WHERE o.id=n.operation_id AND o.lease_until>clock_timestamp() AND $4::timestamptz>clock_timestamp()))",vec![record.operation.clone().into(),hash.into(),success.into(),access.as_ref().map(|a|a.expires_at).into()]).await? {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        if let Some(access) = access {
            access.commit().await?;
        }
        Ok(())
    }
    async fn succeed(
        &self,
        tx: &DatabaseTransaction,
        record: &Record,
        proof: &MailProof,
        authority_until: DateTime<Utc>,
    ) -> Result<bool> {
        // Projections remain provisional until the final receipt write checks the database clock
        // again; a capacity/cache lock wait past either deadline rolls the entire transaction back.
        let admitted=one(tx,"UPDATE webdock_mail.operation SET state='succeeded',finished_at=clock_timestamp() WHERE id=$1 AND state='running' AND lease_until>clock_timestamp() AND $2::timestamptz>clock_timestamp() RETURNING id",vec![record.operation.clone().into(),authority_until.into()]).await?;
        if admitted.is_none() {
            return Ok(false);
        }
        if record.enabled {
            let claim = self.recorded_claim(record)?;
            let MailMaterial::Credentials {
                username, password, ..
            } = claim.material
            else {
                return Err(Error::Conflict);
            };
            let credentials = Credentials {
                username: Some(username),
                password: Some(password),
                bootstrap_password: None,
            };
            if !run(tx,"UPDATE webdock_mail.instance SET encrypted_credentials=$2,internal_url=$3,public_url=$4,verified_at=now(),smtp_certificate_sha256=$5,smtp_certificate_revision=$6,updated_at=now() WHERE customer_id=$1",vec![record.customer.clone().into(),self.cipher.seal(&credentials,&format!("instance:{}",record.customer))?.into(),proof.private_ipv4.map_or_else(||format!("http://app-{}.wd-mail-{}.svc.cluster.local:8080",record.customer,record.customer),|ip|format!("http://{ip}:8080")).into(),format!("https://mail-{}.{}",record.customer,self.hostname_suffix).into(),proof.submission_certificate_sha256.clone().into(),proof.submission_certificate_sha256.as_ref().map(|_|record.revision).into()]).await? {return Err(Error::Unavailable);}
        }
        if !run(tx,"UPDATE webdock_mail.service SET state=$3,applied_revision=$2,updated_at=now() WHERE customer_id=$1 AND revision=$2",vec![record.customer.clone().into(),record.revision.into(),if record.enabled {"ready"}else{"suspended"}.into()]).await? {return Err(Error::Unavailable);}
        capacity::complete(tx, &record.customer, record.enabled).await?;
        Ok(true)
    }
}
