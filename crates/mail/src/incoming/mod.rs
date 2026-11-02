//! Incoming Mail desired state. Reads never allocate an engine; mutations enqueue fenced work.
//! Only a separately authenticated executor can acknowledge observed runtime/configuration state.
pub mod agent;
mod capacity;
mod configuration;
mod directory;
mod service;
mod types;
pub use types::*;

use crate::{
    MailDependencies, authority,
    outbox::{field, one},
};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, QueryResult, Statement};
use serde::de::DeserializeOwned;
use webdock_browser_session::StoredAuthority;
use webdock_identity::{Principal, RlsSigner};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid incoming Mail request")]
    Invalid,
    #[error("incoming Mail is unavailable")]
    Unavailable,
    #[error("incoming Mail changed or requires reconciliation")]
    Conflict,
}
type Result<T> = std::result::Result<T, Error>;
impl From<crate::Error> for Error {
    fn from(_: crate::Error) -> Self {
        Self::Unavailable
    }
}
pub struct IncomingConfig {
    pub enabled: bool,
    pub mailboxes_enabled: bool,
    pub sending_enabled: bool,
    pub managed_cluster: String,
    pub rls_secret: String,
}
pub struct NativeIncoming {
    dependencies: MailDependencies,
    config: IncomingConfig,
    signer: RlsSigner,
    local_docker: bool,
}
impl NativeIncoming {
    pub async fn new(dependencies: MailDependencies, config: IncomingConfig) -> Result<Self> {
        authority::validate_pools(&dependencies).await?;
        if config.enabled && !crate::proof::valid_id(&config.managed_cluster) {
            return Err(Error::Invalid);
        }
        let signer = RlsSigner::new(&config.rls_secret, "webdock_auth_runtime")
            .map_err(|_| Error::Invalid)?;
        Ok(Self {
            dependencies,
            config,
            signer,
            local_docker: false,
        })
    }
    /// Explicit owned local integration backend; unavailable in production builds.
    #[cfg(feature = "test-support")]
    pub fn with_local_docker_test(mut self) -> Self {
        self.local_docker = true;
        self
    }
    async fn authorize(
        &self,
        principal: &Principal,
        stored: &StoredAuthority,
        customer: &str,
        manage: bool,
    ) -> Result<authority::AuthorizedCustomer> {
        if !self.config.enabled {
            return Err(Error::Unavailable);
        }
        Ok(authority::customer(
            &self.dependencies,
            &self.signer,
            &principal.user_id,
            &principal.session_id,
            stored,
            customer,
            manage,
        )
        .await?)
    }
    async fn id(&self) -> Result<String> {
        self.dependencies
            .ids
            .allocate()
            .await
            .map(|id| id.to_string())
            .map_err(|_| Error::Unavailable)
    }
}
async fn rows(
    tx: &DatabaseTransaction,
    query: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Vec<QueryResult>> {
    tx.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await
    .map_err(|_| Error::Unavailable)
}
fn decode<T: DeserializeOwned>(row: &QueryResult, key: &str) -> Result<T> {
    serde_json::from_value(field(row, key)?).map_err(|_| Error::Unavailable)
}
fn state<T: DeserializeOwned>(row: &QueryResult, key: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(field(row, key)?))
        .map_err(|_| Error::Unavailable)
}
fn revision(value: &str) -> Result<i64> {
    if value.is_empty()
        || (value.starts_with('0') && value != "0")
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::Invalid);
    }
    value.parse::<i64>().map_err(|_| Error::Invalid)
}
async fn ready(tx: &DatabaseTransaction, customer: &str) -> Result<bool> {
    Ok(one(tx,"SELECT 1 FROM webdock_mail.service WHERE customer_id=$1 AND desired_enabled AND state='ready' AND revision=applied_revision AND EXISTS(SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true') FOR SHARE",vec![customer.into()]).await?.is_some())
}
async fn can_manage(access: &authority::AuthorizedCustomer, customer: &str) -> Result<bool> {
    if access.operator || access.principal.is_acting_customer() {
        return Ok(true);
    }
    Ok(one(&access.business,"SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE t.customer_id=$1 AND m.\"userId\"=$2 AND m.role IN ('owner','admin')",vec![customer.into(),access.principal.user_id.clone().into()]).await?.is_some())
}
async fn commit(access: authority::AuthorizedCustomer) -> Result<()> {
    access
        .business
        .commit()
        .await
        .map_err(|_| Error::Unavailable)?;
    access.auth.commit().await.map_err(|_| Error::Unavailable)
}
