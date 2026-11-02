use crate::{
    contracts::{id, text, valid_email},
    service::{field, json_rows, local_org, lock_org, operator},
    *,
};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use webdock_identity::Principal;
impl AccessService {
    pub async fn manage_access(
        &self,
        cookie: &str,
        command: AccessCommand,
    ) -> Result<ChangeResult> {
        self.manage_access_as(&self.actor(cookie).await?, command)
            .await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn manage_access_as(
        &self,
        actor: &Principal,
        command: AccessCommand,
    ) -> Result<ChangeResult> {
        let actor = self.current_actor(actor).await?;
        if !actor.is_operator() {
            return Err(Error::Forbidden);
        }
        let action = match &command {
            AccessCommand::CreateOrganization { .. } => "create-organization",
            AccessCommand::LinkWebsite { .. } => "link-website",
            AccessCommand::Invite { .. } => "invite",
            AccessCommand::ChangeAccess { .. } => "change-access",
            AccessCommand::RevokeAccess { .. } => "revoke-access",
            AccessCommand::Suspend { .. } => "suspend",
            AccessCommand::CancelInvite { .. } => "cancel-invite",
        };
        let result = self.operator_change(&actor, command, action).await;
        self.track_failure(&actor, action, None, result).await
    }
    async fn operator_change(
        &self,
        actor: &Principal,
        command: AccessCommand,
        action: &str,
    ) -> Result<ChangeResult> {
        if let AccessCommand::Suspend { user, blocked } = command {
            id(&user)?;
            let tx = self.auth.begin().await?;
            let live = self
                .identity
                .authenticate_session_in(&tx, &actor.user_id, &actor.session_id)
                .await?
                .ok_or(Error::Unauthorized)?;
            if !live.is_operator() {
                return Err(Error::Forbidden);
            }
            if !self.identity.local_authority_in(&tx, &user).await? {
                return Err(Error::ActivationRequired);
            }
            // The retained owner trigger rejects suspension of the last verified customer owner.
            tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET banned=$2,\"banReason\"=CASE WHEN $2 THEN 'Suspended by operator' ELSE NULL END,\"banExpires\"=NULL,\"updatedAt\"=statement_timestamp() WHERE id=$1 AND role='user' RETURNING id",[user.clone().into(),blocked.into()])).await?.ok_or(Error::Forbidden)?;
            if blocked {
                tx.execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "DELETE FROM webdock_auth.session WHERE \"userId\"=$1",
                    [user.clone().into()],
                ))
                .await?;
            }
            self.audit(&tx, actor, action, Some(&user), true).await?;
            tx.commit().await?;
            return Ok(ChangeResult {
                id: Some(user),
                message: "Account status saved.".into(),
                delivery: None,
                link: None,
            });
        }
        // Provisioning can leave an unverified, access-free account if the subsequent transaction fails.
        let invitee = if let AccessCommand::Invite { email, name, .. } = &command {
            Some(
                self.provision_invitee(&valid_email(email)?, &text(name, 160)?)
                    .await?,
            )
        } else {
            None
        };
        let tx = self.domain(actor).await?;
        operator(&tx).await?;
        let mut queued = false;
        let mut invited = None;
        let target = match command {
            AccessCommand::CreateOrganization { name } => {
                let org = self.next_id().await?;
                tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.organization(id,name,slug,\"createdAt\") VALUES($1,$2,$3,statement_timestamp())",[org.clone().into(),text(&name,160)?.into(),format!("customer-{org}").into()])).await?;
                org
            }
            AccessCommand::LinkWebsite {
                binding,
                organization,
            } => {
                lock_org(&tx, &organization).await?;
                if !local_org(&tx, &organization).await? {
                    return Err(Error::ActivationRequired);
                }
                let site = self.website(&tx, &binding).await?;
                if site["organization_id"]
                    .as_str()
                    .is_some_and(|v| v != organization)
                {
                    return Err(Error::Conflict);
                }
                tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.app_binding SET organization_id=$2 WHERE id=$1 AND (organization_id IS NULL OR organization_id=$2) RETURNING id",[binding.clone().into(),organization.into()])).await?.ok_or(Error::Conflict)?;
                binding
            }
            AccessCommand::Invite {
                binding,
                email,
                role,
                ..
            } => {
                let site = self.website(&tx, &binding).await?;
                let org = field(&site, "organization_id")?;
                lock_org(&tx, org).await?;
                if !local_org(&tx, org).await? {
                    return Err(Error::ActivationRequired);
                }
                self.active_organization(&tx, org).await?;
                let user = invitee.as_deref().ok_or(Error::InvalidInput)?;
                let email = valid_email(&email)?;
                let grant = self.next_id().await?;
                tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.project_grant(id,user_id,binding_id,organization_id,role,enabled) VALUES($1,$2,$3,$4,$5,true) ON CONFLICT(user_id,binding_id) DO UPDATE SET role=excluded.role,enabled=true,organization_id=excluded.organization_id",[grant.into(),user.into(),binding.clone().into(),org.into(),role.as_str().into()])).await?;
                if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.member WHERE \"userId\"=$1 AND \"organizationId\"=$2",[user.into(),org.into()])).await?.is_some(){
                    self.queue_delivery(&tx,actor,org,user,&email,None,Some(&binding),1).await?;
                }else if let Some(row)=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.invitation WHERE \"organizationId\"=$1 AND lower(email)=lower($2) AND status='pending'",[org.into(),email.clone().into()])).await?{
                    invited=Some(self.resend_invitation(&tx,actor,org,&row.try_get::<String>("","id")?).await?.0);
                }else{invited=Some(self.create_invitation(&tx,actor,org,user,&email,MemberRole::Member).await?);}
                queued = true;
                user.to_owned()
            }
            AccessCommand::ChangeAccess { grant, role } => {
                self.grant_organization(&tx, &grant).await?;
                tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.project_grant g SET role=$2,enabled=true FROM webdock_auth.\"user\" u WHERE g.id=$1 AND u.id=g.user_id AND u.role='user' AND NOT coalesce(u.banned,false) AND webdock_rls.local_identity_authority(u.id) RETURNING g.id",[grant.clone().into(),role.as_str().into()])).await?.ok_or(Error::Forbidden)?;
                grant
            }
            AccessCommand::RevokeAccess { grant } => {
                self.grant_organization(&tx, &grant).await?;
                tx.query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE webdock_auth.project_grant SET enabled=false WHERE id=$1 RETURNING id",
                    [grant.clone().into()],
                ))
                .await?
                .ok_or(Error::NotFound)?;
                grant
            }
            AccessCommand::CancelInvite { id: invitation } => {
                let row = tx
                    .query_one_raw(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT \"organizationId\" AS org FROM webdock_auth.invitation WHERE id=$1",
                        [id(&invitation)?.into()],
                    ))
                    .await?
                    .ok_or(Error::NotFound)?;
                let org: String = row.try_get("", "org")?;
                lock_org(&tx, &org).await?;
                if !local_org(&tx, &org).await? {
                    return Err(Error::ActivationRequired);
                }
                self.cancel_invitation(&tx, &org, &invitation).await?;
                invitation
            }
            AccessCommand::Suspend { .. } => unreachable!(),
        };
        operator(&tx).await?;
        self.audit(&tx, actor, action, Some(&target), false).await?;
        tx.commit().await?;
        let mut delivery = queued.then_some("queued");
        let mut link = None;
        if let (Some(user), Some(invitation)) = (&invitee, &invited) {
            (delivery, link) = self.invitation_link(actor, user, invitation).await?;
        } else if let Some(user) = invitee
            && self.activation_needed(&user).await?
            && self
                .identity
                .queue_account_activation(&user, None)
                .await
                .is_err()
        {
            delivery = Some("invitation-queued-activation-unavailable")
        }
        Ok(ChangeResult {
            link,
            id: Some(target),
            message: "Change saved.".into(),
            delivery,
        })
    }
    async fn active_organization(&self, tx: &DatabaseTransaction, org: &str) -> Result<()> {
        if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE t.organization_id=$1 AND c.status<>'active'",[org.into()])).await?.is_some(){return Err(Error::Forbidden)}
        Ok(())
    }
    async fn grant_organization(&self, tx: &DatabaseTransaction, grant: &str) -> Result<()> {
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT organization_id FROM webdock_auth.project_grant WHERE id=$1",
                [id(grant)?.into()],
            ))
            .await?
            .ok_or(Error::NotFound)?;
        let org: String = row.try_get("", "organization_id")?;
        lock_org(tx, &org).await?;
        if !local_org(tx, &org).await? {
            return Err(Error::ActivationRequired);
        }
        self.active_organization(tx, &org).await
    }
    async fn website(&self, tx: &DatabaseTransaction, binding: &str) -> Result<Value> {
        let rows=json_rows(tx,"SELECT to_jsonb(b)||jsonb_build_object('redirectUris',c.\"redirectUris\") AS data FROM webdock_auth.app_binding b JOIN webdock_auth.\"oauthClient\" c ON c.\"clientId\"=b.client_id WHERE b.id=$1 AND b.enabled AND NOT coalesce(c.disabled,false) FOR UPDATE OF b",vec![id(binding)?.into()]).await?;
        let site = rows.into_iter().next().ok_or(Error::NotFound)?;
        let uris: Vec<String> = serde_json::from_value(site["redirectUris"].clone())
            .map_err(|_| Error::InvalidInput)?;
        website_url(
            &uris,
            &self.config.console_origins,
            self.config.allow_local_http,
        )
        .ok_or(Error::Forbidden)?;
        Ok(site)
    }
    pub async fn current_preview(&self, cookie: &str) -> Result<Value> {
        self.current_preview_as(&self.actor(cookie).await?).await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn current_preview_as(&self, actor: &Principal) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        let result = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.access_preview() AS data",
            ))
            .await?
            .ok_or(Error::NotFound)?
            .try_get::<Option<Value>>("", "data")?
            .unwrap_or(Value::Null);
        tx.commit().await?;
        Ok(result)
    }
    pub async fn start_preview(&self, cookie: &str, customer: &str) -> Result<Value> {
        self.start_preview_as(&self.actor(cookie).await?, customer)
            .await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn start_preview_as(&self, actor: &Principal, customer: &str) -> Result<Value> {
        self.start_customer_scope_as(actor, customer, PreviewMode::ReadOnly)
            .await
    }
    /// Explicit owner acting; the old preview entry points remain read-only.
    pub async fn start_customer_scope_as(
        &self,
        actor: &Principal,
        customer: &str,
        mode: PreviewMode,
    ) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        if !actor.is_operator() {
            return Err(Error::Forbidden);
        }
        let (tx, auth, actor) = self.fenced_session_domain(&actor).await?;
        if !actor.is_operator() {
            return Err(Error::Forbidden);
        }
        operator(&tx).await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT c.name,t.organization_id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 AND c.status='active' FOR SHARE OF c",[id(customer)?.into()])).await?.ok_or(Error::NotFound)?;
        let org: String = row.try_get("", "organization_id")?;
        if !local_org(&tx, &org).await? {
            return Err(Error::ActivationRequired);
        }
        tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES($1,$2,$3,$4,$5) ON CONFLICT(session_id) DO NOTHING RETURNING session_id",[actor.session_id.clone().into(),actor.user_id.clone().into(),customer.into(),org.into(),mode.as_str().into()])).await?.ok_or(Error::Conflict)?;
        self.audit(
            &tx,
            &actor,
            if mode == PreviewMode::Owner {
                "tenant-acting-start"
            } else {
                "tenant-preview-start"
            },
            Some(customer),
            false,
        )
        .await?;
        tx.commit().await?;
        auth.commit().await?;
        self.current_preview_as(&actor).await
    }
    pub async fn exit_preview(&self, cookie: &str) -> Result<Value> {
        self.exit_preview_as(&self.actor(cookie).await?).await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn exit_preview_as(&self, actor: &Principal) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        if actor.role() != "operator" {
            return Err(Error::Forbidden);
        }
        // Root is rechecked even while operator() is intentionally false during a preview.
        let (tx, auth, actor) = self.fenced_session_domain(&actor).await?;
        if actor.role() != "operator" {
            return Err(Error::Forbidden);
        }
        let root: bool = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT coalesce((webdock_rls.actor()->>'root')::boolean,false) AS allowed",
            ))
            .await?
            .ok_or(Error::Forbidden)?
            .try_get("", "allowed")?;
        if !root {
            return Err(Error::Forbidden);
        }
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1 AND actor_id=$2 RETURNING customer_id",[actor.session_id.clone().into(),actor.user_id.clone().into()])).await?;
        let customer: Option<String> = row.map(|r| r.try_get("", "customer_id")).transpose()?;
        if customer.is_some() {
            self.audit(
                &tx,
                &actor,
                "tenant-preview-exit",
                customer.as_deref(),
                false,
            )
            .await?;
        }
        tx.commit().await?;
        auth.commit().await?;
        Ok(json!({"customerID":customer,"message":"Customer preview ended."}))
    }
}
