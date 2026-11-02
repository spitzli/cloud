//! Session-bound Managed first factor; canonical identities and second factors remain Core-owned.
use crate::neon_client::NeonLogin;
use crate::{
    IdentityError, IdentityService, PasswordAuthentication, Result,
    security::{credential, digest},
};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct NeonSource {
    pub(crate) user: String,
    pub(crate) subject: String,
    pub(crate) session: String,
    pub(crate) revision: i32,
    pub(crate) credential_digest: String,
    pub(crate) encrypted_cookie: String,
    pub(crate) mfa_verified: bool,
}
#[derive(Serialize, Deserialize)]
struct CookieEnvelope {
    user: String,
    subject: String,
    session: String,
    cookie: String,
}
pub(crate) struct NeonMapping {
    pub(crate) user: String,
    pub(crate) subject: String,
    pub(crate) issuer: String,
    pub(crate) revision: i32,
    pub(crate) enabled: bool,
}
impl IdentityService {
    pub(crate) async fn neon_required(&self, tx: &DatabaseTransaction) -> Result<bool> {
        let present = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT to_regprocedure('webdock_rls.neon_required()') IS NOT NULL AS present",
            ))
            .await?
            .ok_or(IdentityError::Configuration)?
            .try_get::<bool>("", "present")?;
        if !present {
            return Ok(false);
        }
        Ok(tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.neon_required() AS required",
            ))
            .await?
            .ok_or(IdentityError::Configuration)?
            .try_get("", "required")?)
    }
    pub fn with_neon_auth(mut self, client: crate::NeonAuthClient) -> Self {
        self.neon = Some(client);
        self
    }
    async fn has_neon_gate(&self, tx: &DatabaseTransaction) -> Result<bool> {
        if self.neon_gate.get() == Some(&true) {
            return Ok(true);
        }
        let present=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT to_regprocedure('webdock_rls.native_provider_session_live(text,text)') IS NOT NULL AS present")).await?.ok_or(IdentityError::Configuration)?.try_get::<bool>("","present")?;
        if present {
            let _ = self.neon_gate.set(true);
        }
        Ok(present)
    }
    pub(crate) async fn provider_session_live(
        &self,
        tx: &DatabaseTransaction,
        user: &str,
        session: &str,
    ) -> Result<bool> {
        if !self.has_neon_gate(tx).await? {
            return Ok(true);
        }
        Ok(tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.native_provider_session_live($1,$2) AS live",
                [user.into(), session.into()],
            ))
            .await?
            .ok_or(IdentityError::Configuration)?
            .try_get("", "live")?)
    }
    pub(crate) async fn neon_mapping(
        &self,
        tx: &DatabaseTransaction,
        user: &str,
    ) -> Result<Option<NeonMapping>> {
        if !self.has_neon_gate(tx).await? {
            return Ok(None);
        }
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT user_id,provider_subject::text,provider_origin,revision,enabled FROM webdock_auth.neon_identity WHERE user_id=$1",[user.into()])).await?;
        row.map(|r| {
            Ok(NeonMapping {
                user: r.try_get("", "user_id")?,
                subject: r.try_get("", "provider_subject")?,
                issuer: r.try_get("", "provider_origin")?,
                revision: r.try_get("", "revision")?,
                enabled: r.try_get("", "enabled")?,
            })
        })
        .transpose()
    }
    pub(crate) async fn neon_mapping_email(&self, email: &str) -> Result<Option<NeonMapping>> {
        let tx = self.db.begin().await?;
        if !self.has_neon_gate(&tx).await? {
            return Ok(None);
        }
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.\"user\" WHERE lower(email)=$1",
                [email.into()],
            ))
            .await?;
        let found = if let Some(row) = row {
            self.neon_mapping(&tx, &row.try_get::<String>("", "id")?)
                .await?
        } else {
            None
        };
        tx.commit().await?;
        Ok(found)
    }
    async fn source_from_login(
        &self,
        tx: &DatabaseTransaction,
        user: &str,
        mapping: &NeonMapping,
        login: NeonLogin,
    ) -> Result<(NeonSource, String)> {
        if !mapping.enabled
            || login.subject != mapping.subject
            || self
                .neon
                .as_ref()
                .is_none_or(|c| c.issuer() != mapping.issuer)
        {
            return Err(IdentityError::Unauthorized);
        }
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.neon_session_facts($1) AS facts",
                [login.token.into()],
            ))
            .await?
            .ok_or(IdentityError::Unauthorized)?;
        let facts: Option<Value> = row.try_get("", "facts")?;
        let facts = facts.ok_or(IdentityError::Unauthorized)?;
        let field = |key: &str| {
            facts[key]
                .as_str()
                .map(str::to_owned)
                .ok_or(IdentityError::InvalidEncoding)
        };
        if field("subject")? != mapping.subject {
            return Err(IdentityError::Unauthorized);
        }
        let session = field("sessionID")?;
        let hash = field("passwordHash")?;
        let encrypted_cookie = self
            .keys
            .as_ref()
            .ok_or(IdentityError::Configuration)?
            .encrypt(
                &serde_json::to_string(&CookieEnvelope {
                    user: user.into(),
                    subject: mapping.subject.clone(),
                    session: session.clone(),
                    cookie: login.cookie,
                })
                .map_err(|_| IdentityError::InvalidEncoding)?,
            )?;
        Ok((
            NeonSource {
                user: user.into(),
                subject: mapping.subject.clone(),
                session,
                revision: mapping.revision,
                credential_digest: digest(&hash),
                encrypted_cookie,
                mfa_verified: false,
            },
            hash,
        ))
    }
    pub(crate) async fn source_live(
        &self,
        tx: &DatabaseTransaction,
        source: &NeonSource,
    ) -> Result<bool> {
        Ok(tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.neon_binding_live($1,$2::uuid,$3::uuid,$4,$5) AS live",
                [
                    source.user.clone().into(),
                    source.session.clone().into(),
                    source.subject.clone().into(),
                    source.revision.into(),
                    source.credential_digest.clone().into(),
                ],
            ))
            .await?
            .ok_or(IdentityError::Configuration)?
            .try_get("", "live")?)
    }
    pub(crate) async fn session_source(
        &self,
        tx: &DatabaseTransaction,
        user: &str,
        session: &str,
    ) -> Result<Option<NeonSource>> {
        if self.neon_mapping(tx, user).await?.is_none() {
            return Ok(None);
        }
        if !self.provider_session_live(tx, user, session).await? {
            return Err(IdentityError::Unauthorized);
        }
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT user_id,provider_subject::text,provider_session_id::text,identity_revision,credential_digest,encrypted_cookie,mfa_verified FROM webdock_auth.neon_session WHERE session_id=$1 AND user_id=$2",[session.into(),user.into()])).await?.ok_or(IdentityError::Unauthorized)?;
        Ok(Some(NeonSource {
            user: row.try_get("", "user_id")?,
            subject: row.try_get("", "provider_subject")?,
            session: row.try_get("", "provider_session_id")?,
            revision: row.try_get("", "identity_revision")?,
            credential_digest: row.try_get("", "credential_digest")?,
            encrypted_cookie: row.try_get("", "encrypted_cookie")?,
            mfa_verified: row.try_get("", "mfa_verified")?,
        }))
    }
    pub(crate) async fn pending_neon(
        &self,
        tx: &DatabaseTransaction,
        user: &str,
        cookie: &str,
    ) -> Result<Option<NeonSource>> {
        if self.neon_mapping(tx, user).await?.is_none() {
            return Ok(None);
        }
        let token = self
            .cookies
            .read_challenge(cookie)?
            .ok_or(IdentityError::Unauthorized)?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT n.user_id,n.provider_subject::text,n.provider_session_id::text,n.identity_revision,n.credential_digest,n.encrypted_cookie FROM webdock_auth.neon_mfa_challenge n JOIN webdock_auth.identity_mfa_challenge c ON c.token_hash=n.token_hash WHERE n.token_hash=$1 AND n.user_id=$2 AND c.expires_at>statement_timestamp() AND c.attempts<5 FOR UPDATE OF n,c",[digest(&token).into(),user.into()])).await?.ok_or(IdentityError::Unauthorized)?;
        let source = NeonSource {
            user: row.try_get("", "user_id")?,
            subject: row.try_get("", "provider_subject")?,
            session: row.try_get("", "provider_session_id")?,
            revision: row.try_get("", "identity_revision")?,
            credential_digest: row.try_get("", "credential_digest")?,
            encrypted_cookie: row.try_get("", "encrypted_cookie")?,
            mfa_verified: false,
        };
        if !self.source_live(tx, &source).await? {
            return Err(IdentityError::Unauthorized);
        }
        Ok(Some(source))
    }
    pub(crate) async fn authenticate_neon_password(
        &self,
        email: &str,
        password: &str,
        mapping: NeonMapping,
    ) -> Result<PasswordAuthentication> {
        if !mapping.enabled {
            return Ok(PasswordAuthentication::Invalid);
        }
        let login = self
            .neon
            .as_ref()
            .ok_or(IdentityError::Configuration)?
            .sign_in(email, password)
            .await?;
        let tx = self.db.begin().await?;
        self.complete_neon_login(tx, email, mapping, login).await
    }
    pub(crate) async fn enroll_neon_password(
        &self,
        tx: DatabaseTransaction,
        user: &str,
        email: &str,
        password: &str,
    ) -> Result<PasswordAuthentication> {
        let client = self.neon.as_ref().ok_or(IdentityError::Configuration)?;
        self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        let name: String = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT name FROM webdock_auth.\"user\" WHERE id=$1",
                [user.into()],
            ))
            .await?
            .ok_or(IdentityError::Unauthorized)?
            .try_get("", "name")?;
        // The caller holds exact native credential/approval locks. Never adopt an existing provider user.
        let login = client.sign_up(email, password, &name).await?;
        let mapping = NeonMapping {
            user: user.into(),
            subject: login.subject.clone(),
            issuer: client.issuer().into(),
            revision: 1,
            enabled: true,
        };
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES($1,$2,$3::uuid,$1)",
            [user.into(),client.issuer().into(),mapping.subject.clone().into()])).await?;
        self.complete_neon_login(tx, email, mapping, login).await
    }
    async fn complete_neon_login(
        &self,
        tx: DatabaseTransaction,
        email: &str,
        mapping: NeonMapping,
        login: NeonLogin,
    ) -> Result<PasswordAuthentication> {
        let (source, hash) = self
            .source_from_login(&tx, &mapping.user, &mapping, login)
            .await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,role,\"emailVerified\",\"mustChangePassword\",coalesce(\"twoFactorEnabled\",false) AS mfa FROM webdock_auth.\"user\" WHERE id=$1 AND lower(email)=$2 AND NOT coalesce(banned,false) FOR UPDATE",[mapping.user.clone().into(),email.into()])).await?.ok_or(IdentityError::Unauthorized)?;
        let role: Option<String> = row.try_get("", "role")?;
        if !row.try_get::<bool>("", "emailVerified")?
            || ![Some("user"), Some("operator")].contains(&role.as_deref())
            || crate::service::externally_managed(&tx, &mapping.user).await?
        {
            return Err(IdentityError::SecurityState);
        }
        let changed=tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.account SET password=$1,\"updatedAt\"=statement_timestamp() WHERE \"userId\"=$2 AND \"providerId\"='credential'",[hash.clone().into(),mapping.user.clone().into()])).await?;
        if changed.rows_affected() != 1 {
            return Err(IdentityError::SecurityState);
        }
        if row.try_get::<bool>("", "mfa")? {
            let set_cookie = self.issue_mfa_challenge(&tx, &mapping.user, &hash).await?;
            let token = self
                .cookies
                .read_challenge(
                    set_cookie
                        .split(';')
                        .next()
                        .ok_or(IdentityError::InvalidEncoding)?,
                )?
                .ok_or(IdentityError::InvalidEncoding)?;
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.neon_mfa_challenge(token_hash,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie) VALUES($1,$2,$3::uuid,$4::uuid,$5,$6,$7)",[digest(&token).into(),source.user.clone().into(),source.session.clone().into(),source.subject.clone().into(),source.revision.into(),source.credential_digest.clone().into(),source.encrypted_cookie.clone().into()])).await?;
            tx.commit().await?;
            return Ok(PasswordAuthentication::MfaRequired {
                user_id: mapping.user,
                set_cookie,
            });
        }
        let setup =
            role.as_deref() == Some("operator") || row.try_get::<bool>("", "mustChangePassword")?;
        let session = self
            .issue_session_with_source(tx, &mapping.user, Some(source))
            .await?;
        if setup {
            Ok(PasswordAuthentication::SetupRequired {
                user_id: mapping.user,
                session: Some(session),
            })
        } else {
            Ok(PasswordAuthentication::Authenticated(session))
        }
    }
    /// Requires the current native account password and recent complete native identity proof.
    pub async fn link_neon_identity(&self, cookie: &str, password: &str) -> Result<()> {
        let client = self.neon.as_ref().ok_or(IdentityError::Configuration)?;
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        if !actor.verified
            || actor.must_change
            || actor.preview
            || (actor.role == "operator" && !actor.mfa)
            || self.neon_mapping(&tx, &actor.id).await?.is_some()
        {
            return Err(IdentityError::SecurityState);
        }
        self.verify_current_password(password, &credential(&tx, &actor.id).await?)
            .await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT u.name FROM webdock_auth.\"user\" u JOIN webdock_auth.session s ON s.\"userId\"=u.id WHERE u.id=$1 AND s.id=$2 AND s.\"createdAt\">statement_timestamp()-interval '5 minutes'",[actor.id.clone().into(),actor.session_id.clone().into()])).await?.ok_or(IdentityError::SecurityState)?;
        let name: String = row.try_get("", "name")?;
        tx.commit().await?;
        let login = match client.sign_in(&actor.email, password).await {
            Ok(login) => login,
            Err(IdentityError::Unauthorized) => {
                client.sign_up(&actor.email, password, &name).await?
            }
            Err(error) => return Err(error),
        };
        let tx = self.db.begin().await?;
        let current = self.locked_actor(&tx, cookie).await?;
        if current.id != actor.id
            || current.session_id != actor.session_id
            || current.preview
            || current.role != actor.role
            || current.mfa != actor.mfa
        {
            return Err(IdentityError::Unauthorized);
        }
        let mapping = NeonMapping {
            user: actor.id.clone(),
            subject: login.subject.clone(),
            issuer: client.issuer().into(),
            revision: 1,
            enabled: true,
        };
        let (mut source, hash) = self
            .source_from_login(&tx, &actor.id, &mapping, login)
            .await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES($1,$2,$3::uuid,$1)",[actor.id.clone().into(),client.issuer().into(),source.subject.clone().into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.account SET password=$1,\"updatedAt\"=statement_timestamp() WHERE \"userId\"=$2 AND \"providerId\"='credential'",[hash.into(),actor.id.clone().into()])).await?;
        source.mfa_verified = actor.mfa;
        self.store_neon_source(&tx, &actor.session_id, &source)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn store_neon_source(
        &self,
        tx: &DatabaseTransaction,
        session: &str,
        source: &NeonSource,
    ) -> Result<()> {
        if !self.source_live(tx, source).await? {
            return Err(IdentityError::Unauthorized);
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.neon_session(session_id,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie,mfa_verified) VALUES($1,$2,$3::uuid,$4::uuid,$5,$6,$7,$8)",[session.into(),source.user.clone().into(),source.session.clone().into(),source.subject.clone().into(),source.revision.into(),source.credential_digest.clone().into(),source.encrypted_cookie.clone().into(),source.mfa_verified.into()])).await?;
        Ok(())
    }
    pub(crate) fn provider_cookie(&self, source: &NeonSource) -> Result<String> {
        let data = self
            .keys
            .as_ref()
            .ok_or(IdentityError::Configuration)?
            .decrypt(&source.encrypted_cookie)?;
        let envelope: CookieEnvelope =
            serde_json::from_str(&data).map_err(|_| IdentityError::InvalidEncoding)?;
        if envelope.user != source.user
            || envelope.subject != source.subject
            || envelope.session != source.session
        {
            return Err(IdentityError::InvalidEncoding);
        }
        Ok(envelope.cookie)
    }
}

