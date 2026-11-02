use super::*;
use crate::{outbox::run, proof::valid_id};

struct Allocated {
    resource: String,
    identity: String,
    operation: String,
    event: String,
}
impl NativeIncoming {
    pub async fn manage_mailboxes(
        &self,
        principal: &Principal,
        stored: &StoredAuthority,
        customer: &str,
        command: MailboxCommand,
    ) -> Result<MailboxChange> {
        if !self.config.mailboxes_enabled {
            return Err(Error::Unavailable);
        }
        validate(&command)?;
        let ids = Allocated {
            resource: self.id().await?,
            identity: self.id().await?,
            operation: self.id().await?,
            event: self.id().await?,
        };
        if !self.config.enabled {
            return Err(Error::Unavailable);
        }
        let access = if matches!(command, MailboxCommand::Revoke { .. }) {
            authority::customer_for_revocation(
                &self.dependencies,
                &self.signer,
                principal,
                stored,
                customer,
            )
            .await?
        } else {
            self.authorize(principal, stored, customer, true).await?
        };
        let tx = &access.business;
        one(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("mail-configuration:{customer}").into()],
        )
        .await?;
        if !matches!(command, MailboxCommand::Revoke { .. }) && !ready(tx, customer).await? {
            return Err(Error::Unavailable);
        }
        let restricted_subject = match &command {
            MailboxCommand::Revoke { subject, .. } => Some(subject.clone()),
            _ => None,
        };
        let (id, change) = match command {
            MailboxCommand::ConfigurationRetry { revision: expected } => {
                if !access.operator {
                    return Err(Error::Unavailable);
                }
                retry(tx, customer, &expected).await?;
                (None, Some(("configuration.retry", customer.to_owned())))
            }
            MailboxCommand::DomainAttach { sender_domain_id } => {
                let (id, changed) =
                    attach_domain(tx, customer, &sender_domain_id, &ids.resource).await?;
                (Some(id.clone()), changed.then_some(("domain.attach", id)))
            }
            MailboxCommand::MailboxCreate {
                domain_id,
                local_part,
                name,
                quota_bytes,
            } => {
                create_mailbox(
                    tx,
                    customer,
                    &ids.resource,
                    NewMailbox {
                        domain: &domain_id,
                        local_part: &local_part,
                        name: name.trim(),
                        quota: quota_bytes,
                    },
                )
                .await?;
                (
                    Some(ids.resource.clone()),
                    Some(("mailbox.create", ids.resource.clone())),
                )
            }
            MailboxCommand::Grant {
                mailbox_id,
                subject,
            } => {
                let changed = change_grant(
                    &access,
                    customer,
                    &mailbox_id,
                    &subject,
                    Some(&ids.identity),
                )
                .await?;
                (None, changed.then_some(("grant", mailbox_id)))
            }
            MailboxCommand::Revoke {
                mailbox_id,
                subject,
            } => {
                let changed = change_grant(&access, customer, &mailbox_id, &subject, None).await?;
                (None, changed.then_some(("revoke", mailbox_id)))
            }
        };
        if let Some((action, target)) = change {
            queue(
                &access,
                stored,
                customer,
                action,
                &target,
                &ids,
                restricted_subject.as_deref(),
            )
            .await?;
        }
        commit(access).await?;
        Ok(MailboxChange { ok: true, id })
    }
}
fn validate(command: &MailboxCommand) -> Result<()> {
    let valid = match command {
        MailboxCommand::ConfigurationRetry { revision: value } => {
            revision(value).is_ok_and(|v| v > 0)
        }
        MailboxCommand::DomainAttach { sender_domain_id } => valid_id(sender_domain_id),
        MailboxCommand::Grant {
            mailbox_id,
            subject,
        }
        | MailboxCommand::Revoke {
            mailbox_id,
            subject,
        } => valid_id(mailbox_id) && valid_id(subject),
        MailboxCommand::MailboxCreate {
            domain_id,
            local_part,
            name,
            quota_bytes,
        } => {
            valid_id(domain_id)
                && !local_part.is_empty()
                && local_part.len() <= 64
                && local_part.split(['.', '_', '+', '-']).all(|part| {
                    !part.is_empty()
                        && part
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                })
                && !name.trim().is_empty()
                && name.trim().encode_utf16().count() <= 80
                && !name.chars().any(|c| c == '\0' || c == '\r' || c == '\n')
                && (1048576..=8589934592).contains(quota_bytes)
        }
    };
    if !valid {
        return Err(Error::Invalid);
    }
    Ok(())
}
async fn queue(
    access: &authority::AuthorizedCustomer,
    stored: &StoredAuthority,
    customer: &str,
    action: &str,
    target: &str,
    ids: &Allocated,
    restricted_subject: Option<&str>,
) -> Result<()> {
    let tx = &access.business;
    let principal = &access.principal;
    let subject = principal.user_id.as_str();
    let row=one(tx,"INSERT INTO webdock_mail.configuration(customer_id) VALUES($1) ON CONFLICT(customer_id) DO UPDATE SET revision=configuration.revision+1 RETURNING revision",vec![customer.into()]).await?.ok_or(Error::Unavailable)?;
    let revision: i64 = field(&row, "revision")?;
    run(tx,"UPDATE webdock_mail.configuration_operation o SET state='superseded',finished_at=now() WHERE o.customer_id=$1 AND o.state='pending' AND NOT EXISTS(SELECT 1 FROM webdock_mail.native_configuration_operation n WHERE n.operation_id=o.id AND n.mode='restrict')",vec![customer.into()]).await?;
    run(tx,"INSERT INTO webdock_mail.configuration_operation(id,customer_id,revision,requested_by,reason) VALUES($1,$2,$3,$4,$5)",vec![ids.operation.clone().into(),customer.into(),revision.into(),subject.into(),action.into()]).await?;
    let restriction = if let Some(subject) = restricted_subject {
        let row=one(tx,"SELECT revision FROM webdock_mail.mailbox_grant WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3 AND NOT enabled",vec![customer.into(),target.into(),subject.into()]).await?.ok_or(Error::Conflict)?;
        Some((
            target.to_owned(),
            subject.to_owned(),
            field::<i64>(&row, "revision")?,
        ))
    } else {
        None
    };
    run(tx,"INSERT INTO webdock_mail.native_configuration_operation(operation_id,customer_id,cluster_id,subject,session_id,native_authority,mode,restricted_mailbox_id,restricted_subject,restricted_revision,admission_member_id,operator_authority) SELECT $1,$2,host_id,$3,$4,$5,$6,$7,$8,$9,$10,$11 FROM webdock_mail.service WHERE customer_id=$2",vec![
        ids.operation.clone().into(),customer.into(),subject.into(),principal.session_id.clone().into(),
        serde_json::to_value(stored).map_err(|_|Error::Invalid)?.into(),
        if restriction.is_some(){"restrict"}else{"apply"}.into(),
        restriction.as_ref().map(|v|v.0.clone()).into(),restriction.as_ref().map(|v|v.1.clone()).into(),restriction.map(|v|v.2).into(),
        if access.operator {None::<String>} else {access.member_id.clone()}.into(),access.operator.into()
    ]).await?;
    run(tx,"INSERT INTO webdock_mail.access_event(id,customer_id,subject,action,target_id) VALUES($1,$2,$3,$4,$5)",vec![ids.event.clone().into(),customer.into(),subject.into(),action.into(),target.into()]).await?;
    Ok(())
}
async fn retry(tx: &DatabaseTransaction, customer: &str, expected: &str) -> Result<()> {
    let current = one(
        tx,
        "SELECT revision FROM webdock_mail.configuration WHERE customer_id=$1 FOR UPDATE",
        vec![customer.into()],
    )
    .await?
    .ok_or(Error::Conflict)?;
    if field::<i64>(&current, "revision")? != revision(expected)? {
        return Err(Error::Conflict);
    }
    if !retry_available(tx, customer).await? {
        return Err(Error::Conflict);
    }
    run(tx,"UPDATE webdock_mail.configuration_operation SET state='superseded',finished_at=coalesce(finished_at,now()) WHERE customer_id=$1 AND state='needs_review'",vec![customer.into()]).await?;
    Ok(())
}
pub(super) async fn retry_available(tx: &DatabaseTransaction, customer: &str) -> Result<bool> {
    let row=one(tx,"SELECT c.revision<>c.applied_revision AND NOT EXISTS(SELECT 1 FROM webdock_mail.configuration_operation r WHERE r.customer_id=c.customer_id AND r.state='running') AND (EXISTS(SELECT 1 FROM webdock_mail.configuration_operation r WHERE r.customer_id=c.customer_id AND r.state='needs_review') OR (o.state='succeeded' AND n.mode='restrict') OR (o.state='pending' AND n.operation_id IS NULL)) AS allowed FROM webdock_mail.configuration c JOIN webdock_mail.configuration_operation o ON o.customer_id=c.customer_id AND o.revision=c.revision LEFT JOIN webdock_mail.native_configuration_operation n ON n.operation_id=o.id WHERE c.customer_id=$1",vec![customer.into()]).await?;
    Ok(row
        .as_ref()
        .map(|r| field::<Option<bool>>(r, "allowed"))
        .transpose()?
        .flatten()
        .unwrap_or(false))
}
async fn attach_domain(
    tx: &DatabaseTransaction,
    customer: &str,
    sender: &str,
    allocated: &str,
) -> Result<(String, bool)> {
    let source=one(tx,"SELECT domain FROM webdock_auth.mail_sender_domain WHERE id=$1 AND customer_id=$2 AND deleted_at IS NULL AND status='ownership_verified' FOR SHARE",vec![sender.into(),customer.into()]).await?.ok_or(Error::Unavailable)?;
    let domain: String = field(&source, "domain")?;
    let canonical = url::Host::parse(&domain).map_err(|_| Error::Invalid)?;
    if !matches!(&canonical,url::Host::Domain(host) if host==&domain && host.contains('.') && !host.ends_with('.'))
    {
        return Err(Error::Invalid);
    }
    if let Some(row) = one(
        tx,
        "SELECT id FROM webdock_mail.domain WHERE sender_domain_id=$1 AND customer_id=$2",
        vec![sender.into(), customer.into()],
    )
    .await?
    {
        return Ok((field(&row, "id")?, false));
    }
    let count = one(
        tx,
        "SELECT count(*) AS count FROM webdock_mail.domain WHERE customer_id=$1",
        vec![customer.into()],
    )
    .await?
    .ok_or(Error::Unavailable)?;
    if field::<i64>(&count, "count")? >= 32 {
        return Err(Error::Conflict);
    }
    run(
        tx,
        "INSERT INTO webdock_mail.domain(id,customer_id,sender_domain_id,name) VALUES($1,$2,$3,$4)",
        vec![
            allocated.into(),
            customer.into(),
            sender.into(),
            domain.into(),
        ],
    )
    .await?;
    Ok((allocated.into(), true))
}
struct NewMailbox<'a> {
    domain: &'a str,
    local_part: &'a str,
    name: &'a str,
    quota: u64,
}
async fn create_mailbox(
    tx: &DatabaseTransaction,
    customer: &str,
    allocated: &str,
    mailbox: NewMailbox<'_>,
) -> Result<()> {
    if one(tx,"SELECT d.id FROM webdock_mail.domain d JOIN webdock_auth.mail_sender_domain s ON s.id=d.sender_domain_id AND s.customer_id=d.customer_id WHERE d.customer_id=$1 AND d.id=$2 AND s.deleted_at IS NULL AND s.status='ownership_verified' FOR SHARE OF d,s",vec![customer.into(),mailbox.domain.into()]).await?.is_none() {return Err(Error::Unavailable);}
    let used=one(tx,"SELECT count(*) AS count,coalesce(sum(quota_bytes),0)::bigint AS bytes FROM webdock_mail.mailbox WHERE customer_id=$1",vec![customer.into()]).await?.ok_or(Error::Unavailable)?;
    let bytes = u64::try_from(field::<i64>(&used, "bytes")?).map_err(|_| Error::Unavailable)?;
    if field::<i64>(&used, "count")? >= 64
        || bytes
            .checked_add(mailbox.quota)
            .is_none_or(|v| v > 8589934592)
    {
        return Err(Error::Conflict);
    }
    if one(
        tx,
        "SELECT 1 FROM webdock_mail.mailbox WHERE domain_id=$1 AND local_part=$2",
        vec![mailbox.domain.into(), mailbox.local_part.into()],
    )
    .await?
    .is_some()
    {
        return Err(Error::Conflict);
    }
    run(tx,"INSERT INTO webdock_mail.mailbox(id,customer_id,domain_id,local_part,name,quota_bytes) VALUES($1,$2,$3,$4,$5,$6)",vec![allocated.into(),customer.into(),mailbox.domain.into(),mailbox.local_part.into(),mailbox.name.into(),i64::try_from(mailbox.quota).map_err(|_|Error::Invalid)?.into()]).await?;
    Ok(())
}
async fn change_grant(
    access: &authority::AuthorizedCustomer,
    customer: &str,
    mailbox: &str,
    subject: &str,
    identity: Option<&str>,
) -> Result<bool> {
    let tx = &access.business;
    if one(
        tx,
        "SELECT id FROM webdock_mail.mailbox WHERE customer_id=$1 AND id=$2 AND enabled FOR UPDATE",
        vec![customer.into(), mailbox.into()],
    )
    .await?
    .is_none()
    {
        return Err(Error::Unavailable);
    }
    let existing=one(tx,"SELECT enabled,member_id,operator_grant,clerk_grant FROM webdock_mail.mailbox_grant WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3 FOR UPDATE",vec![customer.into(),mailbox.into(),subject.into()]).await?;
    let Some(identity) = identity else {
        let enabled = existing
            .as_ref()
            .map(|row| field::<bool>(row, "enabled"))
            .transpose()?
            .unwrap_or(false);
        if !enabled {
            return Ok(false);
        }
        run(tx,"UPDATE webdock_mail.mailbox_grant SET enabled=false,revision=revision+1 WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3",vec![customer.into(),mailbox.into(),subject.into()]).await?;
        return Ok(true);
    };
    // Lock the actual local identity and membership; a stale projection never creates authority.
    let user=one(tx,"SELECT id,role,\"twoFactorEnabled\" AS mfa FROM webdock_auth.\"user\" WHERE id=$1 AND NOT coalesce(banned,false) AND \"emailVerified\" AND NOT \"mustChangePassword\" FOR SHARE",vec![subject.into()]).await?.ok_or(Error::Unavailable)?;
    let member=one(tx,"SELECT m.id FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE m.\"userId\"=$1 AND t.customer_id=$2 AND m.role<>'' FOR SHARE OF m",vec![subject.into(),customer.into()]).await?;
    let member_id = member
        .as_ref()
        .map(|row| field::<String>(row, "id"))
        .transpose()?;
    let operator = member_id.is_none();
    if operator
        && !((access.operator
            || (access.principal.is_acting_customer() && subject == access.principal.user_id))
            && field::<String>(&user, "role")? == "operator"
            && field::<bool>(&user, "mfa")?)
    {
        return Err(Error::Unavailable);
    }
    if let Some(row) = &existing
        && field::<bool>(row, "enabled")?
        && field::<Option<String>>(row, "member_id")? == member_id
        && field::<bool>(row, "operator_grant")? == operator
        && field::<Option<String>>(row, "clerk_grant")?.is_none()
    {
        return Ok(false);
    }
    let counts=one(tx,"SELECT (SELECT count(*) FROM webdock_mail.mailbox_grant WHERE customer_id=$1 AND enabled) AS grants,(SELECT count(*) FROM webdock_mail.identity WHERE customer_id=$1) AS identities,EXISTS(SELECT 1 FROM webdock_mail.identity WHERE customer_id=$1 AND subject=$2) AS present",vec![customer.into(),subject.into()]).await?.ok_or(Error::Unavailable)?;
    if field::<i64>(&counts, "grants")? >= 256
        || (!field::<bool>(&counts, "present")? && field::<i64>(&counts, "identities")? >= 128)
    {
        return Err(Error::Conflict);
    }
    run(tx,"INSERT INTO webdock_mail.identity(id,customer_id,subject) VALUES($1,$2,$3) ON CONFLICT(customer_id,subject) DO NOTHING",vec![identity.into(),customer.into(),subject.into()]).await?;
    run(tx,"INSERT INTO webdock_mail.mailbox_grant(customer_id,mailbox_id,subject,member_id,operator_grant,clerk_grant) VALUES($1,$2,$3,$4,$5,NULL) ON CONFLICT(mailbox_id,subject) DO UPDATE SET enabled=true,member_id=excluded.member_id,operator_grant=excluded.operator_grant,clerk_grant=NULL,revision=mailbox_grant.revision+1",vec![customer.into(),mailbox.into(),subject.into(),member_id.into(),operator.into()]).await?;
    Ok(true)
}
