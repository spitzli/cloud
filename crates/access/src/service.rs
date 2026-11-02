use crate::{
    contracts::{id, text, valid_email},
    *,
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait, Value as SqlValue,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use webdock_identity::{
    Delivery, DeliveryKind, DeliveryRequest, IdentityService, Principal, RlsSigner, SecretKeys,
    UnavailableDelivery,
};
use webdock_persistence::IdAllocator;

const PROFILE: &str = "c.id,c.name,c.status,c.customer_type,c.first_name,c.last_name,c.company_name,c.contact_name,c.contact_email,c.phone,c.address_line1,c.address_line2,c.postal_code,c.city,c.region,c.country";
pub struct AccessService {
    pub(crate) business: DatabaseConnection,
    pub(crate) auth: DatabaseConnection,
    pub(crate) identity: Arc<IdentityService>,
    pub(crate) allocator: Arc<IdAllocator>,
    pub(crate) signer: RlsSigner,
    pub(crate) keys: Arc<SecretKeys>,
    pub(crate) config: AccessConfig,
    delivery: Arc<dyn Delivery>,
}
impl AccessService {
    pub fn new(
        business: DatabaseConnection,
        auth: DatabaseConnection,
        identity: Arc<IdentityService>,
        allocator: Arc<IdAllocator>,
        signer: RlsSigner,
        keys: Arc<SecretKeys>,
        config: AccessConfig,
    ) -> Result<Self> {
        let origin = &config.auth_origin;
        if (!config.allow_local_http && origin.scheme() != "https")
            || webdock_identity::validate_account_origin(origin).is_err()
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            business,
            auth,
            identity,
            allocator,
            signer,
            keys,
            config,
            delivery: Arc::new(UnavailableDelivery),
        })
    }
    pub fn with_delivery(mut self, delivery: Arc<dyn Delivery>) -> Self {
        self.delivery = delivery;
        self
    }
    pub(crate) async fn next_id(&self) -> Result<String> {
        self.allocator
            .allocate()
            .await
            .map(|id| id.to_string())
            .map_err(|_| Error::Allocation)
    }
    pub(crate) async fn current_actor(&self, principal: &Principal) -> Result<Principal> {
        let tx = self.auth.begin().await?;
        let current = self.current_actor_in(&tx, principal).await?;
        tx.commit().await?;
        Ok(current)
    }
    pub(crate) async fn current_actor_in(
        &self,
        tx: &DatabaseTransaction,
        principal: &Principal,
    ) -> Result<Principal> {
        let current = self
            .identity
            .authenticate_session_in(tx, &principal.user_id, &principal.session_id)
            .await?
            .ok_or(Error::Unauthorized)?;
        if !current.can_access()
            || principal.customer_scope().is_some()
                && principal.customer_scope() != current.customer_scope()
        {
            return Err(Error::Unauthorized);
        }
        Ok(current)
    }
    pub(crate) async fn fenced_session_domain(
        &self,
        identified: &Principal,
    ) -> Result<(DatabaseTransaction, DatabaseTransaction, Principal)> {
        let tx = self.signer.begin(&self.business, identified).await?;
        lock_session_context(&tx, &identified.session_id).await?;
        let auth = self.auth.begin().await?;
        let current = self.current_actor_in(&auth, identified).await?;
        Ok((tx, auth, current))
    }
    pub(crate) async fn actor(&self, cookie: &str) -> Result<Principal> {
        let actor = self
            .identity
            .authenticate_cookie(cookie)
            .await?
            .ok_or(Error::Unauthorized)?;
        if !actor.can_access() {
            return Err(Error::Unauthorized);
        }
        Ok(actor)
    }
    pub(crate) async fn domain(&self, actor: &Principal) -> Result<DatabaseTransaction> {
        let tx = self.signer.begin(&self.business, actor).await?;
        live(&tx).await?;
        if actor.role() == "operator" && actor.in_tenant_preview() {
            let expired=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.studio_tenant_preview SET expired_at=statement_timestamp() WHERE session_id=$1 AND actor_id=$2 AND expired_at IS NULL AND webdock_rls.access_preview()->>'status'='expired' RETURNING customer_id",[actor.session_id.clone().into(),actor.user_id.clone().into()])).await?;
            if let Some(expired) = expired {
                self.audit(
                    &tx,
                    actor,
                    "tenant-preview-expired",
                    Some(&expired.try_get::<String>("", "customer_id")?),
                    false,
                )
                .await?;
            }
        }
        Ok(tx)
    }
    pub async fn require_operator(&self, cookie: &str) -> Result<Principal> {
        self.require_operator_as(&self.actor(cookie).await?).await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn require_operator_as(&self, actor: &Principal) -> Result<Principal> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        operator(&tx).await?;
        tx.commit().await?;
        Ok(actor)
    }
    pub async fn list_tenants(&self, cookie: &str) -> Result<Vec<Value>> {
        self.list_tenants_as(&self.actor(cookie).await?).await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn list_tenants_as(&self, actor: &Principal) -> Result<Vec<Value>> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        let rows=json_rows(&tx,"SELECT jsonb_build_object('id',c.id,'name',c.name,'status',c.status,'role',CASE WHEN $3 THEN coalesce(webdock_rls.membership_role(t.organization_id),'admin') ELSE m.role END,'localAuthority',webdock_rls.local_organization_authority(t.organization_id)) AS data FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id LEFT JOIN webdock_auth.member m ON m.\"organizationId\"=t.organization_id AND m.\"userId\"=$1 WHERE $2 OR $3 OR (c.status='active' AND m.id IS NOT NULL AND webdock_rls.local_organization_authority(t.organization_id)) ORDER BY c.name,c.id",vec![actor.user_id.clone().into(),actor.is_operator().into(),actor.in_tenant_preview().into()]).await?;
        tx.commit().await?;
        Ok(rows)
    }
    pub async fn list_invitations(&self, cookie: &str) -> Result<Vec<Value>> {
        self.list_invitations_as(&self.actor(cookie).await?).await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn list_invitations_as(&self, actor: &Principal) -> Result<Vec<Value>> {
        let actor = self.current_actor(actor).await?;
        if actor.role() != "user" || actor.in_tenant_preview() {
            return Ok(vec![]);
        }
        let tx = self.auth.begin().await?;
        self.identity
            .authenticate_session_in(&tx, &actor.user_id, &actor.session_id)
            .await?
            .ok_or(Error::Unauthorized)?;
        let rows=json_rows(&tx,"SELECT jsonb_build_object('id',i.id,'role',i.role,'name',o.name) AS data FROM webdock_auth.invitation i JOIN webdock_auth.organization o ON o.id=i.\"organizationId\" JOIN webdock_auth.tenant_customer t ON t.organization_id=o.id JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE lower(i.email)=lower($1) AND i.status='pending' AND i.\"expiresAt\">statement_timestamp() AND c.status='active' AND webdock_rls.local_organization_authority(o.id) ORDER BY i.\"createdAt\" DESC,i.id",vec![actor.email.into()]).await?;
        tx.commit().await?;
        Ok(rows)
    }
    pub async fn get_tenant(&self, cookie: &str, customer: &str) -> Result<Value> {
        self.get_tenant_as(&self.actor(cookie).await?, customer)
            .await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn get_tenant_as(&self, actor: &Principal, customer: &str) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        let mut tenant = tenant(&tx, &actor, customer).await?;
        if actor.in_tenant_preview() {
            tenant["member_role"] = json!(if actor.is_acting_customer() {
                "owner"
            } else {
                "admin"
            });
        }
        let org = field(&tenant, "organization_id")?;
        let local = local_org(&tx, org).await?;
        let can_manage = local
            && !actor.in_read_only_preview()
            && tenant["status"] == "active"
            && (actor.is_operator()
                || ["owner", "admin"].contains(&tenant["member_role"].as_str().unwrap_or("")));
        let members = if can_manage {
            json_rows(&tx,"SELECT jsonb_build_object('id',m.id,'role',m.role,'name',u.name,'email',u.email,'banned',u.banned,'emailVerified',u.\"emailVerified\",'mustChangePassword',u.\"mustChangePassword\",'localAuthority',webdock_rls.local_identity_authority(u.id)) AS data FROM webdock_auth.member m JOIN webdock_auth.\"user\" u ON u.id=m.\"userId\" WHERE m.\"organizationId\"=$1 AND u.role='user' ORDER BY u.name,m.id",vec![org.into()]).await?
        } else {
            vec![]
        };
        // Operators see every event; customer administrators only what row security allows them.
        let activity = if can_manage {
            json_rows(&tx,"SELECT jsonb_build_object('id',e.id,'action',e.action,'outcome',e.outcome,'createdAt',e.created_at,'actorName',u.name,'actorEmail',u.email) AS data FROM webdock_auth.access_event e LEFT JOIN webdock_auth.\"user\" u ON u.id=e.actor_id WHERE e.target_id=$1 OR e.target_id IN (SELECT id FROM webdock_auth.invitation WHERE \"organizationId\"=$2) ORDER BY e.created_at DESC,e.id DESC LIMIT 50",vec![customer.into(),org.into()]).await?
        } else {
            vec![]
        };
        let sites = self.sites_in(&tx, &actor, Some(org)).await?;
        let org = org.to_owned();
        tx.commit().await?;
        // Delivery and account state live in identity tables that only the auth connection reads;
        // the domain transaction above has already established that this actor may manage the tenant.
        let invitations = if can_manage {
            json_rows(&self.auth, INVITATION_PROGRESS, vec![org.into()]).await?
        } else {
            vec![]
        };
        Ok(
            json!({"tenant":tenant,"canManage":can_manage,"operator":actor.is_operator(),"localAuthority":local,"members":members,"invitations":invitations,"activity":activity,"sites":sites}),
        )
    }
    pub async fn account_sites(&self, cookie: &str) -> Result<Vec<Value>> {
        self.account_sites_as(&self.actor(cookie).await?).await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn account_sites_as(&self, actor: &Principal) -> Result<Vec<Value>> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        let sites = self.sites_in(&tx, &actor, None).await?;
        tx.commit().await?;
        Ok(sites)
    }
    async fn sites_in(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        org: Option<&str>,
    ) -> Result<Vec<Value>> {
        let rows=json_rows(tx,"SELECT jsonb_build_object('id',b.id,'name',b.label,'organization_id',b.organization_id,'redirectUris',c.\"redirectUris\",'customerID',t.customer_id,'customerName',customer.name,'customerStatus',customer.status) AS data FROM webdock_auth.app_binding b JOIN webdock_auth.\"oauthClient\" c ON c.\"clientId\"=b.client_id LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=b.organization_id LEFT JOIN webdock_admin.customers customer ON customer.id=t.customer_id WHERE b.enabled AND NOT coalesce(c.disabled,false) AND ($1::text IS NULL OR b.organization_id=$1) ORDER BY b.label,b.id LIMIT 200",vec![org.map(str::to_owned).into()]).await?;
        let mut sites = vec![];
        for row in rows {
            let redirects = strings(&row["redirectUris"])?;
            let Some(url) = website_url(
                &redirects,
                &self.config.console_origins,
                self.config.allow_local_http,
            ) else {
                continue;
            };
            let Some(organization) = row["organization_id"].as_str() else {
                continue;
            };
            if !local_org(tx, organization).await? {
                continue;
            }
            let role = if actor.is_operator() {
                Some("operator".to_owned())
            } else if actor.in_tenant_preview() {
                Some("admin".to_owned())
            } else {
                let flags=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT coalesce((settings->>'cmsEnabled')::boolean,true) AS enabled FROM webdock_auth.platform_settings WHERE id=true")).await?;
                if flags.is_some_and(|r| r.try_get::<bool>("", "enabled").ok() == Some(false)) {
                    continue;
                }
                let billing: bool = self
                    .auth
                    .query_one_raw(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT webdock_rls.billing_cms_allowed($1) AS allowed",
                        [field(&row, "id")?.into()],
                    ))
                    .await?
                    .ok_or(Error::Forbidden)?
                    .try_get("", "allowed")?;
                if !billing {
                    continue;
                }
                let allowed=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT CASE WHEN c.status='active' AND m.role IN ('owner','admin') THEN 'admin' ELSE g.role END AS role FROM webdock_auth.member m LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" LEFT JOIN webdock_admin.customers c ON c.id=t.customer_id LEFT JOIN webdock_auth.project_grant g ON g.user_id=m.\"userId\" AND g.binding_id=$2 WHERE m.\"userId\"=$1 AND m.\"organizationId\"=$3 AND (t.customer_id IS NULL OR c.status='active') AND (g.id IS NULL OR (g.enabled AND g.organization_id=$3)) AND ((c.status='active' AND m.role IN ('owner','admin')) OR g.enabled) LIMIT 1",[actor.user_id.clone().into(),field(&row,"id")?.into(),organization.into()])).await?;
                allowed
                    .map(|r| r.try_get::<Option<String>>("", "role"))
                    .transpose()?
                    .flatten()
            };
            if let Some(role) = role {
                let mut site = json!({"id":row["id"],"name":row["name"],"url":url,"role":role});
                if !row["customerID"].is_null() {
                    site["customerID"] = row["customerID"].clone();
                    site["customerName"] = row["customerName"].clone();
                }
                sites.push(site);
            }
        }
        Ok(sites)
    }
    pub async fn list_access(&self, cookie: &str, search: &str) -> Result<Value> {
        self.list_access_as(&self.actor(cookie).await?, search)
            .await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn list_access_as(&self, actor: &Principal, search: &str) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        let tx = self.domain(&actor).await?;
        operator(&tx).await?;
        let term = search.chars().take(160).collect::<String>();
        let users=search_rows(&tx,"SELECT jsonb_build_object('id',id,'name',name,'email',email,'role',role,'banned',banned,'emailVerified',\"emailVerified\",'mustChangePassword',\"mustChangePassword\",'localAuthority',webdock_rls.local_identity_authority(id)) AS data FROM webdock_auth.\"user\" ORDER BY \"createdAt\" DESC,id",&term,&["name","email"],100).await?;
        let grants=search_rows(&tx,"SELECT jsonb_build_object('id',g.id,'role',g.role,'enabled',g.enabled,'name',u.name,'email',u.email,'label',b.label) AS data FROM webdock_auth.project_grant g JOIN webdock_auth.\"user\" u ON u.id=g.user_id JOIN webdock_auth.app_binding b ON b.id=g.binding_id ORDER BY u.email,b.label,g.id",&term,&["name","email"],200).await?;
        let invitations=search_rows(&tx,"SELECT jsonb_build_object('id',i.id,'email',i.email,'status',i.status,'expiresAt',i.\"expiresAt\",'name',o.name) AS data FROM webdock_auth.invitation i JOIN webdock_auth.organization o ON o.id=i.\"organizationId\" ORDER BY i.\"createdAt\" DESC,i.id",&term,&["email"],100).await?;
        let organizations=json_rows(&tx,"SELECT jsonb_build_object('id',id,'name',name) AS data FROM webdock_auth.organization ORDER BY name,id LIMIT 200",vec![]).await?;
        let bindings=json_rows(&tx,"SELECT jsonb_build_object('id',b.id,'label',b.label,'organization_id',b.organization_id,'redirectUris',c.\"redirectUris\") AS data FROM webdock_auth.app_binding b JOIN webdock_auth.\"oauthClient\" c ON c.\"clientId\"=b.client_id WHERE b.enabled AND NOT coalesce(c.disabled,false) ORDER BY b.label,b.id LIMIT 200",vec![]).await?;
        let websites = bindings
            .into_iter()
            .filter(|b| {
                strings(&b["redirectUris"])
                    .ok()
                    .and_then(|r| {
                        website_url(
                            &r,
                            &self.config.console_origins,
                            self.config.allow_local_http,
                        )
                    })
                    .is_some()
            })
            .collect::<Vec<_>>();
        let events=json_rows(&tx,"SELECT to_jsonb(e)||jsonb_build_object('actor',u.email) AS data FROM webdock_auth.access_event e LEFT JOIN webdock_auth.\"user\" u ON u.id=e.actor_id ORDER BY e.created_at DESC,e.id LIMIT 20",vec![]).await?;
        tx.commit().await?;
        Ok(
            json!({"users":users,"organizations":organizations,"websites":websites,"grants":grants,"invitations":invitations,"events":events}),
        )
    }
    pub async fn manage_tenant(
        &self,
        cookie: &str,
        customer: &str,
        command: TenantCommand,
    ) -> Result<ChangeResult> {
        self.manage_tenant_as(&self.actor(cookie).await?, customer, command)
            .await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn manage_tenant_as(
        &self,
        actor: &Principal,
        customer: &str,
        command: TenantCommand,
    ) -> Result<ChangeResult> {
        let actor = self.current_actor(actor).await?;
        let action = match &command {
            TenantCommand::Profile { .. } => "tenant-profile",
            TenantCommand::Invite { .. } => "tenant-invite",
            TenantCommand::Resend { .. } => "tenant-resend",
            TenantCommand::Cancel { .. } => "tenant-cancel",
            TenantCommand::Role { .. } => "tenant-role",
            TenantCommand::Remove { .. } => "tenant-remove",
        };
        let result = self.tenant_change(&actor, customer, command).await;
        self.track_failure(&actor, action, Some(customer), result)
            .await
    }
    async fn tenant_change(
        &self,
        actor: &Principal,
        customer: &str,
        command: TenantCommand,
    ) -> Result<ChangeResult> {
        if actor.in_read_only_preview() {
            return Err(Error::Forbidden);
        }
        let preliminary = self.domain(actor).await?;
        let found = tenant(&preliminary, actor, customer).await?;
        manageable(actor, &found)?;
        let org = field(&found, "organization_id")?.to_owned();
        if !local_org(&preliminary, &org).await? {
            return Err(Error::ActivationRequired);
        }
        preliminary.commit().await?;
        let invitee = if let TenantCommand::Invite { email, name, role } = &command {
            // Ownership is given on purpose: operators and owners may invite owners, admins may not.
            if *role == MemberRole::Owner && !actor.is_operator() && found["member_role"] != "owner"
            {
                return Err(Error::Forbidden);
            }
            Some(
                self.provision_invitee(&valid_email(email)?, &text(name, 160)?)
                    .await?,
            )
        } else {
            None
        };
        let tx = self.domain(actor).await?;
        lock_org(&tx, &org).await?;
        let current = tenant(&tx, actor, customer).await?;
        manageable(actor, &current)?;
        if !local_org(&tx, &org).await? {
            return Err(Error::ActivationRequired);
        }
        let mut target = None;
        let mut queued = false;
        let mut recipient = invitee.clone();
        let action;
        match command {
            TenantCommand::Profile { profile } => {
                action = "tenant-profile";
                let p = profile.validate()?;
                tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_admin.customers SET customer_type=$2::webdock_admin.enum_customers_customer_type,name=$3,first_name=$4,last_name=$5,company_name=$6,contact_name=$7,contact_email=$8,phone=$9,address_line1=$10,address_line2=$11,postal_code=$12,city=$13,region=$14,country=$15,updated_at=statement_timestamp() WHERE id=$1",[customer.into(),match p.customer_type{CustomerType::Person=>"person",CustomerType::Company=>"company"}.into(),p.name.into(),p.first_name.into(),p.last_name.into(),p.company_name.into(),p.contact_name.into(),p.contact_email.into(),p.phone.into(),p.address_line1.into(),p.address_line2.into(),p.postal_code.into(),p.city.into(),p.region.into(),p.country.into()])).await?;
            }
            TenantCommand::Invite { email, role, .. } => {
                action = "tenant-invite";
                let user = invitee.as_deref().ok_or(Error::InvalidInput)?;
                if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.member WHERE \"organizationId\"=$1 AND \"userId\"=$2",[org.clone().into(),user.into()])).await?.is_some(){return Err(Error::Conflict)}
                let invitation = self
                    .create_invitation(&tx, actor, &org, user, &valid_email(&email)?, role)
                    .await?;
                target = Some(invitation);
                queued = true;
            }
            TenantCommand::Resend { invitation } => {
                action = "tenant-resend";
                let (invitation, user) = self
                    .resend_invitation(&tx, actor, &org, &invitation)
                    .await?;
                target = Some(invitation);
                recipient = Some(user);
                queued = true;
            }
            TenantCommand::Cancel { invitation } => {
                action = "tenant-cancel";
                self.cancel_invitation(&tx, &org, &invitation).await?;
                target = Some(invitation);
            }
            TenantCommand::Role { member, role } => {
                action = "tenant-role";
                self.change_member(&tx, actor, &current, &org, &member, Some(role))
                    .await?;
                target = Some(member);
            }
            TenantCommand::Remove { member } => {
                action = "tenant-remove";
                self.change_member(&tx, actor, &current, &org, &member, None)
                    .await?;
                target = Some(member);
            }
        }
        self.audit(&tx, actor, action, Some(customer), false)
            .await?;
        live(&tx).await?;
        tx.commit().await?;
        let mut delivery = queued.then_some("queued");
        let mut link = None;
        if let (true, Some(user), Some(invitation)) = (queued, recipient, target.as_deref()) {
            (delivery, link) = self.invitation_link(actor, &user, invitation).await?;
        }
        Ok(ChangeResult {
            link,
            id: target,
            message: if queued {
                "Invitation saved and queued. Access starts after acceptance."
            } else {
                "Change saved."
            }
            .into(),
            delivery,
        })
    }
    async fn change_member(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        current: &Value,
        org: &str,
        member: &str,
        role: Option<MemberRole>,
    ) -> Result<()> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT m.id,m.role,m.\"userId\" FROM webdock_auth.member m JOIN webdock_auth.\"user\" u ON u.id=m.\"userId\" WHERE m.id=$1 AND m.\"organizationId\"=$2 AND u.role='user' FOR UPDATE OF m",[id(member)?.into(),org.into()])).await?.ok_or(Error::NotFound)?;
        let old: String = row.try_get("", "role")?;
        let user: String = row.try_get("", "userId")?;
        if !actor.is_operator()
            && current["member_role"] != "owner"
            && (old != "member" || role.is_some_and(|r| r != MemberRole::Member))
        {
            return Err(Error::Forbidden);
        }
        if role == Some(MemberRole::Owner) {
            let eligible=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND role='user' AND NOT coalesce(banned,false) AND \"emailVerified\" AND NOT \"mustChangePassword\" AND webdock_rls.local_identity_authority(id) FOR UPDATE",[user.clone().into()])).await?;
            if eligible.is_none() {
                return Err(Error::Forbidden);
            }
        }
        for (protect, roles) in [
            (
                old == "owner" && role != Some(MemberRole::Owner),
                vec!["owner"],
            ),
            (
                ["owner", "admin"].contains(&old.as_str())
                    && (role.is_none() || role == Some(MemberRole::Member)),
                vec!["owner", "admin"],
            ),
        ] {
            if protect {
                let other=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.\"user\" u ON u.id=m.\"userId\" WHERE m.\"organizationId\"=$1 AND m.id<>$2 AND m.role=ANY($3::text[]) AND u.role='user' AND NOT coalesce(u.banned,false) AND u.\"emailVerified\" AND NOT u.\"mustChangePassword\" AND webdock_rls.local_identity_authority(u.id) LIMIT 1",[org.into(),member.into(),roles.into_iter().map(str::to_owned).collect::<Vec<_>>().into()])).await?;
                if other.is_none() {
                    return Err(Error::LastAdministrator);
                }
            }
        }
        if let Some(role) = role {
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_auth.member SET role=$2 WHERE id=$1",
                [member.into(), role.as_str().into()],
            ))
            .await?;
        } else {
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.member WHERE id=$1",
                [member.into()],
            ))
            .await?;
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.project_grant SET enabled=false WHERE user_id=$1 AND organization_id=$2",[user.into(),org.into()])).await?;
        }
        Ok(())
    }
    /// Queues the activation that completes an invitation for a new account, and returns the
    /// copyable link for operators. Customer administrators only ever get the mailed copy.
    pub(crate) async fn invitation_link(
        &self,
        actor: &Principal,
        user: &str,
        invitation: &str,
    ) -> Result<(Option<&'static str>, Option<String>)> {
        if !self.activation_needed(user).await? {
            let mut url = self
                .config
                .auth_origin
                .join("/invitation")
                .map_err(|_| Error::Configuration)?;
            url.query_pairs_mut().append_pair("id", invitation);
            return Ok((Some("queued"), actor.is_operator().then(|| url.into())));
        }
        Ok(
            match self
                .identity
                .queue_account_activation(user, Some(invitation))
                .await
            {
                Ok(Some(url)) => (Some("queued"), actor.is_operator().then_some(url)),
                Ok(None) => (Some("activation-recently-sent"), None),
                Err(_) => (Some("invitation-queued-activation-unavailable"), None),
            },
        )
    }
    pub(crate) async fn activation_needed(&self, user: &str) -> Result<bool> {
        Ok(self.auth.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND NOT \"emailVerified\" AND \"mustChangePassword\"",[user.into()])).await?.is_some())
    }
    pub(crate) async fn provision_invitee(&self, email: &str, name: &str) -> Result<String> {
        let tx = self.auth.begin().await?;
        let rows=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,role,coalesce(banned,false) AS banned FROM webdock_auth.\"user\" WHERE lower(email)=$1",[email.into()])).await?;
        if rows.len() > 1 {
            return Err(Error::Conflict);
        }
        if let Some(row) = rows.first() {
            let id: String = row.try_get("", "id")?;
            if row.try_get::<String>("", "role")? != "user" || row.try_get::<bool>("", "banned")? {
                return Err(Error::Forbidden);
            }
            if !self.identity.local_authority_in(&tx, &id).await? {
                return Err(Error::ActivationRequired);
            }
            tx.commit().await?;
            return Ok(id);
        }
        tx.commit().await?;
        let mut bytes = [0; 32];
        getrandom::getrandom(&mut bytes).map_err(|_| Error::Configuration)?;
        self.identity
            .create_local_account(email, name, &hex::encode(bytes), false)
            .await
            .map_err(Into::into)
    }
    pub(crate) async fn create_invitation(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        org: &str,
        user: &str,
        email: &str,
        role: MemberRole,
    ) -> Result<String> {
        if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.invitation WHERE \"organizationId\"=$1 AND lower(email)=lower($2) AND status='pending'",[org.into(),email.into()])).await?.is_some(){return Err(Error::Conflict)}
        let pending:i64=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT count(*) AS count FROM webdock_auth.invitation WHERE \"organizationId\"=$1 AND status='pending'",[org.into()])).await?.ok_or(Error::Conflict)?.try_get("","count")?;
        if pending >= 100 {
            return Err(Error::Conflict);
        }
        let invitation = self.next_id().await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.invitation(id,\"organizationId\",email,role,status,\"expiresAt\",\"createdAt\",\"inviterId\") VALUES($1,$2,$3,$4,'pending',statement_timestamp()+interval '48 hours',statement_timestamp(),$5)",[invitation.clone().into(),org.into(),email.into(),role.as_str().into(),actor.user_id.clone().into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.access_invitation_state(invitation_id,organization_id,user_id) VALUES($1,$2,$3)",[invitation.clone().into(),org.into(),user.into()])).await?;
        // A new account's activation mail carries the invitation; one message is enough.
        if !self.activation_needed(user).await? {
            self.queue_delivery(tx, actor, org, user, email, Some(&invitation), None, 1)
                .await?;
        }
        Ok(invitation)
    }
    pub(crate) async fn resend_invitation(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        org: &str,
        invitation: &str,
    ) -> Result<(String, String)> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT email,role FROM webdock_auth.invitation WHERE id=$1 AND \"organizationId\"=$2 AND status='pending' FOR UPDATE",[id(invitation)?.into(),org.into()])).await?.ok_or(Error::NotFound)?;
        let email: String = row.try_get("", "email")?;
        let state=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.access_invitation_state SET generation=generation+1 WHERE invitation_id=$1 RETURNING user_id,generation",[invitation.into()])).await?;
        let (user, generation) = if let Some(state) = state {
            (
                state.try_get::<String>("", "user_id")?,
                state.try_get::<i32>("", "generation")?,
            )
        } else {
            let user = self.provision_invitee(&email, "Invited account").await?;
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.access_invitation_state(invitation_id,organization_id,user_id) VALUES($1,$2,$3)",[invitation.into(),org.into(),user.clone().into()])).await?;
            (user, 1)
        };
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.invitation SET \"expiresAt\"=statement_timestamp()+interval '48 hours' WHERE id=$1",[invitation.into()])).await?;
        if !self.activation_needed(&user).await? {
            self.queue_delivery(
                tx,
                actor,
                org,
                &user,
                &email,
                Some(invitation),
                None,
                generation,
            )
            .await?;
        }
        Ok((invitation.into(), user))
    }
    pub(crate) async fn cancel_invitation(
        &self,
        tx: &DatabaseTransaction,
        org: &str,
        invitation: &str,
    ) -> Result<()> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.invitation SET status='canceled' WHERE id=$1 AND \"organizationId\"=$2 AND status='pending' RETURNING email",[id(invitation)?.into(),org.into()])).await?.ok_or(Error::Conflict)?;
        let email: String = row.try_get("", "email")?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.project_grant g SET enabled=false FROM webdock_auth.\"user\" u WHERE u.id=g.user_id AND g.organization_id=$2 AND (lower(u.email)=lower($1) OR u.id IN (SELECT user_id FROM webdock_auth.access_invitation_state WHERE invitation_id=$3))",[email.into(),org.into(),invitation.into()])).await?;
        Ok(())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )] // A private SQL adapter binds the retained outbox columns explicitly.
    pub(crate) async fn queue_delivery(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        org: &str,
        user: &str,
        email: &str,
        invitation: Option<&str>,
        binding: Option<&str>,
        generation: i32,
    ) -> Result<()> {
        let id = self.next_id().await?;
        let mut url = self
            .config
            .auth_origin
            .join(if invitation.is_some() {
                "/invitation"
            } else {
                "/sites"
            })
            .map_err(|_| Error::Configuration)?;
        if let Some(invitation) = invitation {
            url.query_pairs_mut().append_pair("id", invitation);
        }
        let kind = if invitation.is_some() {
            DeliveryKind::Invitation
        } else {
            DeliveryKind::WebsiteAccess
        };
        let payload = self.keys.encrypt(
            &serde_json::to_string(&DeliveryRequest {
                id: id.clone(),
                email: email.into(),
                kind,
                url: url.into(),
            })
            .map_err(|_| Error::InvalidInput)?,
        )?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.access_delivery_job(id,invitation_id,organization_id,user_id,binding_id,actor_id,actor_session_id,generation,kind,payload,admitted_customer_scope,admission_bound) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,webdock_rls.customer_scope(),true)",[id.into(),invitation.map(str::to_owned).into(),org.into(),user.into(),binding.map(str::to_owned).into(),actor.user_id.clone().into(),actor.session_id.clone().into(),generation.into(),kind.as_str().into(),payload.into()])).await?;
        Ok(())
    }
    pub async fn get_invitation(&self, cookie: &str, invitation: &str) -> Result<Value> {
        self.get_invitation_as(&self.actor(cookie).await?, invitation)
            .await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn get_invitation_as(&self, actor: &Principal, invitation: &str) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        if actor.role() != "user" || actor.in_tenant_preview() {
            return Err(Error::Forbidden);
        }
        let tx = self.auth.begin().await?;
        self.identity
            .authenticate_session_in(&tx, &actor.user_id, &actor.session_id)
            .await?
            .ok_or(Error::Unauthorized)?;
        let rows=json_rows(&tx,"SELECT to_jsonb(i)||jsonb_build_object('organizationName',o.name,'organizationSlug',o.slug,'customerID',c.id) AS data FROM webdock_auth.invitation i JOIN webdock_auth.organization o ON o.id=i.\"organizationId\" LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=o.id LEFT JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE i.id=$1 AND lower(i.email)=lower($2) AND (c.id IS NULL OR c.status='active') AND webdock_rls.local_organization_authority(o.id)",vec![id(invitation)?.into(),actor.email.into()]).await?;
        tx.commit().await?;
        rows.into_iter().next().ok_or(Error::NotFound)
    }
    pub async fn respond_invitation(
        &self,
        cookie: &str,
        invitation: &str,
        accept: bool,
    ) -> Result<Value> {
        self.respond_invitation_as(&self.actor(cookie).await?, invitation, accept)
            .await
    }
    /// Trusted backend entry point; revalidates the persisted native principal.
    pub async fn respond_invitation_as(
        &self,
        actor: &Principal,
        invitation: &str,
        accept: bool,
    ) -> Result<Value> {
        let actor = self.current_actor(actor).await?;
        let action = if accept {
            "accept-invitation"
        } else {
            "reject-invitation"
        };
        let result = self.respond_for(&actor, invitation, accept).await;
        self.track_failure(&actor, action, Some(invitation), result)
            .await
    }
    async fn respond_for(
        &self,
        actor: &Principal,
        invitation: &str,
        accept: bool,
    ) -> Result<Value> {
        if actor.role() != "user" || actor.in_tenant_preview() {
            return Err(Error::Forbidden);
        }
        let tx = self.auth.begin().await?;
        let initial = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT \"organizationId\" FROM webdock_auth.invitation WHERE id=$1",
                [id(invitation)?.into()],
            ))
            .await?
            .ok_or(Error::NotFound)?;
        let org: String = initial.try_get("", "organizationId")?;
        lock_org(&tx, &org).await?;
        let current = self
            .identity
            .authenticate_session_in(&tx, &actor.user_id, &actor.session_id)
            .await?
            .ok_or(Error::Unauthorized)?;
        if !current.can_access() || current.role() != "user" || current.in_tenant_preview() {
            return Err(Error::Forbidden);
        }
        if !local_org(&tx, &org).await? {
            return Err(Error::ActivationRequired);
        }
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT i.email,i.role,s.user_id AS target_user FROM webdock_auth.invitation i LEFT JOIN webdock_auth.access_invitation_state s ON s.invitation_id=i.id LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=i.\"organizationId\" LEFT JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE i.id=$1 AND i.status='pending' AND i.\"expiresAt\">statement_timestamp() AND (c.id IS NULL OR c.status='active') FOR UPDATE OF i",[invitation.into()])).await?.ok_or(Error::Conflict)?;
        if row.try_get::<String>("", "email")?.to_lowercase() != current.email.to_lowercase()
            || row
                .try_get::<Option<String>>("", "target_user")?
                .is_some_and(|user| user != current.user_id)
        {
            return Err(Error::Forbidden);
        }
        let role: String = row.try_get("", "role")?;
        if !["owner", "admin", "member"].contains(&role.as_str()) {
            return Err(Error::InvalidInput);
        }
        let mut member = Value::Null;
        if accept {
            let existing=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(m) AS data FROM webdock_auth.member m WHERE \"organizationId\"=$1 AND \"userId\"=$2",[org.clone().into(),current.user_id.clone().into()])).await?;
            if let Some(existing) = existing {
                member = existing.try_get("", "data")?;
            } else {
                let count:i64=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT count(*) AS count FROM webdock_auth.member WHERE \"organizationId\"=$1",[org.clone().into()])).await?.ok_or(Error::Conflict)?.try_get("","count")?;
                if count >= 100 {
                    return Err(Error::Conflict);
                }
                member=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,$4,statement_timestamp()) RETURNING to_jsonb(member) AS data",[self.next_id().await?.into(),org.into(),current.user_id.clone().into(),role.into()])).await?.ok_or(Error::Conflict)?.try_get("","data")?;
            }
        }
        let invitation=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.invitation SET status=$2 WHERE id=$1 RETURNING to_jsonb(invitation) AS data",[invitation.into(),if accept{"accepted"}else{"rejected"}.into()])).await?.ok_or(Error::Conflict)?.try_get::<Value>("","data")?;
        self.audit(
            &tx,
            &current,
            if accept {
                "accept-invitation"
            } else {
                "reject-invitation"
            },
            invitation["id"].as_str(),
            true,
        )
        .await?;
        tx.commit().await?;
        Ok(json!({"invitation":invitation,"member":member}))
    }
    pub async fn dispatch_invitation_jobs(&self, limit: u32) -> Result<DeliveryBatch> {
        if limit == 0 || limit > 100 {
            return Err(Error::InvalidInput);
        }
        let mut batch = DeliveryBatch::default();
        for _ in 0..limit {
            let job=self.auth.query_one_raw(Statement::from_string(DbBackend::Postgres,"UPDATE webdock_auth.access_delivery_job SET attempts=attempts+1,lease_until=statement_timestamp()+interval '60 seconds' WHERE id=(SELECT id FROM webdock_auth.access_delivery_job WHERE state='queued' AND attempts<5 AND next_attempt_at<=statement_timestamp() AND (lease_until IS NULL OR lease_until<=statement_timestamp()) ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING *")).await?;
            let Some(job) = job else { break };
            let id: String = job.try_get("", "id")?;
            let attempt: i32 = job.try_get("", "attempts")?;
            let org: String = job.try_get("", "organization_id")?;
            let tx = self.auth.begin().await?;
            let principal = self
                .identity
                .authenticate_session_in(
                    &tx,
                    &job.try_get::<String>("", "actor_id")?,
                    &job.try_get::<String>("", "actor_session_id")?,
                )
                .await?;
            let admitted_scope: Option<String> = job.try_get("", "admitted_customer_scope")?;
            let mut valid = job.try_get::<bool>("", "admission_bound")?
                && principal.as_ref().is_some_and(|p| {
                    p.can_access()
                        && !p.in_read_only_preview()
                        && p.customer_scope() == admitted_scope.as_deref()
                })
                && local_org(&tx, &org).await?;
            // Hold session and scope locks through delivery, so exiting cannot restore authority
            // between this exact-scope permission check and dispatch.
            let permission = if let Some(principal) = principal.as_ref()
                && valid
            {
                let permission = self.signer.begin(&self.business, principal).await?;
                permission.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                    "SELECT session_id FROM webdock_auth.studio_tenant_preview WHERE session_id=$1 AND actor_id=$2 FOR SHARE",
                    [principal.session_id.clone().into(),principal.user_id.clone().into()])).await?;
                valid = permission.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                    "SELECT coalesce(webdock_rls.organization_allowed($1,true),false) AND webdock_rls.customer_scope() IS NOT DISTINCT FROM $2::text AS allowed",
                    [org.clone().into(),admitted_scope.into()])).await?.ok_or(Error::Forbidden)?.try_get("","allowed")?;
                Some(permission)
            } else {
                None
            };
            let invitation: Option<String> = job.try_get("", "invitation_id")?;
            let recipient: String = job.try_get("", "user_id")?;
            valid&=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND role='user' AND NOT coalesce(banned,false) AND webdock_rls.local_identity_authority(id)",[recipient.clone().into()])).await?.is_some();
            valid&=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE t.organization_id=$1 AND c.status<>'active'",[org.clone().into()])).await?.is_none();
            if let Some(invitation) = invitation {
                valid&=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT i.id FROM webdock_auth.invitation i JOIN webdock_auth.access_invitation_state s ON s.invitation_id=i.id WHERE i.id=$1 AND i.status='pending' AND i.\"expiresAt\">statement_timestamp() AND s.user_id=$2 AND s.generation=$3",[invitation.into(),recipient.into(),job.try_get::<i32>("","generation")?.into()])).await?.is_some();
            } else {
                valid&=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.member WHERE \"organizationId\"=$1 AND \"userId\"=$2",[org.clone().into(),recipient.clone().into()])).await?.is_some();
                valid &= tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT g.id FROM webdock_auth.project_grant g JOIN webdock_auth.app_binding b ON b.id=g.binding_id WHERE g.user_id=$1 AND g.binding_id=$2 AND g.organization_id=$3 AND b.organization_id=$3 AND g.enabled AND b.enabled",[recipient.into(),job.try_get::<Option<String>>("","binding_id")?.into(),org.into()])).await?.is_some();
            }
            let payload: DeliveryRequest =
                serde_json::from_str(&self.keys.decrypt(&job.try_get::<String>("", "payload")?)?)
                    .map_err(|_| Error::InvalidInput)?;
            let delivered = valid
                && matches!(
                    tokio::time::timeout(Duration::from_secs(10), self.delivery.deliver(&payload))
                        .await,
                    Ok(Ok(()))
                );
            if let Some(permission) = permission {
                permission.commit().await?;
            }
            tx.commit().await?;
            let state = if !valid {
                "canceled"
            } else if delivered {
                "delivered"
            } else {
                "queued"
            };
            self.auth.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.access_delivery_job SET state=$3,lease_until=NULL,next_attempt_at=statement_timestamp()+make_interval(secs=>30*attempts) WHERE id=$1 AND attempts=$2",[id.into(),attempt.into(),state.into()])).await?;
            if !valid {
                batch.canceled += 1
            } else if delivered {
                batch.delivered += 1
            } else {
                batch.failed += 1
            }
        }
        Ok(batch)
    }
    pub(crate) async fn audit(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        action: &str,
        target: Option<&str>,
        bootstrap: bool,
    ) -> Result<()> {
        let id = self.next_id().await?;
        if bootstrap {
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_auth.append_access_event($1,$2,$3,$4,'succeeded')",
                [
                    id.into(),
                    actor.user_id.clone().into(),
                    action.into(),
                    target.map(str::to_owned).into(),
                ],
            ))
            .await?;
        } else {
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.access_event(id,actor_id,action,target_id,outcome) VALUES($1,$2,$3,$4,'succeeded')",[id.into(),actor.user_id.clone().into(),action.into(),target.map(str::to_owned).into()])).await?;
        }
        Ok(())
    }
    pub(crate) async fn track_failure<T>(
        &self,
        actor: &Principal,
        action: &str,
        target: Option<&str>,
        result: Result<T>,
    ) -> Result<T> {
        if result.is_err() {
            self.auth
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT webdock_auth.append_access_event($1,$2,$3,$4,'failed')",
                    [
                        self.next_id().await?.into(),
                        actor.user_id.clone().into(),
                        action.into(),
                        target.map(str::to_owned).into(),
                    ],
                ))
                .await?;
        }
        result
    }
}
/// Each invitation with the step its invitee has reached. Mail state comes from the invitation
/// mail, or for new accounts from the activation mail that carries the invitation.
const INVITATION_PROGRESS: &str = "SELECT jsonb_build_object('id',i.id,'email',i.email,'role',i.role,'status',i.status,'createdAt',i.\"createdAt\",'expiresAt',i.\"expiresAt\",'expired',i.status='pending' AND i.\"expiresAt\"<=statement_timestamp(),'name',u.name,'progress',jsonb_build_object('invited',i.\"createdAt\",'emailDelivered',CASE WHEN bool_or(m.delivered) THEN 'delivered' WHEN bool_or(m.waiting) THEN 'queued' WHEN count(m.delivered)>0 THEN 'failed' END,'accountReady',coalesce(NOT u.\"mustChangePassword\",false),'joinedAt',max(j.\"createdAt\"))) AS data FROM (SELECT * FROM webdock_auth.invitation WHERE \"organizationId\"=$1 ORDER BY \"createdAt\" DESC,id LIMIT 100) i LEFT JOIN webdock_auth.access_invitation_state s ON s.invitation_id=i.id LEFT JOIN webdock_auth.\"user\" u ON u.id=s.user_id LEFT JOIN LATERAL (SELECT state='delivered' AS delivered,state='queued' AND attempts<5 AS waiting FROM webdock_auth.access_delivery_job WHERE invitation_id=i.id AND state<>'canceled' UNION ALL SELECT d.delivered_at IS NOT NULL,d.delivered_at IS NULL AND d.attempts<5 FROM webdock_auth.identity_delivery_job d JOIN webdock_auth.identity_recovery_token r ON r.token_hash=d.token_hash WHERE r.user_id=s.user_id AND r.kind='activation') m ON true LEFT JOIN webdock_auth.member j ON i.status='accepted' AND j.\"organizationId\"=i.\"organizationId\" AND j.\"userId\"=s.user_id GROUP BY i.id,i.email,i.role,i.status,i.\"createdAt\",i.\"expiresAt\",u.name,u.\"mustChangePassword\" ORDER BY i.\"createdAt\" DESC,i.id";
pub(crate) async fn local_org<C: ConnectionTrait>(db: &C, org: &str) -> Result<bool> {
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.local_organization_authority($1) AS allowed",
            [org.into()],
        ))
        .await?
        .ok_or(Error::NotFound)?
        .try_get("", "allowed")?)
}
async fn live(tx: &DatabaseTransaction) -> Result<()> {
    if tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.actor() IS NOT NULL AS allowed",
        ))
        .await?
        .ok_or(Error::Unauthorized)?
        .try_get::<bool>("", "allowed")?
    {
        Ok(())
    } else {
        Err(Error::Unauthorized)
    }
}
/// Shared with native preview writers: acquire before retained auth and customer locks.
/// Other native domains can participate using this exact namespace/salt without an edge to Access.
pub(crate) async fn lock_session_context(tx: &DatabaseTransaction, session: &str) -> Result<()> {
    tx.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        [format!("native-session-context:{session}").into()],
    ))
    .await?;
    Ok(())
}
pub(crate) async fn operator(tx: &DatabaseTransaction) -> Result<()> {
    if tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.operator() AS allowed",
        ))
        .await?
        .ok_or(Error::Forbidden)?
        .try_get::<bool>("", "allowed")?
    {
        Ok(())
    } else {
        Err(Error::Forbidden)
    }
}
pub(crate) async fn lock_org(tx: &DatabaseTransaction, org: &str) -> Result<()> {
    tx.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webdock_auth.organization WHERE id=$1 FOR UPDATE",
        [id(org)?.into()],
    ))
    .await?
    .ok_or(Error::NotFound)?;
    Ok(())
}
pub(crate) async fn tenant(
    tx: &DatabaseTransaction,
    actor: &Principal,
    customer: &str,
) -> Result<Value> {
    let sql = format!(
        "SELECT to_jsonb(profile) AS data FROM (SELECT {PROFILE},t.organization_id,webdock_rls.membership_role(t.organization_id) AS member_role FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id LEFT JOIN webdock_auth.member m ON m.\"organizationId\"=t.organization_id AND m.\"userId\"=$2 WHERE c.id=$1 AND ($3 OR $4 OR (c.status='active' AND m.id IS NOT NULL AND webdock_rls.local_organization_authority(t.organization_id)))) profile"
    );
    let rows = json_rows(
        tx,
        &sql,
        vec![
            id(customer)?.into(),
            actor.user_id.clone().into(),
            actor.is_operator().into(),
            actor.in_tenant_preview().into(),
        ],
    )
    .await?;
    if rows.len() != 1 {
        return Err(Error::NotFound);
    }
    rows.into_iter().next().ok_or(Error::NotFound)
}
fn manageable(actor: &Principal, tenant: &Value) -> Result<()> {
    if actor.in_read_only_preview()
        || tenant["status"] != "active"
        || (!actor.is_operator()
            && !["owner", "admin"].contains(&tenant["member_role"].as_str().unwrap_or("")))
    {
        Err(Error::Forbidden)
    } else {
        Ok(())
    }
}
pub(crate) async fn json_rows<C: ConnectionTrait>(
    db: &C,
    sql: &str,
    args: Vec<SqlValue>,
) -> Result<Vec<Value>> {
    db.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        args,
    ))
    .await?
    .into_iter()
    .map(|row| row.try_get("", "data").map_err(Into::into))
    .collect()
}
pub(crate) fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name].as_str().ok_or(Error::InvalidInput)
}
fn strings(value: &Value) -> Result<Vec<String>> {
    value
        .as_array()
        .ok_or(Error::InvalidInput)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or(Error::InvalidInput))
        .collect()
}
fn normalize(value: &str) -> String {
    value
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .replace('ß', "ss")
        .to_lowercase()
        .trim()
        .to_owned()
}
fn matches_query(text: &str, query: &str) -> bool {
    let text = normalize(text);
    let query = normalize(query);
    if query.is_empty() || text.contains(&query) {
        return true;
    }
    let words = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if !query.chars().any(char::is_alphanumeric) {
        return false;
    }
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .all(|token| {
            words.iter().any(|word| {
                word.starts_with(token)
                    || (token.chars().count() >= 4
                        && !token.bytes().all(|b| b.is_ascii_digit())
                        && strsim::levenshtein(word, token) <= 1)
            })
        })
}
async fn search_rows(
    tx: &DatabaseTransaction,
    sql: &str,
    query: &str,
    fields: &[&str],
    limit: usize,
) -> Result<Vec<Value>> {
    let mut output = vec![];
    let mut offset = 0i64;
    loop {
        let rows = json_rows(
            tx,
            &format!("{sql} LIMIT 300 OFFSET $1"),
            vec![offset.into()],
        )
        .await?;
        let count = rows.len();
        for row in rows {
            let text = fields
                .iter()
                .filter_map(|f| row[*f].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if matches_query(&text, query) {
                output.push(row);
                if output.len() == limit {
                    return Ok(output);
                }
            }
        }
        if count < 300 {
            return Ok(output);
        }
        offset += 300;
    }
}

#[cfg(test)]
mod search_tests {
    use super::matches_query;
    #[test]
    fn bounded_directory_matching_handles_accents_typos_and_punctuation() {
        assert!(matches_query("Jörg Straße", "jorg stras"));
        assert!(matches_query("Recipient@example.invalid", "recipent"));
        assert!(!matches_query("Alice 12345", "12346"));
        assert!(!matches_query("Alice Example", "?!"));
    }
}
