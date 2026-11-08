#[path = "native_session.rs"]
mod native_session;
#[path = "native_session_management.rs"]
mod native_session_management;
struct NativeSessionBackend {
    pool: webdock_persistence::postgres::Pool,
    management: bool,
}
use crate::{
    CookiePolicy, IdentityError, Result, SecretKeys,
    crypto::{hash_password, random, verify_password},
};
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, QueryResult, Statement,
    TransactionTrait,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub user_id: String,
    pub session_id: String,
    pub name: String,
    pub email: String,
    pub expires_at: DateTime<Utc>,
    role: String,
    email_verified: bool,
    must_change_password: bool,
    two_factor_enabled: bool,
    tenant_preview: bool,
    tenant_acting: bool,
    customer_scope: Option<String>,
}
impl Principal {
    pub fn can_access(&self) -> bool {
        self.email_verified
            && !self.must_change_password
            && (self.role == "user" || self.role == "operator" && self.two_factor_enabled)
            && self.expires_at > Utc::now()
    }
    pub fn is_operator(&self) -> bool {
        self.can_access() && self.role == "operator" && !self.tenant_preview
    }
    pub fn in_tenant_preview(&self) -> bool {
        self.tenant_preview
    }
    /// Owner authority still requires a matching, live customer scope in signed RLS.
    pub fn is_acting_customer(&self) -> bool {
        self.tenant_preview && self.tenant_acting && self.can_access() && self.role == "operator"
    }
    pub fn in_read_only_preview(&self) -> bool {
        self.tenant_preview && !self.is_acting_customer()
    }
    pub fn customer_scope(&self) -> Option<&str> {
        self.customer_scope.as_deref()
    }
    pub fn requires_password_change(&self) -> bool {
        self.must_change_password
    }
    pub fn email_verified(&self) -> bool {
        self.email_verified
    }
    pub fn two_factor_enabled(&self) -> bool {
        self.two_factor_enabled
    }
    pub fn role(&self) -> &str {
        &self.role
    }
}
/// Contains a Set-Cookie credential; deliberately not Debug/Serialize.
pub struct IssuedSession {
    pub principal: Principal,
    pub set_cookie: String,
}
/// Bare user IDs are never proof; only the accompanying signed cookie may continue a flow.
pub enum PasswordAuthentication {
    Invalid,
    SetupRequired {
        user_id: String,
        session: Option<IssuedSession>,
    },
    MfaRequired {
        user_id: String,
        set_cookie: String,
    },
    Authenticated(IssuedSession),
}
pub struct IdentityService {
    pub(crate) neon: Option<crate::NeonAuthClient>,
    pub(crate) neon_gate: tokio::sync::OnceCell<bool>,
    pub(crate) db: DatabaseConnection,
    native_sessions: Option<NativeSessionBackend>,
    pub(crate) cookies: CookiePolicy,
    pub(crate) password_workers: Arc<Semaphore>,
    pub(crate) keys: Option<SecretKeys>,
    pub(crate) webauthn: Option<webauthn_rs::Webauthn>,
    pub(crate) allocator: Option<Arc<webdock_persistence::IdAllocator>>,
    pub(crate) account_origin: Option<url::Url>,
    pub(crate) delivery: Arc<dyn crate::Delivery>,
}
impl IdentityService {
    pub fn new(db: DatabaseConnection, cookies: CookiePolicy) -> Self {
        Self {
            neon: None,
            neon_gate: tokio::sync::OnceCell::new(),
            db,
            native_sessions: None,
            cookies,
            password_workers: Arc::new(Semaphore::new(4)),
            keys: None,
            account_origin: None,
            allocator: None,
            webauthn: None,
            delivery: Arc::new(crate::UnavailableDelivery),
        }
    }
    pub fn with_native_session_reads(mut self, pool: webdock_persistence::postgres::Pool) -> Self {
        self.native_sessions = Some(NativeSessionBackend {
            pool,
            management: false,
        });
        self
    }
    pub fn with_native_session_management(
        mut self,
        pool: webdock_persistence::postgres::Pool,
    ) -> Self {
        self.native_sessions = Some(NativeSessionBackend {
            pool,
            management: true,
        });
        self
    }
    pub(crate) fn native_management_pool(&self) -> Option<&webdock_persistence::postgres::Pool> {
        self.native_sessions
            .as_ref()
            .filter(|backend| backend.management)
            .map(|backend| &backend.pool)
    }
    /// Trusted backend boundary: subject/session IDs alone are not authentication proof.
    /// Keep canonical share locks until the caller commits or rolls back this transaction.
    pub async fn authenticate_session_native_in<'a>(
        &self,
        tx: webdock_persistence::postgres::Transaction<'a>,
        subject: &str,
        session: &str,
    ) -> Result<(
        webdock_persistence::postgres::Transaction<'a>,
        Option<Principal>,
    )> {
        native_session::revalidate(tx, subject, session).await
    }
    pub fn with_allocator(mut self, allocator: Arc<webdock_persistence::IdAllocator>) -> Self {
        self.allocator = Some(allocator);
        self
    }
    pub(crate) async fn allocate_id<C: ConnectionTrait>(&self, connection: &C) -> Result<String> {
        if let Some(allocator) = &self.allocator {
            return allocator
                .allocate()
                .await
                .map(|id| id.to_string())
                .map_err(|_| IdentityError::Allocation);
        }
        // Transitional compatibility only; the new runtime always injects its durable allocator.
        Ok(connection
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_auth.next_snowflake() AS id",
            ))
            .await?
            .ok_or(IdentityError::Allocation)?
            .try_get("", "id")?)
    }
    pub fn with_secret_keys(mut self, keys: SecretKeys) -> Self {
        self.keys = Some(keys);
        self
    }
    pub fn clear_mfa_cookie(&self) -> String {
        self.cookies.clear_challenge_header()
    }
    pub fn clear_cookie(&self) -> String {
        self.cookies.clear_header()
    }
    pub async fn authenticate_cookie(&self, header: &str) -> Result<Option<Principal>> {
        let Some(token) = self.cookies.read(header)? else {
            return Ok(None);
        };
        if let Some(backend) = &self.native_sessions {
            return native_session::authenticate(&backend.pool, &token).await;
        }
        let tx = self.db.begin().await?;
        let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT u.id,u.name,u.email,u.role,u.\"emailVerified\",u.\"mustChangePassword\",coalesce(u.\"twoFactorEnabled\",false) AS \"twoFactorEnabled\",s.id AS session_id,s.\"expiresAt\",webdock_rls.identity_has_preview(u.id,s.id) AS preview,webdock_rls.identity_is_acting(u.id,s.id) AS acting,webdock_rls.identity_customer_scope(u.id,s.id) AS customer_scope FROM webdock_auth.\"user\" u JOIN webdock_auth.session s ON s.\"userId\"=u.id WHERE s.token=$1 AND s.\"expiresAt\">statement_timestamp() AND NOT coalesce(u.banned,false)", [token.into()])).await?;
        let principal = if let Some(row) = row {
            let p = principal(&row)?;
            if externally_managed(&tx, &p.user_id).await?
                || !self
                    .provider_session_live(&tx, &p.user_id, &p.session_id)
                    .await?
            {
                None
            } else {
                Some(p)
            }
        } else {
            None
        };
        tx.commit().await?;
        Ok(principal)
    }
    /// Authority inspection for trusted administration/migration services, not proof of login.
    pub async fn local_authority_in(
        &self,
        tx: &DatabaseTransaction,
        subject: &str,
    ) -> Result<bool> {
        Ok(!externally_managed(tx, subject).await?)
    }
    /// Trusted backend only: validates an existing canonical session inside the caller's transaction.
    /// A subject/session pair supplied by an HTTP caller is not itself authentication proof.
    pub async fn authenticate_session_in(
        &self,
        tx: &DatabaseTransaction,
        subject: &str,
        session_id: &str,
    ) -> Result<Option<Principal>> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT u.id,u.name,u.email,u.role,u.\"emailVerified\",u.\"mustChangePassword\",coalesce(u.\"twoFactorEnabled\",false) AS \"twoFactorEnabled\",s.id AS session_id,s.\"expiresAt\",webdock_rls.identity_has_preview(u.id,s.id) AS preview,webdock_rls.identity_is_acting(u.id,s.id) AS acting,webdock_rls.identity_customer_scope(u.id,s.id) AS customer_scope FROM webdock_auth.\"user\" u JOIN webdock_auth.session s ON s.\"userId\"=u.id WHERE u.id=$1 AND s.id=$2 AND s.\"expiresAt\">statement_timestamp() AND NOT coalesce(u.banned,false) FOR SHARE OF u,s",[subject.into(),session_id.into()])).await?;
        let Some(row) = row else { return Ok(None) };
        let current = principal(&row)?;
        if externally_managed(tx, &current.user_id).await?
            || !self
                .provider_session_live(tx, &current.user_id, &current.session_id)
                .await?
        {
            return Ok(None);
        }
        Ok(Some(current))
    }
    pub async fn authenticate_password(
        &self,
        email: &str,
        password: &str,
    ) -> Result<PasswordAuthentication> {
        if email.len() > 254 || password.len() > 4096 {
            return Ok(PasswordAuthentication::Invalid);
        }
        let email = email.trim().to_lowercase();
        let key = format!(
            "identity:password:{}",
            hex::encode(Sha256::digest(email.as_bytes()))
        );
        let rate = self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.\"rateLimit\"(id,key,count,\"lastRequest\") VALUES($2,$1,1,floor(extract(epoch FROM statement_timestamp())*1000)::bigint) ON CONFLICT(key) DO UPDATE SET count=CASE WHEN webdock_auth.\"rateLimit\".\"lastRequest\"<=floor(extract(epoch FROM statement_timestamp())*1000)::bigint-60000 THEN 1 ELSE webdock_auth.\"rateLimit\".count+1 END,\"lastRequest\"=CASE WHEN webdock_auth.\"rateLimit\".\"lastRequest\"<=floor(extract(epoch FROM statement_timestamp())*1000)::bigint-60000 THEN floor(extract(epoch FROM statement_timestamp())*1000)::bigint ELSE webdock_auth.\"rateLimit\".\"lastRequest\" END RETURNING count", [key.into(),self.allocate_id(&self.db).await?.into()])).await?.ok_or(IdentityError::InvalidInput)?;
        if rate.try_get::<i32>("", "count")? > 10 {
            return Err(IdentityError::RateLimited);
        }
        if let Some(mapping) = self.neon_mapping_email(&email).await? {
            return self
                .authenticate_neon_password(&email, password, mapping)
                .await;
        }
        let rows = self.db.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT a.password FROM webdock_auth.\"user\" u JOIN webdock_auth.account a ON a.\"userId\"=u.id AND a.\"providerId\"='credential' WHERE lower(u.email)=$1 LIMIT 2", [email.clone().into()])).await?;
        let hash = if rows.len() == 1 {
            rows[0].try_get::<Option<String>>("", "password")?
        } else {
            None
        };
        // Equal-cost missing-account verification avoids a cheap account-existence oracle.
        let candidate = hash.clone().unwrap_or_else(|| "000102030405060708090a0b0c0d0e0f:c7877280641a617ec087f9365d3ad7facccc9681e2af94d0a148b29a67b413ab0e08c7f08f9bd6232c369f5bbd3eec03acc2a170c24fd179aecc67c282756d12".to_owned());
        let password_to_verify = password.to_owned();
        let permit = self
            .password_workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| IdentityError::Worker)?;
        let valid = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            verify_password(&password_to_verify, &candidate)
        })
        .await
        .map_err(|_| IdentityError::Worker)??;
        if !valid || hash.is_none() {
            return Ok(PasswordAuthentication::Invalid);
        }
        let tx = self.db.begin().await?;
        // Recheck the credential under locks so reset/ban cannot race session issuance.
        let rows = tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT u.id,u.\"emailVerified\",u.\"mustChangePassword\",coalesce(u.\"twoFactorEnabled\",false) AS mfa,u.role FROM webdock_auth.\"user\" u JOIN webdock_auth.account a ON a.\"userId\"=u.id AND a.\"providerId\"='credential' WHERE lower(u.email)=$1 AND a.password=$2 AND NOT coalesce(u.banned,false) LIMIT 2 FOR UPDATE OF u,a", [email.clone().into(),hash.clone().unwrap().into()])).await?;
        if rows.len() != 1 {
            return Ok(PasswordAuthentication::Invalid);
        }
        let row = &rows[0];
        let id: String = row.try_get("", "id")?;
        if externally_managed(&tx, &id).await? {
            return Ok(PasswordAuthentication::Invalid);
        }
        let verified: bool = row.try_get("", "emailVerified")?;
        let must_change: bool = row.try_get("", "mustChangePassword")?;
        let mfa: bool = row.try_get("", "mfa")?;
        let role: Option<String> = row.try_get("", "role")?;
        if !verified {
            tx.commit().await?;
            return Ok(PasswordAuthentication::SetupRequired {
                user_id: id,
                session: None,
            });
        }
        if ![Some("user"), Some("operator")].contains(&role.as_deref()) {
            return Ok(PasswordAuthentication::Invalid);
        }
        if self.neon_required(&tx).await? {
            return self.enroll_neon_password(tx, &id, &email, password).await;
        }
        if mfa {
            let set_cookie = self
                .issue_mfa_challenge(&tx, &id, hash.as_deref().unwrap())
                .await?;
            tx.commit().await?;
            return Ok(PasswordAuthentication::MfaRequired {
                user_id: id,
                set_cookie,
            });
        }
        let session = self.issue_session(tx, &id).await?;
        if must_change || role.as_deref() == Some("operator") {
            Ok(PasswordAuthentication::SetupRequired {
                user_id: id,
                session: Some(session),
            })
        } else {
            Ok(PasswordAuthentication::Authenticated(session))
        }
    }
    pub(crate) async fn issue_session(
        &self,
        tx: DatabaseTransaction,
        id: &str,
    ) -> Result<IssuedSession> {
        self.issue_session_with_source(tx, id, None).await
    }
    pub(crate) async fn issue_session_with_source(
        &self,
        tx: DatabaseTransaction,
        id: &str,
        source: Option<crate::neon::NeonSource>,
    ) -> Result<IssuedSession> {
        if source.is_none()
            && (self.neon_mapping(&tx, id).await?.is_some() || self.neon_required(&tx).await?)
        {
            return Err(IdentityError::SecurityState);
        }
        if source.as_ref().is_some_and(|s| s.user != id) {
            return Err(IdentityError::Unauthorized);
        }
        if let Some(source) = &source {
            let required=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT coalesce(\"twoFactorEnabled\",false) AS required FROM webdock_auth.\"user\" WHERE id=$1",[id.into()])).await?.ok_or(IdentityError::Unauthorized)?.try_get::<bool>("","required")?;
            if required && !source.mfa_verified {
                return Err(IdentityError::SecurityState);
            }
        }
        let session_id = self.allocate_id(&tx).await?;
        let token = hex::encode(random::<32>()?);
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($3,$1,$2,statement_timestamp()+interval '8 hours',statement_timestamp())", [token.clone().into(), id.into(),session_id.clone().into()])).await?;
        if let Some(source) = &source {
            self.store_neon_source(&tx, &session_id, source).await?;
        }
        let set_cookie = self.cookies.header(&token, 28800)?;
        tx.commit().await?;
        let principal = self
            .authenticate_cookie(set_cookie.split(';').next().expect("cookie has name"))
            .await?
            .ok_or(IdentityError::Unauthorized)?;
        Ok(IssuedSession {
            principal,
            set_cookie,
        })
    }
    /// A session only an API token reaches: its own token is random and never leaves this call,
    /// so no cookie can carry it. Returns the session id.
    pub async fn issue_api_session(&self, user_id: &str, label: &str, days: i64) -> Result<String> {
        if !(1..=366).contains(&days) || label.is_empty() || label.len() > 120 {
            return Err(IdentityError::InvalidInput);
        }
        if self.native_management_pool().is_some() {
            return self.native_issue_api_session(user_id, label, days).await;
        }
        let tx = self.db.begin().await?;
        let id = self.allocate_id(&tx).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\",\"userAgent\") VALUES($1,$2,$3,statement_timestamp()+make_interval(days=>$4::int),statement_timestamp(),$5)",
            [id.clone().into(), hex::encode(random::<32>()?).into(), user_id.into(), (days as i32).into(), label.into()])).await?;
        tx.commit().await?;
        Ok(id)
    }
    /// Trusted backend only: callers must already authorize the user/session operation.
    pub async fn revoke_session(&self, user_id: &str, session_id: &str) -> Result<bool> {
        if self.native_management_pool().is_some() {
            return self.native_revoke_session(user_id, session_id).await;
        }

        Ok(self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.session WHERE id=$1 AND \"userId\"=$2",
                [session_id.into(), user_id.into()],
            ))
            .await?
            .rows_affected()
            == 1)
    }
    /// Offline/invitation provisioning only; callers must authorize the invitation separately.
    /// New identities cannot access business APIs until email/password setup is completed.
    pub async fn create_local_account(
        &self,
        email: &str,
        name: &str,
        password: &str,
        operator: bool,
    ) -> Result<String> {
        let email = email.trim().to_lowercase();
        let name = name.trim();
        if email.len() > 254
            || !email.contains('@')
            || email
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
            || name.is_empty()
            || name.len() > 160
        {
            return Err(IdentityError::InvalidInput);
        }
        let password = password.to_owned();
        let hash = tokio::task::spawn_blocking(move || hash_password(&password))
            .await
            .map_err(|_| IdentityError::Worker)??;
        let tx = self.db.begin().await?;
        let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($4,$1,$2,false,$3,false,true) RETURNING id", [name.into(),email.into(),if operator { "operator" } else { "user" }.into(),self.allocate_id(&tx).await?.into()])).await?.ok_or(IdentityError::InvalidInput)?;
        let id: String = row.try_get("", "id")?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.account(id,\"accountId\",\"providerId\",\"userId\",password,\"updatedAt\") VALUES($3,$1,'credential',$1,$2,statement_timestamp())", [id.clone().into(),hash.into(),self.allocate_id(&tx).await?.into()])).await?;
        tx.commit().await?;
        Ok(id)
    }
}
fn principal(row: &QueryResult) -> Result<Principal> {
    Ok(Principal {
        user_id: row.try_get("", "id")?,
        session_id: row.try_get("", "session_id")?,
        name: row.try_get("", "name")?,
        email: row.try_get("", "email")?,
        expires_at: row.try_get("", "expiresAt")?,
        role: row
            .try_get::<Option<String>>("", "role")?
            .unwrap_or_default(),
        email_verified: row.try_get("", "emailVerified")?,
        must_change_password: row.try_get("", "mustChangePassword")?,
        two_factor_enabled: row.try_get("", "twoFactorEnabled")?,
        tenant_preview: row.try_get("", "preview")?,
        tenant_acting: row.try_get("", "acting")?,
        customer_scope: row.try_get("", "customer_scope")?,
    })
}
pub(crate) async fn externally_managed(tx: &DatabaseTransaction, user_id: &str) -> Result<bool> {
    // Retained mappings are migration evidence, never grants. Audited cutover must retire them.
    let approved=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT to_regprocedure('webdock_rls.local_identity_authority(text)') IS NOT NULL AS available")).await?.ok_or(IdentityError::InvalidInput)?.try_get::<bool>("","available")?;
    if approved {
        return Ok(!tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.local_identity_authority($1) AS allowed",
                [user_id.into()],
            ))
            .await?
            .ok_or(IdentityError::InvalidInput)?
            .try_get::<bool>("", "allowed")?);
    }
    for (table, column) in [
        ("clerk_identity", "user_id"),
        ("clerk_identity_mapping", "local_subject"),
    ] {
        let present: bool = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT to_regclass($1) IS NOT NULL AS present",
                [format!("webdock_auth.{table}").into()],
            ))
            .await?
            .ok_or(IdentityError::InvalidInput)?
            .try_get("", "present")?;
        if present
            && tx
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    format!(
                        "SELECT 1 AS present FROM webdock_auth.{table} WHERE {column}=$1 LIMIT 1"
                    ),
                    [user_id.into()],
                ))
                .await?
                .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
