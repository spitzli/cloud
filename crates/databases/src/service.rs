use crate::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_browser_session::BrowserSessions;
use spitzli_identity::{IdentityService, Principal, RlsSigner};
use spitzli_persistence::IdAllocator;
use spitzli_secrets::SecretCipher;
use std::sync::Arc;
mod agent;
mod authority;
mod backups;
mod binding;
mod capacity;
mod commands;
mod control;
mod gateway;
mod maintenance;
mod orders;
mod read;
// Staged only: no production parser, route or claim can call this module.
#[cfg(test)]
mod restore_orders;
mod upgrades;
/// Separate pools preserve the restricted authentication, business, and worker boundaries.
pub struct DatabaseService {
    db: DatabaseConnection,
    auth: DatabaseConnection,
    worker: DatabaseConnection,
    identity: Arc<IdentityService>,
    browser: Option<Arc<BrowserSessions>>,
    ids: Arc<IdAllocator>,
    signer: RlsSigner,
    rls_secret: String,
    cipher: SecretCipher,
    gateway_origin: String,
    management: Option<crate::management::ManagementConfig>,
    new_orders_enabled: bool,
}
impl DatabaseService {
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    pub async fn new(
        db: DatabaseConnection,
        auth: DatabaseConnection,
        worker: DatabaseConnection,
        identity: Arc<IdentityService>,
        browser: Option<Arc<BrowserSessions>>,
        ids: Arc<IdAllocator>,
        rls_secret: String,
        encryption_secret: &str,
        gateway_origin: &str,
    ) -> Result<Self> {
        for (pool, role) in [
            (&db, "webdock_auth_runtime"),
            (&auth, "webdock_auth_bootstrap"),
            (&worker, "webdock_worker_runtime"),
        ] {
            spitzli_persistence::connection::verify_runtime_role(pool, role)
                .await
                .map_err(|_| unavailable())?;
        }
        Ok(Self {
            db,
            auth,
            worker,
            identity,
            browser,
            ids,
            signer: RlsSigner::new(rls_secret.clone(), "webdock_auth_runtime")
                .map_err(|_| unavailable())?,
            rls_secret,
            cipher: SecretCipher::new(encryption_secret).map_err(|_| unavailable())?,
            gateway_origin: normalize_origin(gateway_origin)?,
            management: None,
            new_orders_enabled: true,
        })
    }
    async fn id(&self) -> Result<String> {
        Ok(self
            .ids
            .allocate()
            .await
            .map_err(|_| unavailable())?
            .to_string())
    }
    async fn verify(
        &self,
        subject: &str,
        session: &str,
        authority: &StoredAuthority,
    ) -> Result<(DatabaseTransaction, Principal)> {
        let auth = self.auth.begin().await?;
        let principal = spitzli_browser_session::revalidate_stored_authority(
            &self.identity,
            &auth,
            self.browser.as_deref(),
            subject,
            session,
            authority,
        )
        .await
        .map_err(|_| denied())?;
        if !principal.can_access() || principal.in_read_only_preview() {
            return Err(denied());
        }
        Ok((auth, principal))
    }
    async fn begin_worker(&self) -> Result<DatabaseTransaction> {
        self.worker_scope(json!({"kind":"system","purpose":"database-gateway"}))
            .await
    }
    async fn worker_scope(&self, scope: Value) -> Result<DatabaseTransaction> {
        let tx = self.worker.begin().await?;
        self.apply_worker_scope(&tx, scope).await?;
        Ok(tx)
    }
    async fn apply_worker_scope(&self, tx: &DatabaseTransaction, scope: Value) -> Result<()> {
        let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":scope}).to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(self.rls_secret.as_bytes())
            .map_err(|_| unavailable())?;
        mac.update(raw.as_bytes());
        tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true),set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",[raw.into(),hex::encode(mac.finalize().into_bytes()).into()])).await?;
        Ok(())
    }
    async fn begin_actor(&self, principal: &Principal) -> Result<DatabaseTransaction> {
        self.signer
            .begin(&self.db, principal)
            .await
            .map_err(|_| denied())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    async fn authorize(
        &self,
        tx: &DatabaseTransaction,
        principal: &Principal,
        project: Option<&str>,
        customer: Option<&str>,
        write: bool,
        operator: bool,
        authority: &StoredAuthority,
    ) -> Result<Value> {
        self.authorize_mode(
            tx, principal, project, customer, write, operator, authority, true,
        )
        .await
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    async fn authorize_mode(
        &self,
        tx: &DatabaseTransaction,
        principal: &Principal,
        project: Option<&str>,
        customer: Option<&str>,
        write: bool,
        operator: bool,
        authority: &StoredAuthority,
        live: bool,
    ) -> Result<Value> {
        self.authorize_ceiling(
            tx,
            principal,
            project,
            customer,
            write,
            operator,
            authority.read_only(),
            live,
        )
        .await
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "The existing scoped authorizer also serves verified MCP reads without inventing browser authority."
    )]
    async fn authorize_ceiling(
        &self,
        tx: &DatabaseTransaction,
        principal: &Principal,
        project: Option<&str>,
        customer: Option<&str>,
        write: bool,
        operator: bool,
        read_only: bool,
        live: bool,
    ) -> Result<Value> {
        if (principal.in_read_only_preview() && (write || operator || live))
            || !principal.can_access()
            || write && read_only
        {
            return Err(denied());
        }
        let actor = one(tx, "SELECT webdock_rls.actor() AS value", vec![]).await?;
        if actor["subject"] != principal.user_id
            || actor["sessionID"] != principal.session_id
            || (actor["preview"] != Value::Null
                && !principal.is_acting_customer()
                && (write || operator || live))
        {
            return Err(denied());
        }
        if operator && actor["operator"] != true {
            return Err(denied());
        }
        if project.is_none() && customer.is_none() {
            if actor["operator"] != true {
                return Err(denied());
            }
            return Ok(json!({"customerID":null,"operator":true,"canWrite":!read_only}));
        }
        let customer = if let Some(project) = project {
            tx.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_admin.lock_hosting_project($1)",
                [project.into()],
            ))
            .await?;
            one(tx,"SELECT jsonb_build_object('customer',p.customer_id) AS value FROM webdock_admin.projects p WHERE p.id=$1 AND p.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted') ",vec![project.into()]).await?["customer"].as_str().ok_or_else(denied)?.to_owned()
        } else {
            customer.ok_or_else(denied)?.to_owned()
        };
        let access=one(tx,"SELECT jsonb_build_object('customerID',c.id,'operator',webdock_rls.operator(),'canWrite',webdock_rls.customer_permission(c.id,'org:databases:manage')) AS value FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 AND c.status='active' AND webdock_rls.native_organization_authority(t.organization_id) AND webdock_rls.customer_allowed(c.id,false) FOR SHARE OF c",vec![customer.into()]).await?;
        if write && access["canWrite"] != true {
            return Err(denied());
        }
        Ok(access)
    }
    async fn installed_scoped(&self, tx: &DatabaseTransaction, provision: bool) -> Result<()> {
        let scope = one(tx, "SELECT webdock_rls.request_scope() AS value", vec![]).await?;
        if scope["kind"] == "session" {
            return self.installed(tx, provision).await;
        }
        if scope["kind"] != "system"
            || ![
                "agent",
                "git-worker",
                "hosting-setup",
                "database-order",
                "database-gateway",
            ]
            .contains(&scope["purpose"].as_str().unwrap_or_default())
        {
            return Err(denied());
        }
        self.apply_worker_scope(tx, json!({"kind":"system","purpose":"plugin-admission"}))
            .await?;
        let result = self.installed(tx, provision).await;
        self.apply_worker_scope(tx, scope).await?;
        result
    }
    async fn installed(&self, tx: &DatabaseTransaction, provision: bool) -> Result<()> {
        tx.query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))",
        ))
        .await?;
        let state=one(tx,"SELECT jsonb_build_object('state',state) AS value FROM webdock_auth.plugin_installation WHERE id='databases'",vec![]).await?;
        if state["state"] == "disabled" || provision && state["state"] == "draining" {
            return Err(unavailable());
        }
        Ok(())
    }
    async fn audit(
        &self,
        tx: &DatabaseTransaction,
        principal: &Principal,
        action: &str,
        binding: &str,
    ) -> Result<()> {
        exec(tx,"INSERT INTO webdock_auth.database_audit(id,subject,action,binding_id) VALUES($1,$2,$3,$4)",vec![self.id().await?.into(),principal.user_id.clone().into(),action.into(),binding.into()]).await
    }
}
async fn one(tx: &DatabaseTransaction, sql: &str, values: Vec<sea_orm::Value>) -> Result<Value> {
    optional(tx, sql, values).await?.ok_or_else(denied)
}
async fn optional(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Option<Value>> {
    tx.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await?
    .map(|row| row.try_get("", "value").map_err(Into::into))
    .transpose()
}
async fn all(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Vec<Value>> {
    tx.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await?
    .into_iter()
    .map(|row| row.try_get("", "value").map_err(Into::into))
    .collect()
}
async fn exec(tx: &DatabaseTransaction, sql: &str, values: Vec<sea_orm::Value>) -> Result<()> {
    tx.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await?;
    Ok(())
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key].as_str().ok_or_else(denied)
}
fn number(value: &Value, key: &str) -> Result<i32> {
    i32::try_from(value[key].as_i64().ok_or_else(denied)?).map_err(|_| denied())
}
fn token() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| unavailable())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn hash(token: &str) -> Result<String> {
    if token.len() != 43
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(denied());
    }
    Ok(hex::encode(Sha256::digest(token.as_bytes())))
}
const SELECTION: &str = "SELECT jsonb_build_object('id',b.id,'projectID',p.id,'customerID',p.customer_id,'name',b.name,'environment',b.environment,'engine',b.engine,'profile',g.profile) AS value FROM webdock_auth.database_binding b JOIN webdock_admin.projects p ON p.id=b.project_id LEFT JOIN webdock_auth.database_grant g ON g.binding_id=b.id AND g.subject=$1 AND g.enabled WHERE b.enabled AND p.status='active'";

mod exports;
