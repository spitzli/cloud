use crate::{
    Error, MailDependencies, Result,
    outbox::{field, one},
    proof::valid_id,
};
use hmac::{Hmac, Mac};
use sea_orm::{DatabaseTransaction, TransactionTrait};
use serde_json::{Value, json};
use sha2::Sha256;
use webdock_browser_session::{StoredAuthority, revalidate_stored_authority};
use webdock_identity::{Principal, RlsSigner};
pub(crate) struct AuthorizedCustomer {
    pub auth: DatabaseTransaction,
    pub business: DatabaseTransaction,
    pub principal: Principal,
    pub operator: bool,
    pub member_id: Option<String>,
}
pub(crate) async fn validate_pools(deps: &MailDependencies) -> Result<()> {
    for (db, role) in [
        (&deps.auth, "webdock_auth_bootstrap"),
        (&deps.business, "webdock_auth_runtime"),
        (&deps.worker, "webdock_worker_runtime"),
    ] {
        webdock_persistence::connection::verify_runtime_role(db, role)
            .await
            .map_err(|_| Error::Unavailable)?;
    }
    Ok(())
}
pub(crate) async fn module(tx: &DatabaseTransaction, write: bool) -> Result<()> {
    crate::recovery::ensure_open(tx).await?;
    one(
        tx,
        "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))",
        vec![],
    )
    .await?;
    let row = one(
        tx,
        "SELECT state FROM webdock_auth.plugin_installation WHERE id='mail'",
        vec![],
    )
    .await?
    .ok_or(Error::Unavailable)?;
    let state: String = field(&row, "state")?;
    if state != "enabled" && !(state == "draining" && !write) {
        return Err(Error::Unavailable);
    }
    Ok(())
}
pub(crate) async fn customer(
    deps: &MailDependencies,
    signer: &RlsSigner,
    subject: &str,
    session: &str,
    authority: &StoredAuthority,
    customer: &str,
    manage: bool,
) -> Result<AuthorizedCustomer> {
    customer_access(
        deps,
        signer,
        subject,
        session,
        authority,
        CustomerPolicy {
            id: customer,
            allow_inactive_operator: false,
            module_write: manage,
        },
        manage,
    )
    .await
}
/// A platform operator must still be able to suspend retained infrastructure after archival.
pub(crate) async fn customer_for_lifecycle(
    deps: &MailDependencies,
    signer: &RlsSigner,
    principal: &Principal,
    authority: &StoredAuthority,
    customer: &str,
    manage: bool,
) -> Result<AuthorizedCustomer> {
    customer_access(
        deps,
        signer,
        &principal.user_id,
        &principal.session_id,
        authority,
        CustomerPolicy {
            id: customer,
            allow_inactive_operator: true,
            module_write: manage,
        },
        manage,
    )
    .await
}
/// Revocation is allowed during draining, but retains management authorization.
pub(crate) async fn customer_for_revocation(
    deps: &MailDependencies,
    signer: &RlsSigner,
    principal: &Principal,
    authority: &StoredAuthority,
    customer: &str,
) -> Result<AuthorizedCustomer> {
    customer_access(
        deps,
        signer,
        &principal.user_id,
        &principal.session_id,
        authority,
        CustomerPolicy {
            id: customer,
            allow_inactive_operator: false,
            module_write: false,
        },
        true,
    )
    .await
}
struct CustomerPolicy<'a> {
    id: &'a str,
    allow_inactive_operator: bool,
    module_write: bool,
}
async fn customer_access(
    deps: &MailDependencies,
    signer: &RlsSigner,
    subject: &str,
    session: &str,
    authority: &StoredAuthority,
    target: CustomerPolicy<'_>,
    manage: bool,
) -> Result<AuthorizedCustomer> {
    let CustomerPolicy {
        id: customer,
        allow_inactive_operator,
        module_write,
    } = target;
    if !valid_id(customer) || authority.read_only() {
        return Err(Error::Unavailable);
    }
    let auth = deps.auth.begin().await.map_err(|_| Error::Unavailable)?;
    one(
        &auth,
        "SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",
        vec![],
    )
    .await?;
    let principal = revalidate_stored_authority(
        &deps.identity,
        &auth,
        deps.browser.as_deref(),
        subject,
        session,
        authority,
    )
    .await
    .map_err(|_| Error::Unavailable)?;
    if principal.in_read_only_preview() {
        return Err(Error::Unavailable);
    }
    let business = signer
        .begin(&deps.business, &principal)
        .await
        .map_err(|_| Error::Unavailable)?;
    module(&business, module_write).await?;
    let actor: Value = field::<Option<Value>>(
        &one(&business, "SELECT webdock_rls.actor() AS actor", vec![])
            .await?
            .ok_or(Error::Unavailable)?,
        "actor",
    )?
    .ok_or(Error::Unavailable)?;
    if actor["subject"] != subject
        || actor["sessionID"] != session
        || (!actor["preview"].is_null() && !principal.is_acting_customer())
    {
        return Err(Error::Unavailable);
    }
    let tenant=one(&business,"SELECT webdock_rls.customer_allowed(c.id,false) AS allowed FROM webdock_admin.customers c WHERE c.id=$1 AND (c.status='active' OR ($2 AND webdock_rls.operator())) FOR SHARE OF c",vec![customer.into(),allow_inactive_operator.into()]).await?.ok_or(Error::Unavailable)?;
    if !field::<bool>(&tenant, "allowed")? {
        return Err(Error::Unavailable);
    }
    let member=one(&business,"SELECT m.id,m.role FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE t.customer_id=$1 AND m.\"userId\"=$2 AND m.role<>'' FOR SHARE OF m",vec![customer.into(),subject.into()]).await?;
    let member_id = member
        .as_ref()
        .map(|r| field::<String>(r, "id"))
        .transpose()?;
    let role = member
        .as_ref()
        .map(|r| field::<String>(r, "role"))
        .transpose()?;
    let operator = actor["operator"] == true;
    if (!operator && !principal.is_acting_customer() && member_id.is_none())
        || (manage
            && !operator
            && !principal.is_acting_customer()
            && !role
                .as_deref()
                .is_some_and(|r| ["owner", "admin"].contains(&r)))
    {
        return Err(Error::Unavailable);
    }
    Ok(AuthorizedCustomer {
        auth,
        business,
        principal,
        operator,
        member_id,
    })
}
#[derive(Clone, Copy)]
pub(crate) enum Purpose {
    Webmail,
    Sending,
}
impl Purpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Webmail => "mail-webmail-gateway",
            Self::Sending => "mail-send-gateway",
        }
    }
}
async fn apply(tx: &DatabaseTransaction, secret: &str, purpose: &str) -> Result<()> {
    let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":purpose}}).to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| Error::Invalid)?;
    mac.update(raw.as_bytes());
    one(tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true),set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await?;
    Ok(())
}
/// Reuses the existing metadata-only scope without moving customer locks to another connection.
pub(crate) async fn agent_module(tx: &DatabaseTransaction, secret: &str) -> Result<bool> {
    let row=one(tx,"SELECT webdock_rls.worker(ARRAY['agent']::text[]) AS agent,current_setting('webdock.rls_context',true) AS context,current_setting('webdock.rls_signature',true) AS signature",vec![]).await?.ok_or(Error::Unavailable)?;
    if !field::<bool>(&row, "agent")? {
        return Err(Error::Unavailable);
    }
    let context: String = field(&row, "context")?;
    let signature: String = field(&row, "signature")?;
    apply(tx, secret, "installation-preflight").await?;
    one(
        tx,
        "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))",
        vec![],
    )
    .await?;
    let installed=one(tx,"SELECT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation WHERE id='mail' AND state IN ('enabled','draining')) AS allowed",vec![]).await?.ok_or(Error::Unavailable)?;
    one(tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",vec![context.into(),signature.into()]).await?;
    field(&installed, "allowed")
}
pub(crate) async fn worker(
    deps: &MailDependencies,
    secret: &str,
    purpose: Purpose,
    module_write: Option<bool>,
) -> Result<DatabaseTransaction> {
    let tx = deps.worker.begin().await.map_err(|_| Error::Unavailable)?;
    if let Some(write) = module_write {
        // Existing metadata-only preflight authority; never exposed as a caller-selected scope.
        // Keep the shared installation lock while restoring the narrowly named Mail worker scope.
        apply(&tx, secret, "installation-preflight").await?;
        module(&tx, write).await?;
    }
    apply(&tx, secret, purpose.as_str()).await?;
    crate::recovery::ensure_open(&tx).await?;
    Ok(tx)
}
