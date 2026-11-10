//! Native Core BYOK connections. Provider keys never cross the browser response boundary.
mod database;
#[cfg(test)]
mod database_tests;
mod neon;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_browser_session::{BrowserSessions, StoredAuthority, revalidate_stored_authority};
use spitzli_identity::{IdentityService, Principal, RlsSigner};
use spitzli_secrets::SecretCipher;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Check the Neon connection fields.")]
    InvalidInput,
    #[error("The Neon API key cannot access this account or organization.")]
    InvalidCredentials,
    #[error(
        "Neon scope is unavailable. For a project API key, enter its Neon project ID and check the selected organization."
    )]
    InvalidScope,
    #[error("Choose a Neon project in Frankfurt (aws-eu-central-1) for this integration.")]
    UnsupportedDatabaseRegion,
    #[error("Neon connection management is unavailable for this customer.")]
    Forbidden,
    #[error("The Neon connection changed. Reload before trying again.")]
    Conflict,
    #[error(
        "Neon inventory or connection storage is unavailable. Check the API key and try again."
    )]
    Unavailable,
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<sea_orm::DbErr> for Error {
    fn from(_: sea_orm::DbErr) -> Self {
        Self::Unavailable
    }
}
impl Error {
    pub fn status(&self) -> u16 {
        match self {
            Self::InvalidInput
            | Self::InvalidCredentials
            | Self::InvalidScope
            | Self::UnsupportedDatabaseRegion => 400,
            Self::Forbidden => 403,
            Self::Conflict => 409,
            Self::Unavailable => 503,
        }
    }
}

