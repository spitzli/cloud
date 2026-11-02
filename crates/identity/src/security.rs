use crate::{
    IdentityError, IdentityService, IssuedSession, Result,
    crypto::{hash_password, random, verify_password},
    service::externally_managed,
};
use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, QueryResult, Statement, TransactionTrait,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Proof data is deliberately not Debug/Serialize.
pub enum MfaProof {
    Totp(String),
    RecoveryCode(String),
}
/// Display only to the authenticated enrolling account; never log this value.
pub struct TotpEnrollment {
    pub totp_uri: String,
    pub backup_codes: Vec<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub current: bool,
}
pub(crate) struct Actor {
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) email: String,
    pub(crate) role: String,
    pub(crate) verified: bool,
    pub(crate) must_change: bool,
    pub(crate) mfa: bool,
    pub(crate) preview: bool,
}
pub(crate) fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn totp(secret: &str, email: &str) -> Result<totp_rs::Totp> {
    // Retained secrets are raw UTF-8 key material; the otpauth URL encodes those bytes as base32.
    totp_rs::Builder::new()
        .with_secret(secret.as_bytes().to_vec())
        .with_algorithm(totp_rs::Algorithm::SHA1)
        .with_digits(6)
        .with_skew(1)
        .with_step_duration(30)
        .with_issuer(Some("Spitzli Cloud"))
        .with_account_name(email)
        .build()
        .map_err(|_| IdentityError::InvalidEncoding)
}
impl IdentityService {
    pub(crate) async fn issue_mfa_challenge(
        &self,
        tx: &DatabaseTransaction,
        id: &str,
        credential: &str,
    ) -> Result<String> {
        let token = hex::encode(random::<32>()?);
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.identity_mfa_challenge WHERE user_id=$1 AND expires_at<=statement_timestamp()",[id.into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_mfa_challenge(token_hash,user_id,credential_digest,expires_at) VALUES($1,$2,$3,statement_timestamp()+interval '5 minutes')",[digest(&token).into(),id.into(),digest(credential).into()])).await?;
        self.cookies.challenge_header(&token)
    }
    pub(crate) async fn verify_current_password(
        &self,
        password: &str,
        encoded: &str,
    ) -> Result<()> {
        let password = password.to_owned();
        let encoded = encoded.to_owned();
        let permit = self
            .password_workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| IdentityError::Worker)?;
        if !tokio::task::spawn_blocking(move || {
            let _permit = permit;
            verify_password(&password, &encoded)
        })
        .await
        .map_err(|_| IdentityError::Worker)??
        {
            return Err(IdentityError::Unauthorized);
        }
        Ok(())
    }
    pub(crate) async fn locked_actor(
        &self,
        tx: &DatabaseTransaction,
        cookie: &str,
    ) -> Result<Actor> {
        let token = self
            .cookies
            .read(cookie)?
            .ok_or(IdentityError::Unauthorized)?;
        let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT u.id,u.email,u.role,u.\"emailVerified\",u.\"mustChangePassword\",coalesce(u.\"twoFactorEnabled\",false) AS mfa,s.id AS session_id,webdock_rls.identity_has_preview(u.id,s.id) AS preview FROM webdock_auth.\"user\" u JOIN webdock_auth.session s ON s.\"userId\"=u.id WHERE s.token=$1 AND s.\"expiresAt\">statement_timestamp() AND NOT coalesce(u.banned,false) AND u.role IN ('user','operator') FOR UPDATE OF u,s",[token.into()])).await?.ok_or(IdentityError::Unauthorized)?;
        let id: String = row.try_get("", "id")?;
        if externally_managed(tx, &id).await?
            || !self
                .provider_session_live(tx, &id, &row.try_get::<String>("", "session_id")?)
                .await?
        {
            return Err(IdentityError::Unauthorized);
        }
        Ok(Actor {
            id,
            session_id: row.try_get("", "session_id")?,
            email: row.try_get("", "email")?,
            role: row.try_get("", "role")?,
            verified: row.try_get("", "emailVerified")?,
            must_change: row.try_get("", "mustChangePassword")?,
            mfa: row.try_get("", "mfa")?,
            preview: row.try_get("", "preview")?,
        })
    }
    pub async fn begin_totp_enrollment(
        &self,
        cookie: &str,
        password: &str,
    ) -> Result<TotpEnrollment> {
        let keys = self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        if !actor.verified || actor.must_change || actor.mfa || actor.preview {
            return Err(IdentityError::SecurityState);
        }
        let credential = credential(&tx, &actor.id).await?;
        self.verify_current_password(password, &credential).await?;
        let existing = tx
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT verified FROM webdock_auth.\"twoFactor\" WHERE \"userId\"=$1 FOR UPDATE",
                [actor.id.clone().into()],
            ))
            .await?;
        if existing.len() > 1
            || existing.first().is_some_and(|row| {
                row.try_get::<Option<bool>>("", "verified").ok().flatten() != Some(false)
            })
        {
            return Err(IdentityError::SecurityState);
        }
        let secret = hex::encode(random::<20>()?);
        let backup_codes = (0..10)
            .map(|_| {
                let raw = hex::encode(random::<10>()?);
                Ok(format!("{}-{}", &raw[..10], &raw[10..]))
            })
            .collect::<Result<Vec<String>>>()?;
        let encrypted_secret = keys.encrypt(&secret)?;
        let encrypted_codes = keys.encrypt(
            &serde_json::to_string(&backup_codes).map_err(|_| IdentityError::InvalidEncoding)?,
        )?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.\"twoFactor\" WHERE \"userId\"=$1",
            [actor.id.clone().into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"twoFactor\"(id,secret,\"backupCodes\",\"userId\",verified,\"failedVerificationCount\") VALUES($4,$1,$2,$3,false,0)",[encrypted_secret.into(),encrypted_codes.into(),actor.id.clone().into(),self.allocate_id(&tx).await?.into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_mfa_enrollment(session_id,credential_digest,expires_at) VALUES($1,$2,statement_timestamp()+interval '15 minutes') ON CONFLICT(session_id) DO UPDATE SET credential_digest=excluded.credential_digest,expires_at=excluded.expires_at",[actor.session_id.into(),digest(&credential).into()])).await?;
        let totp_uri = totp(&secret, &actor.email)?
            .to_url()
            .map_err(|_| IdentityError::InvalidEncoding)?;
        tx.commit().await?;
        Ok(TotpEnrollment {
            totp_uri,
            backup_codes,
        })
    }
    pub async fn confirm_totp_enrollment(&self, cookie: &str, code: &str) -> Result<IssuedSession> {
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        if !actor.verified || actor.must_change || actor.mfa || actor.preview {
            return Err(IdentityError::SecurityState);
        }
        let credential = credential(&tx, &actor.id).await?;
        let enrollment=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT credential_digest FROM webdock_auth.identity_mfa_enrollment WHERE session_id=$1 AND expires_at>statement_timestamp() FOR UPDATE",[actor.session_id.clone().into()])).await?.ok_or(IdentityError::Unauthorized)?;
        if enrollment.try_get::<String>("", "credential_digest")? != digest(&credential) {
            return Err(IdentityError::Unauthorized);
        }
        let factor = factor(&tx, &actor.id).await?;
        if factor.try_get::<Option<bool>>("", "verified")? != Some(false) {
            return Err(IdentityError::SecurityState);
        }
        if !self
            .verify_factor(
                &tx,
                &actor.id,
                &actor.email,
                &factor,
                &MfaProof::Totp(code.to_owned()),
            )
            .await?
        {
            record_failure(&tx, &factor).await?;
            tx.commit().await?;
            return Err(IdentityError::InvalidCode);
        }
        let mut source = self
            .session_source(&tx, &actor.id, &actor.session_id)
            .await?;
        if let Some(source) = &mut source {
            source.mfa_verified = true;
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"twoFactor\" SET verified=true,\"failedVerificationCount\"=0,\"lockedUntil\"=NULL WHERE \"userId\"=$1",[actor.id.clone().into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"twoFactorEnabled\"=true,\"updatedAt\"=statement_timestamp() WHERE id=$1",[actor.id.clone().into()])).await?;
        invalidate_sessions(&tx, &actor.id, None).await?;
        self.issue_session_with_source(tx, &actor.id, source).await
    }
    pub async fn complete_mfa(&self, cookie: &str, proof: MfaProof) -> Result<IssuedSession> {
        let token = self
            .cookies
            .read_challenge(cookie)?
            .ok_or(IdentityError::Unauthorized)?;
        let token_hash = digest(&token);
        let tx = self.db.begin().await?;
        let lookup = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT user_id FROM webdock_auth.identity_mfa_challenge WHERE token_hash=$1",
                [token_hash.clone().into()],
            ))
            .await?
            .ok_or(IdentityError::Unauthorized)?;
        let id: String = lookup.try_get("", "user_id")?;
        let user=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT email FROM webdock_auth.\"user\" WHERE id=$1 AND NOT coalesce(banned,false) AND \"emailVerified\" AND \"twoFactorEnabled\" AND role IN ('user','operator') FOR UPDATE",[id.clone().into()])).await?.ok_or(IdentityError::Unauthorized)?;
        if externally_managed(&tx, &id).await? {
            return Err(IdentityError::Unauthorized);
        }
        let challenge=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT credential_digest FROM webdock_auth.identity_mfa_challenge WHERE token_hash=$1 AND user_id=$2 AND expires_at>statement_timestamp() AND attempts<5 FOR UPDATE",[token_hash.clone().into(),id.clone().into()])).await?.ok_or(IdentityError::Unauthorized)?;
        if challenge.try_get::<String>("", "credential_digest")?
            != digest(&credential(&tx, &id).await?)
        {
            return Err(IdentityError::Unauthorized);
        }
        let mut source = self.pending_neon(&tx, &id, cookie).await?;
        let factor = factor(&tx, &id).await?;
        if factor.try_get::<Option<bool>>("", "verified")? == Some(false) {
            return Err(IdentityError::SecurityState);
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.identity_mfa_challenge SET attempts=attempts+1 WHERE token_hash=$1",[token_hash.clone().into()])).await?;
        if !self
            .verify_factor(
                &tx,
                &id,
                &user.try_get::<String>("", "email")?,
                &factor,
                &proof,
            )
            .await?
        {
            record_failure(&tx, &factor).await?;
            tx.commit().await?;
            return Err(IdentityError::InvalidCode);
        }
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.identity_mfa_challenge WHERE token_hash=$1",
            [token_hash.into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"twoFactor\" SET \"failedVerificationCount\"=0,\"lockedUntil\"=NULL WHERE \"userId\"=$1",[id.clone().into()])).await?;
        if let Some(source) = &mut source {
            source.mfa_verified = true;
        }
        self.issue_session_with_source(tx, &id, source).await
    }
    async fn verify_factor(
        &self,
        tx: &DatabaseTransaction,
        id: &str,
        email: &str,
        factor: &QueryResult,
        proof: &MfaProof,
    ) -> Result<bool> {
        let keys = self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        if factor.try_get::<bool>("", "locked")? {
            return Err(IdentityError::RateLimited);
        }
        match proof {
            MfaProof::Totp(code) => {
                if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
                    return Ok(false);
                }
                let secret = keys.decrypt(&factor.try_get::<String>("", "secret")?)?;
                let timestamp: i64 = factor.try_get("", "timestamp")?;
                let Some(step) = totp(&secret, email)?.check(
                    code,
                    timestamp
                        .try_into()
                        .map_err(|_| IdentityError::InvalidInput)?,
                ) else {
                    return Ok(false);
                };
                let updated=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_totp_replay(user_id,secret_digest,last_step) VALUES($1,$2,$3) ON CONFLICT(user_id) DO UPDATE SET secret_digest=excluded.secret_digest,last_step=excluded.last_step WHERE webdock_auth.identity_totp_replay.secret_digest<>excluded.secret_digest OR webdock_auth.identity_totp_replay.last_step<excluded.last_step RETURNING user_id",[id.into(),digest(&secret).into(),(step as i64).into()])).await?;
                Ok(updated.is_some())
            }
            MfaProof::RecoveryCode(code) => {
                if code.len() > 128 {
                    return Ok(false);
                }
                let mut codes: Vec<String> = serde_json::from_str(
                    &keys.decrypt(&factor.try_get::<String>("", "backupCodes")?)?,
                )
                .map_err(|_| IdentityError::InvalidEncoding)?;
                let Some(index) = codes
                    .iter()
                    .position(|stored| bool::from(stored.as_bytes().ct_eq(code.as_bytes())))
                else {
                    return Ok(false);
                };
                codes.remove(index);
                let encrypted = keys.encrypt(
                    &serde_json::to_string(&codes).map_err(|_| IdentityError::InvalidEncoding)?,
                )?;
                tx.execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE webdock_auth.\"twoFactor\" SET \"backupCodes\"=$1 WHERE id=$2",
                    [encrypted.into(), factor.try_get::<String>("", "id")?.into()],
                ))
                .await?;
                Ok(true)
            }
        }
    }
    pub async fn change_password(
        &self,
        cookie: &str,
        current: &str,
        new: &str,
    ) -> Result<IssuedSession> {
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        if self.neon_mapping(&tx, &actor.id).await?.is_some() {
            return Err(IdentityError::SecurityState);
        }
        if !actor.verified || actor.preview {
            return Err(IdentityError::SecurityState);
        }
        let old = credential(&tx, &actor.id).await?;
        self.verify_current_password(current, &old).await?;
        let new = new.to_owned();
        let permit = self
            .password_workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| IdentityError::Worker)?;
        let hash = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash_password(&new)
        })
        .await
        .map_err(|_| IdentityError::Worker)??;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.account SET password=$1,\"updatedAt\"=statement_timestamp() WHERE \"userId\"=$2 AND \"providerId\"='credential'",[hash.into(),actor.id.clone().into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"mustChangePassword\"=false,\"updatedAt\"=statement_timestamp() WHERE id=$1",[actor.id.clone().into()])).await?;
        let source = self
            .session_source(&tx, &actor.id, &actor.session_id)
            .await?;
        invalidate_sessions(&tx, &actor.id, None).await?;
        self.issue_session_with_source(tx, &actor.id, source).await
    }
    pub async fn list_sessions(&self, cookie: &str) -> Result<Vec<SessionSummary>> {
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        let rows=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,\"expiresAt\",\"createdAt\" FROM webdock_auth.session WHERE \"userId\"=$1 AND \"expiresAt\">statement_timestamp() ORDER BY \"createdAt\",id",[actor.id.into()])).await?;
        let sessions = rows
            .iter()
            .map(|row| {
                let id: String = row.try_get("", "id")?;
                Ok(SessionSummary {
                    current: id == actor.session_id,
                    id,
                    expires_at: row.try_get("", "expiresAt")?,
                    created_at: row.try_get("", "createdAt")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(sessions)
    }
    pub async fn revoke_other_sessions(&self, cookie: &str) -> Result<u64> {
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        let count = invalidate_sessions(&tx, &actor.id, Some(&actor.session_id)).await?;
        tx.commit().await?;
        Ok(count)
    }

    pub async fn regenerate_recovery_codes(
        &self,
        cookie: &str,
        password: &str,
    ) -> Result<Vec<String>> {
        let keys = self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        if !actor.mfa || !actor.verified || actor.must_change || actor.preview {
            return Err(IdentityError::SecurityState);
        }
        self.verify_current_password(password, &credential(&tx, &actor.id).await?)
            .await?;
        let codes = (0..10)
            .map(|_| {
                let raw = hex::encode(random::<10>()?);
                Ok(format!("{}-{}", &raw[..10], &raw[10..]))
            })
            .collect::<Result<Vec<String>>>()?;
        let row = factor(&tx, &actor.id).await?;
        if row.try_get::<Option<bool>>("", "verified")? == Some(false) {
            return Err(IdentityError::SecurityState);
        }
        let encrypted = keys
            .encrypt(&serde_json::to_string(&codes).map_err(|_| IdentityError::InvalidEncoding)?)?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.\"twoFactor\" SET \"backupCodes\"=$1 WHERE \"userId\"=$2",
            [encrypted.into(), actor.id.clone().into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.identity_mfa_challenge WHERE user_id=$1",
            [actor.id.into()],
        ))
        .await?;
        tx.commit().await?;
        Ok(codes)
    }
    pub async fn disable_two_factor(&self, cookie: &str, password: &str) -> Result<IssuedSession> {
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        if !actor.mfa || !actor.verified || actor.must_change || actor.preview {
            return Err(IdentityError::SecurityState);
        }
        self.verify_current_password(password, &credential(&tx, &actor.id).await?)
            .await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.\"twoFactor\" WHERE \"userId\"=$1",
            [actor.id.clone().into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.identity_totp_replay WHERE user_id=$1",
            [actor.id.clone().into()],
        ))
        .await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"twoFactorEnabled\"=false,\"updatedAt\"=statement_timestamp() WHERE id=$1",[actor.id.clone().into()])).await?;
        let mut source = self
            .session_source(&tx, &actor.id, &actor.session_id)
            .await?;
        if let Some(source) = &mut source {
            source.mfa_verified = false;
        }
        invalidate_sessions(&tx, &actor.id, None).await?;
        self.issue_session_with_source(tx, &actor.id, source).await
    }
    pub async fn refresh_session(&self, cookie: &str) -> Result<IssuedSession> {
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.session SET \"expiresAt\"=CASE WHEN \"updatedAt\"<=statement_timestamp()-interval '15 minutes' THEN statement_timestamp()+interval '8 hours' ELSE \"expiresAt\" END,\"updatedAt\"=CASE WHEN \"updatedAt\"<=statement_timestamp()-interval '15 minutes' THEN statement_timestamp() ELSE \"updatedAt\" END WHERE id=$1 RETURNING token,greatest(1,extract(epoch FROM (\"expiresAt\"-statement_timestamp()))::bigint) AS age",[actor.session_id.clone().into()])).await?.ok_or(IdentityError::Unauthorized)?;
        let token: String = row.try_get("", "token")?;
        let age: i64 = row.try_get("", "age")?;
        let set_cookie = self.cookies.header(
            &token,
            age.try_into().map_err(|_| IdentityError::InvalidInput)?,
        )?;
        tx.commit().await?;
        let principal = self
            .authenticate_cookie(set_cookie.split(';').next().expect("cookie"))
            .await?
            .ok_or(IdentityError::Unauthorized)?;
        Ok(IssuedSession {
            principal,
            set_cookie,
        })
    }
    pub async fn revoke_owned_session(&self, cookie: &str, target: &str) -> Result<bool> {
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        let changed = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.session WHERE id=$1 AND \"userId\"=$2",
                [target.into(), actor.id.into()],
            ))
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }
    pub async fn revoke_session_by_token(&self, cookie: &str, target_token: &str) -> Result<bool> {
        let tx = self.db.begin().await?;
        let actor = self.locked_actor(&tx, cookie).await?;
        let changed = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.session WHERE token=$1 AND \"userId\"=$2",
                [target_token.into(), actor.id.into()],
            ))
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }
    /// Offline first-account command. It asserts installation ownership, not mailbox delivery.
    /// MFA remains disabled and password replacement is mandatory until the real setup flow succeeds.
    pub async fn provision_first_operator(
        &self,
        email: &str,
        name: &str,
        password: &str,
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
        tx.execute_unprepared("SELECT pg_advisory_xact_lock(hashtext('webdock-first-operator'))")
            .await?;
        if tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.\"user\" WHERE role='operator' LIMIT 1",
            ))
            .await?
            .is_some()
        {
            return Err(IdentityError::SecurityState);
        }
        let user=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($3,$1,$2,true,'operator',false,true) RETURNING id",[name.into(),email.into(),self.allocate_id(&tx).await?.into()])).await?.ok_or(IdentityError::InvalidInput)?;
        let id: String = user.try_get("", "id")?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.account(id,\"accountId\",\"providerId\",\"userId\",password,\"updatedAt\") VALUES($3,$1,'credential',$1,$2,statement_timestamp())",[id.clone().into(),hash.into(),self.allocate_id(&tx).await?.into()])).await?;
        tx.commit().await?;
        Ok(id)
    }
}
pub(crate) async fn credential(tx: &DatabaseTransaction, id: &str) -> Result<String> {
    let rows=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT password FROM webdock_auth.account WHERE \"userId\"=$1 AND \"providerId\"='credential' FOR UPDATE",[id.into()])).await?;
    if rows.len() != 1 {
        return Err(IdentityError::Unauthorized);
    }
    rows[0]
        .try_get::<Option<String>>("", "password")?
        .ok_or(IdentityError::Unauthorized)
}
async fn factor(tx: &DatabaseTransaction, id: &str) -> Result<QueryResult> {
    let rows=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,secret,\"backupCodes\",verified,coalesce(\"lockedUntil\">statement_timestamp(),false) AS locked,floor(extract(epoch FROM statement_timestamp()))::bigint AS timestamp FROM webdock_auth.\"twoFactor\" WHERE \"userId\"=$1 FOR UPDATE",[id.into()])).await?;
    if rows.len() != 1 {
        return Err(IdentityError::SecurityState);
    }
    Ok(rows.into_iter().next().expect("one factor"))
}
async fn record_failure(tx: &DatabaseTransaction, factor: &QueryResult) -> Result<()> {
    tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"twoFactor\" SET \"failedVerificationCount\"=CASE WHEN \"lockedUntil\"<=statement_timestamp() THEN 1 ELSE coalesce(\"failedVerificationCount\",0)+1 END,\"lockedUntil\"=CASE WHEN \"lockedUntil\"<=statement_timestamp() THEN NULL WHEN coalesce(\"failedVerificationCount\",0)+1>=10 THEN statement_timestamp()+interval '15 minutes' ELSE \"lockedUntil\" END WHERE id=$1",[factor.try_get::<String>("","id")?.into()])).await?;
    Ok(())
}
pub(crate) async fn invalidate_sessions(
    tx: &DatabaseTransaction,
    id: &str,
    keep: Option<&str>,
) -> Result<u64> {
    tx.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.identity_mfa_challenge WHERE user_id=$1",
        [id.into()],
    ))
    .await?;
    Ok(tx
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.session WHERE \"userId\"=$1 AND ($2::text IS NULL OR id<>$2)",
            [id.into(), keep.map(str::to_owned).into()],
        ))
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::totp;
    #[test]
    fn matches_legacy_raw_utf8_totp_key() {
        let otp = totp(
            "0123456789abcdefghijklmnopqrstuv",
            "fixture@example.invalid",
        )
        .unwrap();
        assert_eq!(otp.check("046920", 1_699_999_980), Some(56_666_666));
        assert_eq!(otp.check("046920", 1_700_000_100), None);
        assert_eq!(otp.check("garbage", 1_699_999_980), None);
    }
}
