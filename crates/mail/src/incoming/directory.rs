use super::*;

impl NativeIncoming {
    pub async fn directory(
        &self,
        principal: &Principal,
        stored: &StoredAuthority,
        customer: &str,
    ) -> Result<MailboxDirectory> {
        if !self.config.mailboxes_enabled {
            return Err(Error::Unavailable);
        }
        let access = self.authorize(principal, stored, customer, false).await?;
        let tx = &access.business;
        let manage = can_manage(&access, customer).await?;
        let ready = ready(tx, customer).await?;
        let mailboxes=rows(tx,
            "SELECT b.id,b.name,b.local_part||'@'||d.name AS address,b.state,b.enabled,b.quota_bytes::text AS quota_bytes,
             coalesce($6 AND g.enabled AND g.revision=g.applied_revision AND b.enabled AND b.state='ready' AND i.state='ready'
             AND b.provider_id IS NOT NULL AND i.provider_id IS NOT NULL AND g.clerk_grant IS NULL
             AND (g.member_id=$4 OR (g.operator_grant AND $5)),false) AS can_open
             FROM webdock_mail.mailbox b JOIN webdock_mail.domain d ON d.id=b.domain_id AND d.customer_id=b.customer_id
             LEFT JOIN webdock_mail.mailbox_grant g ON g.mailbox_id=b.id AND g.customer_id=b.customer_id AND g.subject=$2
             LEFT JOIN webdock_mail.identity i ON i.customer_id=b.customer_id AND i.subject=$2
             WHERE b.customer_id=$1 AND ($3 OR g.enabled) ORDER BY b.name,b.id LIMIT 64",
            vec![customer.into(),access.principal.user_id.clone().into(),manage.into(),access.member_id.clone().into(),(access.operator || access.principal.is_acting_customer()).into(),ready.into()]
        ).await?.iter().map(|row| Ok(Mailbox{id:field(row,"id")?,name:field(row,"name")?,address:field(row,"address")?,state:state(row,"state")?,enabled:field(row,"enabled")?,quota_bytes:field(row,"quota_bytes")?,can_open:field(row,"can_open")?})).collect::<Result<Vec<_>>>()?;
        let domains=rows(tx,"SELECT id,name,state,dkim_keys FROM webdock_mail.domain WHERE customer_id=$1 ORDER BY name LIMIT 32",vec![customer.into()]).await?.iter().map(|row|Ok(Domain{id:field(row,"id")?,name:field(row,"name")?,state:state(row,"state")?,dkim_keys:decode(row,"dkim_keys")?})).collect::<Result<Vec<_>>>()?;
        let (verified_domains, people, grants) =
            management_directory(tx, customer, manage, access.operator).await?;
        let configuration=one(tx,"SELECT c.revision,c.applied_revision,CASE WHEN EXISTS(SELECT 1 FROM webdock_mail.configuration_operation p WHERE p.customer_id=c.customer_id AND p.state='needs_review') THEN 'needs_review' ELSE o.state END AS state FROM webdock_mail.configuration c LEFT JOIN webdock_mail.configuration_operation o ON o.customer_id=c.customer_id AND o.revision=c.revision WHERE c.customer_id=$1",vec![customer.into()]).await?.as_ref().map(|row| -> Result<Configuration> {
            let status=field::<Option<String>>(row,"state")?.map(|value|serde_json::from_value(serde_json::Value::String(value)).map_err(|_|Error::Unavailable)).transpose()?;
            Ok(Configuration{revision:field::<i64>(row,"revision")?.to_string(),applied_revision:field::<i64>(row,"applied_revision")?.to_string(),state:status})
        }).transpose()?;
        let writes = one(
            tx,
            "SELECT 1 FROM webdock_auth.plugin_installation WHERE id='mail' AND state='enabled'",
            vec![],
        )
        .await?
        .is_some();
        let result = MailboxDirectory {
            customer_id: customer.into(),
            operator: access.operator,
            configuration,
            can_manage: manage,
            can_configure: manage && writes && ready,
            can_retry_configuration: access.operator
                && manage
                && writes
                && ready
                && configuration::retry_available(tx, customer).await?,
            mailboxes,
            domains,
            verified_domains,
            people,
            grants,
        };
        commit(access).await?;
        Ok(result)
    }
}
async fn management_directory(
    tx: &DatabaseTransaction,
    customer: &str,
    manage: bool,
    operator: bool,
) -> Result<(Vec<VerifiedDomain>, Vec<Person>, Vec<Grant>)> {
    if !manage {
        return Ok((vec![], vec![], vec![]));
    }
    let domains=rows(tx,"SELECT id,domain FROM webdock_auth.mail_sender_domain WHERE deleted_at IS NULL AND customer_id=$1 AND status='ownership_verified' ORDER BY domain LIMIT 32",vec![customer.into()]).await?.iter().map(|row|Ok(VerifiedDomain{id:field(row,"id")?,domain:field(row,"domain")?})).collect::<Result<Vec<_>>>()?;
    let people=rows(tx,"SELECT u.id,u.name,u.email FROM webdock_auth.\"user\" u WHERE NOT coalesce(u.banned,false) AND u.\"emailVerified\" AND NOT u.\"mustChangePassword\" AND (($2 AND u.role='operator' AND u.\"twoFactorEnabled\") OR EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE m.\"userId\"=u.id AND t.customer_id=$1)) ORDER BY u.name,u.id LIMIT 128",vec![customer.into(),operator.into()]).await?.iter().map(|row|Ok(Person{id:field(row,"id")?,name:field(row,"name")?,email:field(row,"email")?})).collect::<Result<Vec<_>>>()?;
    let grants=rows(tx,"SELECT mailbox_id,subject,enabled,revision FROM webdock_mail.mailbox_grant WHERE customer_id=$1 ORDER BY mailbox_id,subject LIMIT 256",vec![customer.into()]).await?.iter().map(|row|Ok(Grant{mailbox_id:field(row,"mailbox_id")?,subject:field(row,"subject")?,enabled:field(row,"enabled")?,revision:field::<i64>(row,"revision")?.to_string()})).collect::<Result<Vec<_>>>()?;
    Ok((domains, people, grants))
}
