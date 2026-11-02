use super::*;
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EffectiveGrant {
    mailbox: String,
    subject: String,
    revision: i64,
}
async fn effective(
    tx: &DatabaseTransaction,
    customer: &str,
    applied_only: bool,
) -> Result<Vec<EffectiveGrant>> {
    super::super::super::rows(tx,"SELECT g.mailbox_id,g.subject,g.revision FROM webdock_mail.mailbox_grant g JOIN webdock_mail.mailbox b ON b.id=g.mailbox_id AND b.customer_id=g.customer_id JOIN webdock_auth.\"user\" u ON u.id=g.subject WHERE g.customer_id=$1 AND g.enabled AND b.enabled AND g.clerk_grant IS NULL AND NOT coalesce(u.banned,false) AND u.\"emailVerified\" AND NOT u.\"mustChangePassword\" AND (NOT $2 OR (g.revision=g.applied_revision AND b.provider_id IS NOT NULL)) AND ((g.operator_grant AND u.role='operator' AND u.\"twoFactorEnabled\") OR EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE m.id=g.member_id AND m.\"userId\"=g.subject AND t.customer_id=g.customer_id AND m.role<>'')) ORDER BY g.mailbox_id,g.subject LIMIT 257",vec![customer.into(),applied_only.into()]).await?.iter().map(|r|Ok(EffectiveGrant{mailbox:field(r,"mailbox_id")?,subject:field(r,"subject")?,revision:field(r,"revision")?})).collect()
}
impl IncomingAgent {
    async fn configuration_instance(
        &self,
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
    ) -> Result<(ConfigurationCredential, String)> {
        let instance=one(tx,"SELECT encrypted_credentials,smtp_certificate_sha256 FROM webdock_mail.instance WHERE customer_id=$1 AND verified_at IS NOT NULL AND smtp_certificate_revision=$2 FOR SHARE",vec![record.customer.clone().into(),field::<i64>(&record.row,"service_revision")?.into()]).await?.ok_or(Error::Conflict)?;
        let credentials: Credentials = self.cipher.open(
            &field::<String>(&instance, "encrypted_credentials")?,
            &format!("instance:{}", record.customer),
        )?;
        if credentials.bootstrap_password.is_some() {
            return Err(Error::Conflict);
        }
        Ok((
            ConfigurationCredential {
                username: credentials.username.ok_or(Error::Conflict)?,
                password: credentials.password.ok_or(Error::Conflict)?,
            },
            field::<Option<String>>(&instance, "smtp_certificate_sha256")?
                .ok_or(Error::Conflict)?,
        ))
    }
    async fn configuration_snapshot(
        &self,
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
    ) -> Result<(ConfigurationSnapshot, ConfigurationMaterial, String)> {
        let (administrator, pin) = self.configuration_instance(tx, record).await?;
        let mut material = ConfigurationMaterial {
            administrator,
            identities: BTreeMap::new(),
        };
        let restricted = record.mode == ConfigurationMode::Restrict;
        let grants = effective(tx, &record.customer, restricted).await?;
        if grants.len() > 256 {
            return Err(Error::Conflict);
        }
        let domains = if restricted {
            vec![]
        } else {
            super::super::super::rows(tx,"SELECT d.id,d.name,d.provider_id,s.status='ownership_verified' AND s.deleted_at IS NULL AS verified FROM webdock_mail.domain d JOIN webdock_auth.mail_sender_domain s ON s.id=d.sender_domain_id AND s.customer_id=d.customer_id WHERE d.customer_id=$1 ORDER BY d.id LIMIT 33",vec![record.customer.clone().into()]).await?.iter().map(|r|Ok(ConfigurationDomain{id:field(r,"id")?,name:field(r,"name")?,provider_id:field(r,"provider_id")?,ownership_verified:field(r,"verified")?})).collect::<Result<Vec<_>>>()?
        };
        let mut mailboxes=super::super::super::rows(tx,"SELECT id,domain_id,local_part,quota_bytes,provider_id FROM webdock_mail.mailbox WHERE customer_id=$1 AND enabled ORDER BY id LIMIT 65",vec![record.customer.clone().into()]).await?.iter().map(|r|Ok(ConfigurationMailbox{id:field(r,"id")?,domain_id:field(r,"domain_id")?,local_part:field(r,"local_part")?,quota_bytes:u64::try_from(field::<i64>(r,"quota_bytes")?).map_err(|_|Error::Conflict)?,provider_id:field(r,"provider_id")?})).collect::<Result<Vec<_>>>()?;
        if restricted {
            mailboxes
                .retain(|m| m.provider_id.is_some() && grants.iter().any(|g| g.mailbox == m.id));
        }
        let rows=super::super::super::rows(tx,"SELECT id,subject,provider_id,encrypted_credentials FROM webdock_mail.identity WHERE customer_id=$1 ORDER BY id LIMIT 129 FOR UPDATE",vec![record.customer.clone().into()]).await?;
        let mut identities = Vec::new();
        for row in rows {
            let id: String = field(&row, "id")?;
            let subject: String = field(&row, "subject")?;
            let provider_id: Option<String> = field(&row, "provider_id")?;
            if restricted && provider_id.is_none() {
                continue;
            }
            let mailbox_ids = grants
                .iter()
                .filter(|g| g.subject == subject)
                .map(|g| g.mailbox.clone())
                .collect::<Vec<_>>();
            let enabled = !mailbox_ids.is_empty();
            if !restricted && (enabled || provider_id.is_some()) {
                let username = format!("wd-{id}@mail-{}.{}", record.customer, self.hostname_suffix);
                let secret = if let Some(encrypted) =
                    field::<Option<String>>(&row, "encrypted_credentials")?
                {
                    self.cipher.open::<ConfigurationCredential>(
                        &encrypted,
                        &format!("identity:{}:{id}", record.customer),
                    )?
                } else {
                    if provider_id.is_some() {
                        return Err(Error::Conflict);
                    }
                    let secret = ConfigurationCredential {
                        username: username.clone(),
                        password: random()?,
                    };
                    run(tx,"UPDATE webdock_mail.identity SET encrypted_credentials=$2 WHERE customer_id=$1 AND id=$3 AND encrypted_credentials IS NULL",vec![record.customer.clone().into(),self.cipher.seal(&secret,&format!("identity:{}:{id}",record.customer))?.into(),id.clone().into()]).await?;
                    secret
                };
                if secret.username != username {
                    return Err(Error::Conflict);
                }
                material.identities.insert(id.clone(), secret);
            }
            identities.push(ConfigurationIdentity {
                id,
                subject,
                provider_id,
                enabled,
                mailbox_ids,
            });
        }
        let granted = if restricted {
            vec![ConfigurationGrant {
                mailbox_id: field(&record.row, "restricted_mailbox_id")?,
                subject: field(&record.row, "restricted_subject")?,
                revision: field::<i64>(&record.row, "restricted_revision")?.to_string(),
                enabled: false,
            }]
        } else {
            grants
                .into_iter()
                .map(|g| ConfigurationGrant {
                    mailbox_id: g.mailbox,
                    subject: g.subject,
                    revision: g.revision.to_string(),
                    enabled: true,
                })
                .collect()
        };
        Ok((
            ConfigurationSnapshot {
                domains,
                mailboxes,
                identities,
                grants: granted,
            },
            material,
            pin,
        ))
    }
    pub(super) async fn configuration_sources_current(
        &self,
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
        spec: &ConfigurationSpec,
    ) -> Result<bool> {
        let (administrator, pin) = match self.configuration_instance(tx, record).await {
            Ok(value) => value,
            Err(Error::Conflict) => return Ok(false),
            Err(error) => return Err(error),
        };
        let material = ConfigurationMaterial {
            administrator,
            identities: BTreeMap::new(),
        };
        if material
            .management_fingerprint()
            .map_err(|_| Error::Conflict)?
            != spec.target.management_sha256
            || pin != spec.target.certificate_sha256
        {
            return Ok(false);
        }
        let grants = effective(
            tx,
            &record.customer,
            record.mode == ConfigurationMode::Restrict,
        )
        .await?;
        let expected = spec
            .snapshot
            .identities
            .iter()
            .flat_map(|i| i.mailbox_ids.iter().map(|m| (m.clone(), i.subject.clone())))
            .collect::<BTreeSet<_>>();
        let current = grants
            .iter()
            .filter(|g| {
                spec.snapshot
                    .identities
                    .iter()
                    .any(|i| i.subject == g.subject)
            })
            .map(|g| (g.mailbox.clone(), g.subject.clone()))
            .collect::<BTreeSet<_>>();
        if (record.mode == ConfigurationMode::Apply && current != expected)
            || (record.mode == ConfigurationMode::Restrict && !current.is_subset(&expected))
        {
            return Ok(false);
        }
        if record.mode == ConfigurationMode::Restrict {
            return Ok(one(tx,"SELECT 1 FROM webdock_mail.mailbox_grant WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3 AND revision=$4 AND NOT enabled",vec![record.customer.clone().into(),field::<String>(&record.row,"restricted_mailbox_id")?.into(),field::<String>(&record.row,"restricted_subject")?.into(),field::<i64>(&record.row,"restricted_revision")?.into()]).await?.is_some());
        }
        let granted = spec
            .snapshot
            .grants
            .iter()
            .map(|g| {
                Ok(EffectiveGrant {
                    mailbox: g.mailbox_id.clone(),
                    subject: g.subject.clone(),
                    revision: super::super::super::revision(&g.revision)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if grants != granted {
            return Ok(false);
        }
        let domains=super::super::super::rows(tx,"SELECT d.id,d.name,s.status='ownership_verified' AND s.deleted_at IS NULL AS verified FROM webdock_mail.domain d JOIN webdock_auth.mail_sender_domain s ON s.id=d.sender_domain_id AND s.customer_id=d.customer_id WHERE d.customer_id=$1 ORDER BY d.id LIMIT 33",vec![record.customer.clone().into()]).await?;
        if domains.len() != spec.snapshot.domains.len() {
            return Ok(false);
        }
        for row in domains {
            let id: String = field(&row, "id")?;
            let name: String = field(&row, "name")?;
            if !field::<bool>(&row, "verified")?
                || !spec
                    .snapshot
                    .domains
                    .iter()
                    .any(|d| d.id == id && d.name == name)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(super) async fn configuration_issue(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        record: &ConfigurationRecord,
        expires: DateTime<Utc>,
    ) -> Result<ConfigurationClaim> {
        let (snapshot, material, certificate_sha256) =
            self.configuration_snapshot(tx, record).await?;
        let spec = ConfigurationSpec {
            version: 1,
            operation_id: record.operation.clone(),
            customer_id: record.customer.clone(),
            cluster_id: agent.cluster_id().into(),
            revision: record.revision.to_string(),
            service_revision: field::<i64>(&record.row, "service_revision")?.to_string(),
            agent_generation: agent.generation(),
            mode: record.mode,
            target: ConfigurationTarget {
                instance_key: field(&record.row, "instance_key")?,
                hostname: format!("mail-{}.{}", record.customer, self.hostname_suffix),
                image: self.image.clone(),
                resources: MailResources::FIXED,
                management_sha256: material
                    .management_fingerprint()
                    .map_err(|_| Error::Invalid)?,
                certificate_sha256,
            },
            material_sha256: material.fingerprint().map_err(|_| Error::Invalid)?,
            snapshot,
        };
        spec.validate().map_err(|_| Error::Conflict)?;
        let lease = MailLease {
            lease_token: uuid::Uuid::new_v4().to_string(),
        };
        let until=one(tx,"UPDATE webdock_mail.configuration_operation SET state='running',lease_token=$2,lease_until=least(clock_timestamp()+interval '120 seconds',$4::timestamptz),agent_generation=$3,desired=$5 WHERE id=$1 AND state='pending' AND $4::timestamptz>clock_timestamp() RETURNING lease_until",vec![record.operation.clone().into(),lease.lease_token.clone().into(),agent.generation().into(),expires.into(),serde_json::to_value(&spec.snapshot).map_err(|_|Error::Invalid)?.into()]).await?.ok_or(Error::Conflict)?;
        if !run(tx,"UPDATE webdock_mail.native_configuration_operation SET request_sha256=$2,spec=$3,encrypted_material=$4,material_sha256=$5,lease_token_sha256=$6,agent_generation=$7 WHERE operation_id=$1 AND request_sha256 IS NULL",vec![record.operation.clone().into(),spec.fingerprint().map_err(|_|Error::Invalid)?.into(),serde_json::to_value(&spec).map_err(|_|Error::Invalid)?.into(),self.cipher.seal(&material,&format!("configuration:{}",record.operation))?.into(),spec.material_sha256.clone().into(),digest(&lease.lease_token).into(),agent.generation().into()]).await?{return Err(Error::Conflict);}
        Ok(ConfigurationClaim {
            spec,
            lease,
            lease_until: field(&until, "lease_until")?,
            material,
            checkpoints: vec![],
        })
    }
}