impl IdentityService {
    /// Change the Managed credential, revoke Core sessions, and require a fresh first factor + MFA.
    pub async fn change_managed_password(
        &self,
        cookie: &str,
        current: &str,
        new: &str,
    ) -> Result<bool> {
        if !(12..=4096).contains(&new.len()) {
            return Err(IdentityError::InvalidInput);
        }
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        let Some(source) = self
            .session_source(&tx, &actor.id, &actor.session_id)
            .await?
        else {
            return Ok(false);
        };
        if !actor.verified || actor.preview {
            return Err(IdentityError::SecurityState);
        }
        self.verify_current_password(current, &credential(&tx, &actor.id).await?)
            .await?;
        let provider_cookie = self.provider_cookie(&source)?;
        tx.commit().await?;
        self.neon
            .as_ref()
            .ok_or(IdentityError::Configuration)?
            .change_password(&provider_cookie, current, new)
            .await?;
        let tx = self.db.begin().await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.session WHERE \"userId\"=$1",
            [source.user.into()],
        ))
        .await?;
        tx.commit().await?;
        Ok(true)
    }
    pub async fn reset_managed_password(&self, token: &str, new: &str) -> Result<bool> {
        let Some(client) = &self.neon else {
            return Ok(false);
        };
        if !(16..=4096).contains(&token.len()) || !(12..=4096).contains(&new.len()) {
            return Err(IdentityError::InvalidInput);
        }
        client.reset_password(token, new).await?;
        Ok(true)
    }
    /// Sign-out always revokes the local cookie, even when the provider is unavailable.
    pub async fn logout(&self, cookie: &str) -> Result<()> {
        let Some(token) = self.cookies.read(cookie)? else {
            return Ok(());
        };
        let tx = self.db.begin().await?;
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id,\"userId\" FROM webdock_auth.session WHERE token=$1 FOR UPDATE",
                [token.into()],
            ))
            .await?;
        let mut provider_cookie = None;
        if let Some(row) = row {
            let user: String = row.try_get("", "userId")?;
            let session: String = row.try_get("", "id")?;
            if self.has_neon_gate(&tx).await?
                && let Some(p)=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT provider_subject::text,provider_session_id::text,encrypted_cookie FROM webdock_auth.neon_session WHERE session_id=$1 AND user_id=$2",[session.clone().into(),user.clone().into()])).await?{
     let envelope=self.keys.as_ref().ok_or(IdentityError::Configuration)?.decrypt(&p.try_get::<String>("","encrypted_cookie")?)?;let envelope:CookieEnvelope=serde_json::from_str(&envelope).map_err(|_|IdentityError::InvalidEncoding)?;
     if envelope.user!=user||envelope.subject!=p.try_get::<String>("","provider_subject")?||envelope.session!=p.try_get::<String>("","provider_session_id")?{return Err(IdentityError::InvalidEncoding)}provider_cookie=Some(envelope.cookie);
    }

            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.session WHERE id=$1 AND \"userId\"=$2",
                [session.into(), user.into()],
            ))
            .await?;
        }
        tx.commit().await?;
        if let (Some(client), Some(cookie)) = (&self.neon, provider_cookie) {
            let _ = client.sign_out(&cookie).await;
        }
        Ok(())
    }
}
