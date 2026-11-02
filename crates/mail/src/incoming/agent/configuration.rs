//! Native configuration execution never adopts historical jobs or expands restriction authority.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use webdock_agent_protocol::mail_configuration::*;
mod authority;
mod callbacks;
mod snapshot;
use authority::Access;
const CONFIGURATION:&str="SELECT n.*,o.revision,o.state AS operation_state,o.lease_token,o.lease_until,o.agent_generation AS operation_agent_generation,o.reason,
 s.revision AS service_revision,s.applied_revision AS service_applied_revision,s.desired_enabled,s.state AS service_state,s.instance_key,c.revision AS configuration_revision
 FROM webdock_mail.native_configuration_operation n JOIN webdock_mail.configuration_operation o ON o.id=n.operation_id AND o.customer_id=n.customer_id
 JOIN webdock_mail.service s ON s.customer_id=n.customer_id AND s.host_id=n.cluster_id JOIN webdock_mail.configuration c ON c.customer_id=n.customer_id";
struct ConfigurationRecord {
    row: QueryResult,
    customer: String,
    operation: String,
    revision: i64,
    mode: ConfigurationMode,
    fence: CustomerFence,
}
impl ConfigurationRecord {
    fn from(row: QueryResult, fence: CustomerFence) -> Result<Self> {
        Ok(Self {
            customer: field(&row, "customer_id")?,
            operation: field(&row, "operation_id")?,
            revision: field(&row, "revision")?,
            mode: match field::<String>(&row, "mode")?.as_str() {
                "apply" => ConfigurationMode::Apply,
                "restrict" => ConfigurationMode::Restrict,
                _ => return Err(Error::Unavailable),
            },
            row,
            fence,
        })
    }
    fn current(&self) -> Result<bool> {
        Ok(field::<bool>(&self.row, "desired_enabled")?
            && field::<String>(&self.row, "service_state")? == "ready"
            && field::<i64>(&self.row, "service_revision")?
                == field::<i64>(&self.row, "service_applied_revision")?
            && match self.mode {
                ConfigurationMode::Apply => {
                    self.revision == field::<i64>(&self.row, "configuration_revision")?
                }
                ConfigurationMode::Restrict => {
                    self.revision <= field::<i64>(&self.row, "configuration_revision")?
                }
            })
    }
}
impl IncomingAgent {
    pub fn with_configuration_enabled(mut self, enabled: bool) -> Self {
        self.configuration_enabled = enabled;
        self
    }
    async fn configuration_load(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        id: &str,
    ) -> Result<ConfigurationRecord> {
        self.scope(tx, agent).await?;
        let candidate=one(tx,"SELECT customer_id,subject,admission_member_id FROM webdock_mail.native_configuration_operation WHERE operation_id=$1 AND cluster_id=$2",vec![id.into(),agent.cluster_id().into()]).await?.ok_or(Error::Conflict)?;
        let customer: String = field(&candidate, "customer_id")?;
        let fence = self.customer_fence(tx, agent, &customer).await?;
        // Membership removal holds its member row before the detach trigger changes
        // configuration. Keep that same order on this worker transaction.
        if let Some(member) = field::<Option<String>>(&candidate, "admission_member_id")? {
            one(tx,"SELECT m.id FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE m.id=$1 AND m.\"userId\"=$2 AND t.customer_id=$3 FOR SHARE OF m",vec![member.into(),field::<String>(&candidate,"subject")?.into(),customer.clone().into()]).await?;
        }
        one(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("mail-configuration:{customer}").into()],
        )
        .await?;
        let row=one(tx,&format!("{CONFIGURATION} WHERE n.operation_id=$1 AND n.cluster_id=$2 FOR UPDATE OF o,n FOR SHARE OF s,c"),vec![id.into(),agent.cluster_id().into()]).await?.ok_or(Error::Conflict)?;
        ConfigurationRecord::from(row, fence)
    }
    async fn configuration_ready(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
    ) -> Result<bool> {
        if !self.configuration_enabled || !self.incoming.config.mailboxes_enabled {
            return Ok(false);
        }
        if !self.qualified(tx, agent).await? {
            return Ok(false);
        }
        Ok(one(tx,"SELECT 1 FROM webdock_auth.hosting_agent WHERE cluster_id=$1 AND generation=$2 AND observation#>>'{capabilities,nativeMail,configurationVersion}'='1'",vec![agent.cluster_id().into(),agent.generation().into()]).await?.is_some())
    }
    async fn configuration_review(
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
    ) -> Result<()> {
        run(tx,"UPDATE webdock_mail.configuration_operation SET state='needs_review',finished_at=clock_timestamp() WHERE id=$1 AND state IN ('pending','running')",vec![record.operation.clone().into()]).await?;
        Ok(())
    }
    async fn retire_unissued_restriction(
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
    ) -> Result<bool> {
        if record.mode != ConfigurationMode::Restrict
            || field::<String>(&record.row, "operation_state")? != "pending"
            || field::<Option<String>>(&record.row, "request_sha256")?.is_some()
        {
            return Ok(false);
        }
        let system = field::<String>(&record.row, "provenance_kind")? == "restriction";
        if one(tx,"SELECT 1 FROM webdock_mail.mailbox_grant WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3 AND revision=$4 AND NOT enabled AND (NOT $5 OR (member_id IS NULL AND clerk_grant IS NULL))",vec![record.customer.clone().into(),field::<String>(&record.row,"restricted_mailbox_id")?.into(),field::<String>(&record.row,"restricted_subject")?.into(),field::<i64>(&record.row,"restricted_revision")?.into(),system.into()]).await?.is_some(){return Ok(false);}
        if !run(tx,"UPDATE webdock_mail.configuration_operation SET state='superseded',finished_at=clock_timestamp() WHERE id=$1 AND state='pending'",vec![record.operation.clone().into()]).await?{return Err(Error::Conflict);}
        Ok(true)
    }
    async fn materialize_restriction(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
    ) -> Result<()> {
        let candidate=one(tx,"SELECT e.customer_id,e.configuration_revision FROM webdock_mail.native_configuration_restriction e JOIN webdock_mail.service s ON s.customer_id=e.customer_id JOIN webdock_mail.configuration c ON c.customer_id=e.customer_id AND c.revision>=e.configuration_revision WHERE s.host_id=$1 AND e.operation_id IS NULL AND NOT e.obsolete AND s.desired_enabled AND s.state='ready' AND s.revision=s.applied_revision ORDER BY e.created_at,e.customer_id LIMIT 1",vec![agent.cluster_id().into()]).await?;
        let Some(candidate) = candidate else {
            return Ok(());
        };
        let operation = self.incoming.id().await?;
        let customer: String = field(&candidate, "customer_id")?;
        self.customer_fence(tx, agent, &customer).await?;
        one(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("mail-configuration:{customer}").into()],
        )
        .await?;
        if !self.configuration_ready(tx, agent).await? {
            return Ok(());
        }
        one(
            tx,
            "SELECT webdock_mail.admit_configuration_restriction($1,$2,$3)",
            vec![
                customer.into(),
                field::<i64>(&candidate, "configuration_revision")?.into(),
                operation.into(),
            ],
        )
        .await?;
        Ok(())
    }
    pub async fn claim_configuration(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
    ) -> Result<Option<ConfigurationClaim>> {
        self.scope(&tx, agent).await?;
        if !self.configuration_enabled || !self.incoming.config.mailboxes_enabled {
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(None);
        }
        self.materialize_restriction(&tx, agent).await?;
        let candidate=one(&tx,"SELECT n.operation_id FROM webdock_mail.native_configuration_operation n JOIN webdock_mail.configuration_operation o ON o.id=n.operation_id JOIN webdock_mail.service s ON s.customer_id=n.customer_id JOIN webdock_mail.configuration c ON c.customer_id=n.customer_id WHERE n.cluster_id=$1 AND ((o.state='running' AND (o.lease_until<=clock_timestamp() OR n.agent_generation=$2)) OR (o.state='pending' AND (o.revision=c.revision OR (n.mode='restrict' AND o.revision<=c.revision)) AND s.desired_enabled AND s.state='ready' AND s.revision=s.applied_revision AND NOT EXISTS(SELECT 1 FROM webdock_mail.configuration_operation b WHERE b.customer_id=n.customer_id AND b.id<>o.id AND b.state IN ('running','needs_review')))) ORDER BY (o.state='running') DESC,(n.mode='restrict') DESC,o.created_at,o.id LIMIT 1",vec![agent.cluster_id().into(),agent.generation().into()]).await?;
        let Some(candidate) = candidate else {
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(None);
        };
        let record = self
            .configuration_load(&tx, agent, &field::<String>(&candidate, "operation_id")?)
            .await?;
        if Self::retire_unissued_restriction(&tx, &record).await? {
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(None);
        }
        if !self.configuration_ready(&tx, agent).await? {
            return Err(Error::Unavailable);
        }
        let access = self.configuration_authority(&tx, &record).await?;
        let expired = field::<String>(&record.row, "operation_state")? == "running"
            && field::<Option<DateTime<Utc>>>(&record.row, "lease_until")?
                .is_none_or(|until| until <= Utc::now());
        if !record.current()? || access.is_none() || expired {
            Self::configuration_review(&tx, &record).await?;
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(None);
        }
        let access = access.ok_or(Error::Unavailable)?;
        let claim = if field::<String>(&record.row, "operation_state")? == "running" {
            self.configuration_recorded(&tx, &record).await?
        } else {
            self.configuration_issue(&tx, agent, &record, access.expires_at)
                .await?
        };
        if !self
            .configuration_sources_current(&tx, &record, &claim.spec)
            .await?
        {
            return Err(Error::Conflict);
        }
        claim.validate(Utc::now()).map_err(|_| Error::Conflict)?;
        let current=one(&tx,"SELECT lease_until>clock_timestamp() AND $2::timestamptz>clock_timestamp() AS current FROM webdock_mail.configuration_operation WHERE id=$1",vec![record.operation.clone().into(),access.expires_at.into()]).await?.ok_or(Error::Conflict)?;
        if !field::<bool>(&current, "current")? {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        access.commit().await?;
        Ok(Some(claim))
    }
    async fn configuration_recorded(
        &self,
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
    ) -> Result<ConfigurationClaim> {
        let spec: ConfigurationSpec = decode(&record.row, "spec")?;
        let lease = MailLease {
            lease_token: field::<Option<String>>(&record.row, "lease_token")?
                .ok_or(Error::Conflict)?,
        };
        if field::<Option<String>>(&record.row, "request_sha256")?.as_deref()
            != Some(&spec.fingerprint().map_err(|_| Error::Conflict)?)
            || field::<Option<String>>(&record.row, "lease_token_sha256")?.as_deref()
                != Some(&digest(&lease.lease_token))
            || field::<Option<i32>>(&record.row, "agent_generation")? != Some(spec.agent_generation)
            || field::<Option<i32>>(&record.row, "operation_agent_generation")?
                != Some(spec.agent_generation)
            || spec.operation_id != record.operation
            || spec.customer_id != record.customer
            || spec.revision != record.revision.to_string()
            || spec.mode != record.mode
            || spec.cluster_id != field::<String>(&record.row, "cluster_id")?
            || spec.target.image != self.image
            || spec.target.hostname != format!("mail-{}.{}", record.customer, self.hostname_suffix)
        {
            return Err(Error::Conflict);
        }
        let material = self.cipher.open(
            &field::<String>(&record.row, "encrypted_material")?,
            &format!("configuration:{}", record.operation),
        )?;
        let checkpoints=super::super::rows(tx,"SELECT checkpoint FROM webdock_mail.native_configuration_checkpoint WHERE operation_id=$1 AND live ORDER BY ordinal",vec![record.operation.clone().into()]).await?.iter().map(|r|decode(r,"checkpoint")).collect::<Result<Vec<_>>>()?;
        let claim = ConfigurationClaim {
            spec,
            lease,
            lease_until: field::<Option<DateTime<Utc>>>(&record.row, "lease_until")?
                .ok_or(Error::Conflict)?,
            material,
            checkpoints,
        };
        claim.validate_history().map_err(|_| Error::Unavailable)?;
        Ok(claim)
    }
    async fn configuration_issued(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &ConfigurationSpec,
        lease: &MailLease,
    ) -> Result<ConfigurationRecord> {
        spec.validate().map_err(|_| Error::Invalid)?;
        lease.validate().map_err(|_| Error::Invalid)?;
        let record = self
            .configuration_load(tx, agent, &spec.operation_id)
            .await?;
        let issued = self.configuration_recorded(tx, &record).await?;
        if issued.spec.fingerprint().map_err(|_| Error::Invalid)?
            != spec.fingerprint().map_err(|_| Error::Invalid)?
            || issued.lease.lease_token != lease.lease_token
            || spec.agent_generation != agent.generation()
        {
            return Err(Error::Conflict);
        }
        Ok(record)
    }
    async fn configuration_live(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        record: &ConfigurationRecord,
        spec: &ConfigurationSpec,
    ) -> Result<Option<Access>> {
        if !self.configuration_ready(tx, agent).await?
            || !record.current()?
            || field::<String>(&record.row, "operation_state")? != "running"
            || spec.service_revision != field::<i64>(&record.row, "service_revision")?.to_string()
        {
            return Ok(None);
        }
        let Some(access) = self.configuration_authority(tx, record).await? else {
            return Ok(None);
        };
        if !self.configuration_sources_current(tx, record, spec).await? {
            return Ok(None);
        }
        let row=one(tx,"SELECT lease_until>clock_timestamp() AND $2::timestamptz>clock_timestamp() AS current FROM webdock_mail.configuration_operation WHERE id=$1",vec![record.operation.clone().into(),access.expires_at.into()]).await?.ok_or(Error::Conflict)?;
        Ok(field::<bool>(&row, "current")?.then_some(access))
    }
    pub async fn guard_configuration(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &ConfigurationSpec,
        lease: &MailLease,
    ) -> Result<()> {
        let record = self.configuration_issued(&tx, agent, spec, lease).await?;
        let access = self
            .configuration_live(&tx, agent, &record, spec)
            .await?
            .ok_or(Error::Conflict)?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        access.commit().await
    }
}
