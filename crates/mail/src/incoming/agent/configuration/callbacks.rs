use super::*;
use base64::engine::general_purpose::STANDARD;
use x509_parser::prelude::FromDer;
fn resource_key(resource: &ConfigurationResource) -> String {
    format!(
        "{}:{}",
        match resource.kind {
            ConfigurationResourceKind::Domain => "domain",
            ConfigurationResourceKind::Mailbox => "mailbox",
            ConfigurationResourceKind::Identity => "identity",
        },
        resource.id
    )
}
fn key(checkpoint: &ConfigurationCheckpoint) -> String {
    match &checkpoint.phase {
        ConfigurationCheckpointPhase::Bound {} => "bound".into(),
        ConfigurationCheckpointPhase::Handoff { resource, .. } => {
            format!("handoff:{}", resource_key(resource))
        }
        ConfigurationCheckpointPhase::Applied { resource, .. } => {
            format!("applied:{}", resource_key(resource))
        }
    }
}
fn dkim(proof: &ConfigurationProof) -> Result<()> {
    for keys in proof.dkim.values() {
        let mut selectors = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for key in keys {
            let suffix = key
                .selector
                .strip_prefix("webdock-rsa-")
                .or_else(|| key.selector.strip_prefix("v1-rsa-"))
                .ok_or(Error::Invalid)?;
            if suffix.is_empty()
                || suffix.len() > 40
                || !selectors.insert(&key.selector)
                || !ids.insert(&key.id)
            {
                return Err(Error::Invalid);
            }
            let bytes = STANDARD
                .decode(&key.public_key)
                .map_err(|_| Error::Invalid)?;
            if STANDARD.encode(&bytes) != key.public_key {
                return Err(Error::Invalid);
            }
            let (remaining, public) = x509_parser::x509::SubjectPublicKeyInfo::from_der(&bytes)
                .map_err(|_| Error::Invalid)?;
            let x509_parser::public_key::PublicKey::RSA(key) =
                public.parsed().map_err(|_| Error::Invalid)?
            else {
                return Err(Error::Invalid);
            };
            if !remaining.is_empty()
                || key.key_size() < 2048
                || key.try_exponent().map_err(|_| Error::Invalid)? < 3
            {
                return Err(Error::Invalid);
            }
        }
    }
    Ok(())
}
impl IncomingAgent {
    async fn configuration_reference(
        &self,
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
        resource: &ConfigurationResource,
        provider: &str,
    ) -> Result<()> {
        if record.mode == ConfigurationMode::Restrict {
            return Ok(());
        }
        let table = match resource.kind {
            ConfigurationResourceKind::Domain => "domain",
            ConfigurationResourceKind::Mailbox => "mailbox",
            ConfigurationResourceKind::Identity => "identity",
        };
        if resource.kind != ConfigurationResourceKind::Domain {
            let opposite = if resource.kind == ConfigurationResourceKind::Mailbox {
                "identity"
            } else {
                "mailbox"
            };
            if one(
                tx,
                &format!(
                    "SELECT 1 FROM webdock_mail.{opposite} WHERE customer_id=$1 AND provider_id=$2"
                ),
                vec![record.customer.clone().into(), provider.into()],
            )
            .await?
            .is_some()
            {
                return Err(Error::Conflict);
            }
        }
        if !run(tx,&format!("UPDATE webdock_mail.{table} SET provider_id=$3 WHERE customer_id=$1 AND id=$2 AND (provider_id IS NULL OR provider_id=$3)"),vec![record.customer.clone().into(),resource.id.clone().into(),provider.into()]).await?{return Err(Error::Conflict);}
        Ok(())
    }
    pub async fn checkpoint_configuration(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &ConfigurationSpec,
        data: ConfigurationCheckpoint,
    ) -> Result<()> {
        data.validate().map_err(|_| Error::Invalid)?;
        let record = self
            .configuration_issued(&tx, agent, spec, &data.lease)
            .await?;
        let event_key = key(&data);
        let fingerprint =
            webdock_agent_protocol::mail::fingerprint(&data).map_err(|_| Error::Invalid)?;
        if let Some(previous)=one(&tx,"SELECT callback_sha256 FROM webdock_mail.native_configuration_checkpoint WHERE operation_id=$1 AND event_key=$2",vec![record.operation.clone().into(),event_key.clone().into()]).await?{
            if field::<String>(&previous,"callback_sha256")?!=fingerprint{return Err(Error::Conflict);}
            tx.commit().await.map_err(|_|Error::Unavailable)?;return Ok(());
        }
        if field::<Option<String>>(&record.row, "runtime_id")?
            .as_ref()
            .is_some_and(|id| id != &data.runtime_id)
        {
            return Err(Error::Conflict);
        }
        let mut claim = self.configuration_recorded(&tx, &record).await?;
        claim.checkpoints.push(data.clone());
        claim.validate_history().map_err(|_| Error::Conflict)?;
        if let ConfigurationCheckpointPhase::Applied {
            resource,
            provider_id,
            ..
        } = &data.phase
        {
            let expected = match resource.kind {
                ConfigurationResourceKind::Domain => spec
                    .snapshot
                    .domains
                    .iter()
                    .find(|r| r.id == resource.id)
                    .and_then(|r| r.provider_id.as_ref()),
                ConfigurationResourceKind::Mailbox => spec
                    .snapshot
                    .mailboxes
                    .iter()
                    .find(|r| r.id == resource.id)
                    .and_then(|r| r.provider_id.as_ref()),
                ConfigurationResourceKind::Identity => spec
                    .snapshot
                    .identities
                    .iter()
                    .find(|r| r.id == resource.id)
                    .and_then(|r| r.provider_id.as_ref()),
            };
            if expected.is_some_and(|id| id != provider_id) {
                return Err(Error::Conflict);
            }
        }
        let access = self.configuration_live(&tx, agent, &record, spec).await?;
        let ordinal=one(&tx,"SELECT count(*)::integer+1 AS ordinal FROM webdock_mail.native_configuration_checkpoint WHERE operation_id=$1",vec![record.operation.clone().into()]).await?.ok_or(Error::Unavailable)?;
        let inserted=one(&tx,"INSERT INTO webdock_mail.native_configuration_checkpoint(operation_id,ordinal,event_key,callback_sha256,checkpoint,live) SELECT $1,$2,$3,$4,$5,coalesce($6::timestamptz>clock_timestamp() AND o.state='running' AND o.lease_until>clock_timestamp(),false) FROM webdock_mail.configuration_operation o WHERE o.id=$1 RETURNING live",vec![record.operation.clone().into(),field::<i32>(&ordinal,"ordinal")?.into(),event_key.into(),fingerprint.into(),serde_json::to_value(&data).map_err(|_|Error::Invalid)?.into(),access.as_ref().map(|a|a.expires_at).into()]).await?.ok_or(Error::Conflict)?;
        if field::<bool>(&inserted, "live")? {
            match &data.phase{
                ConfigurationCheckpointPhase::Bound{}=>{if !run(&tx,"UPDATE webdock_mail.native_configuration_operation SET runtime_id=$2 WHERE operation_id=$1 AND runtime_id IS NULL",vec![record.operation.clone().into(),data.runtime_id.into()]).await?{return Err(Error::Conflict);}},
                ConfigurationCheckpointPhase::Applied{resource,provider_id,..}=>self.configuration_reference(&tx,&record,resource,provider_id).await?,
                ConfigurationCheckpointPhase::Handoff{..}=>{},
            }
            // Provider-reference writes may wait behind another transaction after the first
            // live check. Expiry rolls the entire checkpoint and projection back together.
            let current=one(&tx,"SELECT lease_until>clock_timestamp() AND $2::timestamptz>clock_timestamp() AS current FROM webdock_mail.configuration_operation WHERE id=$1",vec![record.operation.clone().into(),access.as_ref().map(|a|a.expires_at).into()]).await?.ok_or(Error::Conflict)?;
            if !field::<Option<bool>>(&current, "current")?.unwrap_or(false) {
                return Err(Error::Conflict);
            }
        } else {
            Self::configuration_review(&tx, &record).await?;
        }
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        if let Some(access) = access {
            access.commit().await?;
        }
        Ok(())
    }
    pub async fn complete_configuration(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &ConfigurationSpec,
        data: ConfigurationCompletion,
    ) -> Result<()> {
        data.validate().map_err(|_| Error::Invalid)?;
        let record = self
            .configuration_issued(&tx, agent, spec, &data.lease)
            .await?;
        if let Some(proof) = &data.proof {
            proof.validate_for(spec).map_err(|_| Error::Invalid)?;
            dkim(proof)?;
            if field::<Option<String>>(&record.row, "runtime_id")?.as_ref()
                != Some(&proof.runtime_id)
            {
                return Err(Error::Conflict);
            }
        }
        let hash = webdock_agent_protocol::mail::fingerprint(&data).map_err(|_| Error::Invalid)?;
        if let Some(previous) = field::<Option<String>>(&record.row, "receipt_sha256")? {
            if previous != hash {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            return Ok(());
        }
        let access = self.configuration_live(&tx, agent, &record, spec).await?;
        let success = if data.outcome == MailOutcome::Succeeded {
            if let Some(access) = &access {
                self.configuration_succeed(
                    &tx,
                    &record,
                    spec,
                    data.proof.as_ref().ok_or(Error::Invalid)?,
                    access.expires_at,
                )
                .await?
            } else {
                false
            }
        } else {
            false
        };
        if !success {
            Self::configuration_review(&tx, &record).await?;
        }
        if !run(&tx,"UPDATE webdock_mail.native_configuration_operation n SET receipt_sha256=$2,receipt_live=$3 WHERE operation_id=$1 AND receipt_sha256 IS NULL AND (NOT $3 OR EXISTS(SELECT 1 FROM webdock_mail.configuration_operation o WHERE o.id=n.operation_id AND o.lease_until>clock_timestamp() AND $4::timestamptz>clock_timestamp()))",vec![record.operation.clone().into(),hash.into(),success.into(),access.as_ref().map(|a|a.expires_at).into()]).await?{return Err(Error::Conflict);}
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        if let Some(access) = access {
            access.commit().await?;
        }
        Ok(())
    }
    async fn configuration_succeed(
        &self,
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
        spec: &ConfigurationSpec,
        proof: &ConfigurationProof,
        until: DateTime<Utc>,
    ) -> Result<bool> {
        let claim = self.configuration_recorded(tx, record).await?;
        for (kind, mapping) in [
            (ConfigurationResourceKind::Domain, proof.domains.clone()),
            (ConfigurationResourceKind::Mailbox, proof.mailboxes.clone()),
            (
                ConfigurationResourceKind::Identity,
                proof
                    .identities
                    .iter()
                    .filter_map(|(id, p)| p.as_ref().map(|p| (id.clone(), p.clone())))
                    .collect(),
            ),
        ] {
            if record.mode == ConfigurationMode::Restrict
                && kind != ConfigurationResourceKind::Identity
            {
                continue;
            }
            for (id, provider) in mapping {
                if !claim.checkpoints.iter().any(|checkpoint|matches!(&checkpoint.phase,ConfigurationCheckpointPhase::Applied{resource,provider_id,..} if resource.kind==kind&&resource.id==id&&provider_id==&provider)){return Err(Error::Conflict);}
            }
        }
        let admitted=one(tx,"UPDATE webdock_mail.configuration_operation SET state='succeeded',finished_at=clock_timestamp() WHERE id=$1 AND state='running' AND lease_until>clock_timestamp() AND $2::timestamptz>clock_timestamp() RETURNING id",vec![record.operation.clone().into(),until.into()]).await?.is_some();
        if !admitted {
            return Ok(false);
        }
        if !record.mode.advances_configuration() {
            for grant in &spec.snapshot.grants {
                if !run(tx,"UPDATE webdock_mail.mailbox_grant SET applied_revision=revision WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3 AND revision=$4 AND NOT enabled",vec![record.customer.clone().into(),grant.mailbox_id.clone().into(),grant.subject.clone().into(),super::super::super::revision(&grant.revision)?.into()]).await?{return Err(Error::Conflict);}
            }
            return Ok(true);
        }
        for (kind, mapping) in [
            ("domain", proof.domains.clone()),
            ("mailbox", proof.mailboxes.clone()),
            (
                "identity",
                proof
                    .identities
                    .iter()
                    .filter_map(|(id, p)| p.as_ref().map(|p| (id.clone(), p.clone())))
                    .collect(),
            ),
        ] {
            for (id, provider) in mapping {
                if !run(tx,&format!("UPDATE webdock_mail.{kind} SET state='ready' WHERE customer_id=$1 AND id=$2 AND provider_id=$3"),vec![record.customer.clone().into(),id.into(),provider.into()]).await?{return Err(Error::Conflict);}
            }
        }
        for (id, keys) in &proof.dkim {
            if !run(
                tx,
                "UPDATE webdock_mail.domain SET dkim_keys=$3 WHERE customer_id=$1 AND id=$2",
                vec![
                    record.customer.clone().into(),
                    id.clone().into(),
                    serde_json::to_value(keys)
                        .map_err(|_| Error::Invalid)?
                        .into(),
                ],
            )
            .await?
            {
                return Err(Error::Conflict);
            }
        }
        // Exact whole-configuration fencing above also covers disabled grants omitted from the
        // bounded enabled set: every existing identity's complete effective groups was observed.
        run(tx,"UPDATE webdock_mail.mailbox_grant SET applied_revision=revision WHERE customer_id=$1 AND NOT enabled",vec![record.customer.clone().into()]).await?;
        for grant in &spec.snapshot.grants {
            if !run(tx,"UPDATE webdock_mail.mailbox_grant SET applied_revision=revision WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3 AND revision=$4 AND enabled",vec![record.customer.clone().into(),grant.mailbox_id.clone().into(),grant.subject.clone().into(),super::super::super::revision(&grant.revision)?.into()]).await?{return Err(Error::Conflict);}
        }
        if !run(tx,"UPDATE webdock_mail.configuration SET applied_revision=$2 WHERE customer_id=$1 AND revision=$2",vec![record.customer.clone().into(),record.revision.into()]).await?{return Err(Error::Conflict);}
        Ok(true)
    }
}
