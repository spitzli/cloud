use super::*;
use sea_orm::TransactionTrait;
use webdock_browser_session::revalidate_stored_authority;
pub(super) struct Access {
    auth: Option<DatabaseTransaction>,
    pub expires_at: DateTime<Utc>,
}
impl Access {
    pub async fn commit(self) -> Result<()> {
        if let Some(tx) = self.auth {
            tx.commit().await.map_err(|_| Error::Unavailable)?;
        }
        Ok(())
    }
}
impl IncomingAgent {
    pub(super) async fn configuration_authority(
        &self,
        tx: &DatabaseTransaction,
        record: &ConfigurationRecord,
    ) -> Result<Option<Access>> {
        if !record.fence.module_available
            || (record.mode == ConfigurationMode::Apply
                && (!record.fence.customer_active || !record.fence.mail_enabled))
        {
            return Ok(None);
        }
        if field::<String>(&record.row, "provenance_kind")? == "restriction" {
            let valid=one(tx,"SELECT 1 FROM webdock_mail.mailbox_grant WHERE customer_id=$1 AND mailbox_id=$2 AND subject=$3 AND NOT enabled AND revision=$4 AND member_id IS NULL AND clerk_grant IS NULL",vec![record.customer.clone().into(),field::<String>(&record.row,"restricted_mailbox_id")?.into(),field::<String>(&record.row,"restricted_subject")?.into(),field::<i64>(&record.row,"restricted_revision")?.into()]).await?.is_some();
            return Ok(valid.then(|| Access {
                auth: None,
                expires_at: Utc::now() + chrono::Duration::seconds(120),
            }));
        }
        let stored: StoredAuthority = decode(&record.row, "native_authority")?;
        if stored.read_only() {
            return Ok(None);
        }
        if matches!(stored, StoredAuthority::FirstPartyBrowser { .. })
            && self.incoming.dependencies.browser.is_none()
        {
            return Err(Error::Unavailable);
        }
        let auth = self
            .incoming
            .dependencies
            .auth
            .begin()
            .await
            .map_err(|_| Error::Unavailable)?;
        one(&auth,"SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",vec![]).await?;
        let principal = match revalidate_stored_authority(
            &self.incoming.dependencies.identity,
            &auth,
            self.incoming.dependencies.browser.as_deref(),
            &field::<String>(&record.row, "subject")?,
            &field::<String>(&record.row, "session_id")?,
            &stored,
        )
        .await
        {
            Ok(value) => value,
            Err(error) if super::super::authorization::denied(&error) => return Ok(None),
            Err(_) => return Err(Error::Unavailable),
        };
        if principal.in_read_only_preview() {
            return Ok(None);
        }
        if principal.is_acting_customer() {
            let business = self.incoming.signer.begin(&self.incoming.dependencies.business, &principal).await.map_err(|_|Error::Unavailable)?;
            let allowed = one(&business,"SELECT webdock_rls.customer_allowed($1,true) AS allowed",vec![record.customer.clone().into()]).await?.is_some_and(|r|field::<bool>(&r,"allowed").ok()==Some(true));
            if !allowed { return Ok(None); }
        } else if field::<bool>(&record.row,"operator_authority")? {
            if !principal.is_operator(){return Ok(None);}
        } else if one(tx,r#"SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" WHERE t.customer_id=$1 AND m."userId"=$2 AND m.id=$3 AND m.role IN ('owner','admin') FOR SHARE OF m"#,vec![record.customer.clone().into(),principal.user_id.into(),field::<Option<String>>(&record.row,"admission_member_id")?.into()]).await?.is_none(){return Ok(None);}

        Ok(Some(Access {
            auth: Some(auth),
            expires_at: principal.expires_at,
        }))
    }
}