// Each variant accepts exactly its own fields; credentials cannot hitchhike on reads.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
pub enum Command {
    Get {
        #[serde(rename = "customerID")]
        customer: String,
    },
    Connect {
        #[serde(rename = "customerID")]
        customer: String,
        #[serde(rename = "apiKey")]
        api_key: String,
        #[serde(rename = "organizationID")]
        organization: Option<String>,
        #[serde(rename = "neonProjectID")]
        neon_project: Option<String>,
        #[serde(rename = "expectedRevision")]
        revision: i64,
        #[serde(rename = "idempotencyKey")]
        idempotency: String,
    },
    Projects {
        #[serde(rename = "customerID")]
        customer: String,
        cursor: Option<String>,
    },
    Disconnect {
        #[serde(rename = "customerID")]
        customer: String,
        #[serde(rename = "expectedRevision")]
        revision: i64,
        #[serde(rename = "idempotencyKey")]
        idempotency: String,
    },
}
impl Command {
    pub fn parse(value: Value) -> Result<Self> {
        let command: Self = serde_json::from_value(value).map_err(|_| Error::InvalidInput)?;
        command.validate()
    }
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let command: Self = serde_json::from_slice(bytes).map_err(|_| Error::InvalidInput)?;
        command.validate()
    }
    fn validate(self) -> Result<Self> {
        let command = self;
        let customer = command.customer();
        if customer
            .parse::<i64>()
            .ok()
            .is_none_or(|id| id <= 0 || id.to_string() != customer)
        {
            return Err(Error::InvalidInput);
        }
        match &command {
            Self::Connect {
                api_key,
                organization,
                neon_project,
                revision,
                idempotency,
                ..
            } => {
                neon::ApiKey::new(api_key.clone())?;
                if organization
                    .as_deref()
                    .is_some_and(|id| !neon::identifier(id) || id.len() > 60)
                {
                    return Err(Error::InvalidInput);
                }
                if neon_project
                    .as_deref()
                    .is_some_and(|id| !neon::identifier(id))
                {
                    return Err(Error::InvalidInput);
                }
                mutation_valid(*revision, idempotency)?;
            }
            Self::Disconnect {
                revision,
                idempotency,
                ..
            } => mutation_valid(*revision, idempotency)?,
            Self::Projects { cursor, .. }
                if cursor.as_deref().is_some_and(|c| {
                    c.is_empty() || c.len() > 1024 || c.chars().any(char::is_control)
                }) =>
            {
                return Err(Error::InvalidInput);
            }
            _ => {}
        }
        Ok(command)
    }
    fn customer(&self) -> &str {
        match self {
            Self::Get { customer }
            | Self::Connect { customer, .. }
            | Self::Projects { customer, .. }
            | Self::Disconnect { customer, .. } => customer,
        }
    }
}
fn mutation_valid(revision: i64, key: &str) -> Result<()> {
    if !(0..9_007_199_254_740_991).contains(&revision)
        || !(16..=128).contains(&key.len())
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::InvalidInput);
    }
    Ok(())
}
struct Guard {
    business: DatabaseTransaction,
    authentication: DatabaseTransaction,
    principal: Principal,
    proof: StoredAuthority,
    customer: String,
    manage: bool,
}
pub struct Connections {
    db: DatabaseConnection,
    auth: DatabaseConnection,
    identity: Arc<IdentityService>,
    browser: Option<Arc<BrowserSessions>>,
    signer: RlsSigner,
    cipher: SecretCipher,
    provider: neon::Client,
}
impl Connections {
    pub fn new(
        db: DatabaseConnection,
        auth: DatabaseConnection,
        identity: Arc<IdentityService>,
        browser: Option<Arc<BrowserSessions>>,
        rls_secret: &str,
        encryption_key: &str,
    ) -> Result<Self> {
        Ok(Self {
            db,
            auth,
            identity,
            browser,
            signer: RlsSigner::new(rls_secret, "webdock_auth_runtime")
                .map_err(|_| Error::Unavailable)?,
            cipher: SecretCipher::new(encryption_key).map_err(|_| Error::Unavailable)?,
            provider: neon::Client::new()?,
        })
    }
    #[cfg(feature = "test-transport")]
    pub fn with_test_provider(mut self, origin: String) -> Self {
        self.provider = neon::Client::test_client(origin);
        self
    }
    async fn guard(
        &self,
        principal: &Principal,
        proof: &StoredAuthority,
        customer: &str,
        manage: bool,
    ) -> Result<Guard> {
        let business = self.db.begin().await?;
        business
            .execute_unprepared("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='10s'")
            .await?;
        // Scope exit shares this fence with all native browser mutations.
        one(
            &business,
            "SELECT pg_advisory_xact_lock_shared(hashtextextended($1,0))",
            vec![format!("native-session-context:{}", principal.session_id).into()],
        )
        .await?;
        let authentication = self.auth.begin().await?;
        authentication
            .execute_unprepared("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='10s'")
            .await?;
        let current = revalidate_stored_authority(
            &self.identity,
            &authentication,
            self.browser.as_deref(),
            &principal.user_id,
            &principal.session_id,
            proof,
        )
        .await
        .map_err(authority_error)?;
        if current.customer_scope() != principal.customer_scope()
            || manage && (proof.read_only() || current.in_read_only_preview())
        {
            return Err(Error::Forbidden);
        }
        self.signer
            .apply(&business, &current)
            .await
            .map_err(|_| Error::Unavailable)?;
        if current.is_operator() {
            let mapped = one(
                &business,
                "SELECT role::text AS role FROM webdock_admin.users WHERE auth_subject=$1 FOR SHARE",
                vec![current.user_id.clone().into()],
            )
            .await?
            .ok_or(Error::Forbidden)?;
            if mapped.try_get::<String>("", "role")? != "operator" {
                return Err(Error::Forbidden);
            }
        }
        let allowed = one(
            &business,
            "SELECT webdock_rls.neon_connection_allowed($1,$2) AS allowed",
            vec![customer.into(), manage.into()],
        )
        .await?
        .ok_or(Error::Unavailable)?
        .try_get::<bool>("", "allowed")?;
        if !allowed {
            return Err(Error::Forbidden);
        }
        let binding = one(&business,
            "SELECT t.organization_id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 AND c.status='active' FOR SHARE OF c,t",
            vec![customer.into()]).await?.ok_or(Error::Forbidden)?;
        if !current.is_operator() && !current.in_tenant_preview() {
            let organization: String = binding.try_get("", "organization_id")?;
            let membership=one(&business,"SELECT role FROM webdock_auth.member WHERE \"organizationId\"=$1 AND \"userId\"=$2 FOR SHARE",vec![organization.into(),current.user_id.clone().into()]).await?.ok_or(Error::Forbidden)?;
            if !["owner", "admin"].contains(&membership.try_get::<String>("", "role")?.as_str()) {
                return Err(Error::Forbidden);
            }
        }
        // Per-customer serialization is held only during local work, never provider IO.
        one(
            &business,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("native-neon-connection:{customer}").into()],
        )
        .await?;
        Ok(Guard {
            business,
            authentication,
            principal: current,
            proof: proof.clone(),
            customer: customer.into(),
            manage,
        })
    }
    async fn finish(&self, guard: Guard) -> Result<()> {
        let current = revalidate_stored_authority(
            &self.identity,
            &guard.authentication,
            self.browser.as_deref(),
            &guard.principal.user_id,
            &guard.principal.session_id,
            &guard.proof,
        )
        .await
        .map_err(authority_error)?;
        if current.customer_scope() != guard.principal.customer_scope() {
            return Err(Error::Forbidden);
        }
        if guard.manage && (guard.proof.read_only() || current.in_read_only_preview()) {
            return Err(Error::Forbidden);
        }
        let allowed = one(
            &guard.business,
            "SELECT webdock_rls.neon_connection_allowed($1,$2) AS allowed",
            vec![guard.customer.clone().into(), guard.manage.into()],
        )
        .await?
        .ok_or(Error::Unavailable)?
        .try_get::<bool>("", "allowed")?;
        if !allowed {
            return Err(Error::Forbidden);
        }
        guard.business.commit().await?;
        guard.authentication.commit().await?;
        Ok(())
    }
    pub async fn execute(
        &self,
        principal: &Principal,
        proof: &StoredAuthority,
        command: Command,
    ) -> Result<Value> {
        match command {
            Command::Get { customer } => {
                let guard = self.guard(principal, proof, &customer, false).await?;
                let connection = load(&guard.business, &customer).await?;
                let mut can_manage = !proof.read_only()
                    && !guard.principal.in_read_only_preview()
                    && one(
                        &guard.business,
                        "SELECT webdock_rls.neon_connection_allowed($1,true) AS allowed",
                        vec![customer.clone().into()],
                    )
                    .await?
                    .ok_or(Error::Unavailable)?
                    .try_get::<bool>("", "allowed")?;
                if can_manage
                    && !guard.principal.is_operator()
                    && !guard.principal.in_tenant_preview()
                {
                    can_manage = one(&guard.business,
                        "SELECT EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_auth.member m ON m.\"organizationId\"=t.organization_id WHERE t.customer_id=$1 AND m.\"userId\"=$2 AND m.role IN ('owner','admin')) AS allowed",
                        vec![customer.clone().into(),guard.principal.user_id.clone().into()]).await?.ok_or(Error::Unavailable)?.try_get::<bool>("","allowed")?;
                }
                let result = reply(connection.as_ref(), can_manage);
                self.finish(guard).await?;
                Ok(result)
            }
            Command::Projects { customer, cursor } => {
                let guard = self.guard(principal, proof, &customer, true).await?;
                let connection = load(&guard.business, &customer)
                    .await?
                    .filter(|row| row["status"] == "connected")
                    .ok_or(Error::Conflict)?;
                let key: String = self
                    .cipher
                    .open(
                        field(&connection, "encrypted_key")?,
                        &purpose(
                            &customer,
                            field(&connection, "id")?,
                            revision(Some(&connection)),
                            connection["neon_project_id"].as_str(),
                        ),
                    )
                    .map_err(|_| Error::Unavailable)?;
                self.finish(guard).await?;
                let page = self
                    .provider
                    .inventory(
                        &neon::ApiKey::new(key)?,
                        connection["organization_id"].as_str(),
                        connection["neon_project_id"].as_str(),
                        cursor.as_deref(),
                    )
                    .await?;
                // A disconnect, key replacement or scope revocation while IO ran invalidates this page.
                let guard = self.guard(principal, proof, &customer, true).await?;
                let current = load(&guard.business, &customer).await?;
                if revision(current.as_ref()) != revision(Some(&connection)) {
                    return Err(Error::Conflict);
                }
                let mut result = reply(current.as_ref(), true);
                result["projects"] = json!(page.projects);
                result["nextCursor"] = json!(page.next_cursor);
                self.finish(guard).await?;
                Ok(result)
            }
            Command::Connect {
                customer,
                api_key,
                organization,
                neon_project,
                revision: expected,
                idempotency,
            } => {
                let digest = payload_digest(
                    "connect",
                    expected,
                    organization.as_deref(),
                    neon_project.as_deref(),
                    Some(&api_key),
                );
                if let Some(result) = self
                    .preflight(principal, proof, &customer, expected, &idempotency, &digest)
                    .await?
                {
                    return Ok(result);
                }
                // Explicit user key only. This GET proves access without changing any Neon resources.
                self.provider
                    .inventory(
                        &neon::ApiKey::new(api_key.clone())?,
                        organization.as_deref(),
                        neon_project.as_deref(),
                        None,
                    )
                    .await?;
                self.mutate(
                    principal,
                    proof,
                    &customer,
                    expected,
                    (&idempotency, &digest),
                    Some((api_key, organization, neon_project)),
                )
                .await
            }
            Command::Disconnect {
                customer,
                revision: expected,
                idempotency,
            } => {
                let digest = payload_digest("disconnect", expected, None, None, None);
                self.mutate(
                    principal,
                    proof,
                    &customer,
                    expected,
                    (&idempotency, &digest),
                    None,
                )
                .await
            }
        }
    }
    async fn preflight(
        &self,
        principal: &Principal,
        proof: &StoredAuthority,
        customer: &str,
        expected: i64,
        key: &str,
        digest: &str,
    ) -> Result<Option<Value>> {
        let guard = self.guard(principal, proof, customer, true).await?;
        let row = load(&guard.business, customer).await?;
        let replay = replay(&guard.business, customer, key, digest, row.as_ref()).await?;
        if replay.is_none() && revision(row.as_ref()) != expected {
            return Err(Error::Conflict);
        }
        self.finish(guard).await?;
        Ok(replay)
    }
    async fn mutate(
        &self,
        principal: &Principal,
        proof: &StoredAuthority,
        customer: &str,
        expected: i64,
        receipt: (&str, &str),
        connect: Option<(String, Option<String>, Option<String>)>,
    ) -> Result<Value> {
        let (key, digest) = receipt;
        let guard = self.guard(principal, proof, customer, true).await?;
        let row = load(&guard.business, customer).await?;
        if let Some(result) = replay(&guard.business, customer, key, digest, row.as_ref()).await? {
            self.finish(guard).await?;
            return Ok(result);
        }
        if revision(row.as_ref()) != expected {
            return Err(Error::Conflict);
        }
        let next = expected
            .checked_add(1)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or(Error::Conflict)?;
        let id = row
            .as_ref()
            .and_then(|v| v["id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let (status, encrypted, org, project) =
            if let Some((api_key, organization, project)) = connect {
                (
                    "connected",
                    Some(
                        self.cipher
                            .seal(&api_key, &purpose(customer, &id, next, project.as_deref()))
                            .map_err(|_| Error::Unavailable)?,
                    ),
                    organization,
                    project,
                )
            } else {
                (
                    "disconnected",
                    None,
                    row.as_ref()
                        .and_then(|v| v["organization_id"].as_str())
                        .map(str::to_owned),
                    row.as_ref()
                        .and_then(|v| v["neon_project_id"].as_str())
                        .map(str::to_owned),
                )
            };
        guard.business.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.neon_connection(customer_id,id,revision,status,encrypted_key,organization_id,neon_project_id) VALUES($1,$2::uuid,$3,$4,$5,$6,$7) ON CONFLICT(customer_id) DO UPDATE SET revision=excluded.revision,status=excluded.status,encrypted_key=excluded.encrypted_key,organization_id=excluded.organization_id,neon_project_id=excluded.neon_project_id,connected_at=CASE WHEN excluded.status='connected' THEN statement_timestamp() ELSE webdock_auth.neon_connection.connected_at END,updated_at=statement_timestamp()",
            vec![customer.into(),id.into(),next.into(),status.into(),encrypted.into(),org.into(),project.into()])).await?;
        guard.business.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.neon_connection_receipt(customer_id,idempotency_key,payload_digest,revision) VALUES($1,$2,$3,$4)",vec![customer.into(),key.into(),digest.into(),next.into()])).await?;
        let result = reply(load(&guard.business, customer).await?.as_ref(), true);
        self.finish(guard).await?;
        Ok(result)
    }
}
fn authority_error(error: spitzli_browser_session::Error) -> Error {
    match error {
        spitzli_browser_session::Error::Database(_)
        | spitzli_browser_session::Error::Configuration
        | spitzli_browser_session::Error::Identity(
            spitzli_identity::IdentityError::Configuration
            | spitzli_identity::IdentityError::Crypto
            | spitzli_identity::IdentityError::RateLimited
            | spitzli_identity::IdentityError::Database(_)
            | spitzli_identity::IdentityError::Allocation
            | spitzli_identity::IdentityError::Worker
            | spitzli_identity::IdentityError::Persistence,
        ) => Error::Unavailable,
        spitzli_browser_session::Error::OAuth(error) if error.status() >= 500 => Error::Unavailable,
        _ => Error::Forbidden,
    }
}
async fn one(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Option<sea_orm::QueryResult>> {
    Ok(tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await?)
}
async fn load(tx: &DatabaseTransaction, customer: &str) -> Result<Option<Value>> {
    one(
        tx,
        "SELECT to_jsonb(c) AS value FROM webdock_auth.neon_connection c WHERE customer_id=$1",
        vec![customer.into()],
    )
    .await?
    .map(|r| r.try_get("", "value").map_err(Error::from))
    .transpose()
}
fn revision(row: Option<&Value>) -> i64 {
    row.and_then(|r| r["revision"].as_i64()).unwrap_or(0)
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key].as_str().ok_or(Error::Unavailable)
}
fn purpose(customer: &str, id: &str, revision: i64, project: Option<&str>) -> String {
    let base = format!("native-neon-api-key-v1:{customer}:{id}:{revision}");
    match project {
        Some(project) => format!("{base}:project:{project}"),
        None => base,
    }
}
fn reply(row: Option<&Value>, can_manage: bool) -> Value {
    json!({"canManage":can_manage,"connection":row.map(|r|json!({"id":r["id"],"revision":r["revision"],"status":r["status"],"authMethod":"api-key","organizationID":r["organization_id"],"neonProjectID":r["neon_project_id"],"connectedAt":r["connected_at"]}))})
}
fn payload_digest(
    action: &str,
    revision: i64,
    org: Option<&str>,
    project: Option<&str>,
    key: Option<&str>,
) -> String {
    let payload = match project {
        Some(project) => json!(["native-neon-v2", action, revision, org, project, key]),
        None => json!(["native-neon-v1", action, revision, org, key]),
    };
    hex::encode(Sha256::digest(payload.to_string().as_bytes()))
}
async fn replay(
    tx: &DatabaseTransaction,
    customer: &str,
    key: &str,
    digest: &str,
    row: Option<&Value>,
) -> Result<Option<Value>> {
    let Some(receipt)=one(tx,"SELECT payload_digest,revision FROM webdock_auth.neon_connection_receipt WHERE customer_id=$1 AND idempotency_key=$2",vec![customer.into(),key.into()]).await? else { return Ok(None) };
    if receipt.try_get::<String>("", "payload_digest")? != digest
        || receipt.try_get::<i64>("", "revision")? != revision(row)
    {
        return Err(Error::Conflict);
    }
    Ok(Some(reply(row, true)))
}

#[cfg(test)]
mod tests;
