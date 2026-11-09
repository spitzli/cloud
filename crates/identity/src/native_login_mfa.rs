//! One-use native step-up with durable failure and replay protection.
use super::{BUDGET, User, credential, local, locked_user, mapping};
use crate::{
    IdentityError, IdentityService, IssuedSession, MfaProof, Result,
    neon::NeonSource,
    security::{digest, totp},
};
use subtle::ConstantTimeEq;
use tokio_postgres::{Row, types::Type};
use webdock_persistence::postgres::Transaction;

impl IdentityService {
    pub(crate) async fn native_complete_mfa(
        &self,
        cookie: &str,
        proof: MfaProof,
    ) -> Result<IssuedSession> {
        let token = self
            .cookies
            .read_challenge(cookie)?
            .ok_or(IdentityError::Unauthorized)?;
        let cookie=tokio::time::timeout(BUDGET,async {
            let hash=digest(&token);let mut connection=self.login_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let lookup=tx.query_typed_opt("SELECT user_id FROM webdock_auth.identity_mfa_challenge WHERE token_hash=$1",&[(&hash,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
            let user:String=lookup.try_get("user_id").map_err(|_|IdentityError::Persistence)?;let actor=locked_user(&tx,&user).await?;
            if !actor.verified || !actor.mfa || ![Some("user"),Some("operator")].contains(&actor.role.as_deref()) || !local(&tx,&user).await? {return Err(IdentityError::Unauthorized);}
            let challenge=tx.query_typed_opt("SELECT credential_digest FROM webdock_auth.identity_mfa_challenge WHERE token_hash=$1 AND user_id=$2 AND expires_at>statement_timestamp() AND attempts<5 FOR UPDATE",&[(&hash,Type::TEXT),(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
            if challenge.try_get::<_,String>("credential_digest").map_err(|_|IdentityError::Persistence)?!=digest(&credential(&tx,&user).await?) {return Err(IdentityError::Unauthorized);}
            let mut source=self.native_pending_source(&tx,&user,&hash).await?;
            let factors=tx.query_typed(r#"SELECT id FROM webdock_auth."twoFactor" WHERE "userId"=$1 FOR UPDATE"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            if factors.len()!=1 {return Err(IdentityError::SecurityState);}
            // Refresh time/lock facts after acquisition, not the pre-wait SELECT projection.
            let factor=tx.query_typed_one(r#"SELECT id,secret,"backupCodes",verified,coalesce("lockedUntil">statement_timestamp(),false) AS locked,floor(extract(epoch FROM statement_timestamp()))::bigint AS timestamp FROM webdock_auth."twoFactor" WHERE "userId"=$1"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            if factor.try_get::<_,Option<bool>>("verified").map_err(|_|IdentityError::Persistence)?==Some(false) {return Err(IdentityError::SecurityState);}
            if tx.execute_typed("UPDATE webdock_auth.identity_mfa_challenge SET attempts=attempts+1 WHERE token_hash=$1 AND expires_at>statement_timestamp() AND attempts<5",&[(&hash,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?!=1 {return Err(IdentityError::Unauthorized);}
            if !self.native_verify_factor(&tx,&actor,&factor,&proof).await? {
                let id:String=factor.try_get("id").map_err(|_|IdentityError::Persistence)?;
                tx.execute_typed(r#"UPDATE webdock_auth."twoFactor" SET "failedVerificationCount"=CASE WHEN "lockedUntil"<=statement_timestamp() THEN 1 ELSE coalesce("failedVerificationCount",0)+1 END,"lockedUntil"=CASE WHEN "lockedUntil"<=statement_timestamp() THEN NULL WHEN coalesce("failedVerificationCount",0)+1>=10 THEN statement_timestamp()+interval '15 minutes' ELSE "lockedUntil" END WHERE id=$1"#,&[(&id,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
                tx.commit().await.map_err(|_|IdentityError::Persistence)?;return Err(IdentityError::InvalidCode);
            }
            if tx.execute_typed("DELETE FROM webdock_auth.identity_mfa_challenge WHERE token_hash=$1 AND user_id=$2 AND expires_at>statement_timestamp()",&[(&hash,Type::TEXT),(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?!=1{return Err(IdentityError::Unauthorized);}
            tx.execute_typed(r#"UPDATE webdock_auth."twoFactor" SET "failedVerificationCount"=0,"lockedUntil"=NULL WHERE "userId"=$1"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            if let Some(source)=&mut source {source.mfa_verified=true;}
            self.native_login_cookie(tx,&actor,source).await
        }).await.map_err(|_|IdentityError::Persistence)??;
        // The committed transaction/connection is gone before native canonical rereading.
        let principal = self
            .authenticate_cookie(
                cookie
                    .split(';')
                    .next()
                    .ok_or(IdentityError::InvalidEncoding)?,
            )
            .await?
            .ok_or(IdentityError::Unauthorized)?;
        Ok(IssuedSession {
            principal,
            set_cookie: cookie,
        })
    }
    async fn native_pending_source(
        &self,
        tx: &Transaction<'_>,
        user: &str,
        hash: &str,
    ) -> Result<Option<NeonSource>> {
        if mapping(tx, user).await?.is_none() {
            return Ok(None);
        }
        let row=tx.query_typed_opt("SELECT n.user_id,n.provider_subject::text,n.provider_session_id::text,n.identity_revision,n.credential_digest,n.encrypted_cookie FROM webdock_auth.neon_mfa_challenge n JOIN webdock_auth.identity_mfa_challenge c ON c.token_hash=n.token_hash WHERE n.token_hash=$1 AND n.user_id=$2 AND c.expires_at>statement_timestamp() AND c.attempts<5 FOR UPDATE OF n,c",&[(&hash,Type::TEXT),(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
        let source = NeonSource {
            user: row
                .try_get("user_id")
                .map_err(|_| IdentityError::Persistence)?,
            subject: row
                .try_get("provider_subject")
                .map_err(|_| IdentityError::Persistence)?,
            session: row
                .try_get("provider_session_id")
                .map_err(|_| IdentityError::Persistence)?,
            revision: row
                .try_get("identity_revision")
                .map_err(|_| IdentityError::Persistence)?,
            credential_digest: row
                .try_get("credential_digest")
                .map_err(|_| IdentityError::Persistence)?,
            encrypted_cookie: row
                .try_get("encrypted_cookie")
                .map_err(|_| IdentityError::Persistence)?,
            mfa_verified: false,
        };
        self.provider_cookie(&source)?;
        if !self.native_source_live(tx, &source).await? {
            return Err(IdentityError::Unauthorized);
        }
        Ok(Some(source))
    }
    async fn native_verify_factor(
        &self,
        tx: &Transaction<'_>,
        user: &User,
        factor: &Row,
        proof: &MfaProof,
    ) -> Result<bool> {
        let keys = self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        if factor
            .try_get::<_, bool>("locked")
            .map_err(|_| IdentityError::Persistence)?
        {
            return Err(IdentityError::RateLimited);
        }
        match proof {
            MfaProof::Totp(code) => {
                if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
                    return Ok(false);
                }
                let secret = keys.decrypt(
                    &factor
                        .try_get::<_, String>("secret")
                        .map_err(|_| IdentityError::Persistence)?,
                )?;
                let timestamp: i64 = factor
                    .try_get("timestamp")
                    .map_err(|_| IdentityError::Persistence)?;
                let Some(step) = totp(&secret, &user.email)?.check(
                    code,
                    timestamp
                        .try_into()
                        .map_err(|_| IdentityError::InvalidInput)?,
                ) else {
                    return Ok(false);
                };
                Ok(tx.query_typed_opt("INSERT INTO webdock_auth.identity_totp_replay(user_id,secret_digest,last_step) VALUES($1,$2,$3) ON CONFLICT(user_id) DO UPDATE SET secret_digest=excluded.secret_digest,last_step=excluded.last_step WHERE webdock_auth.identity_totp_replay.secret_digest<>excluded.secret_digest OR webdock_auth.identity_totp_replay.last_step<excluded.last_step RETURNING user_id",&[(&user.id,Type::TEXT),(&digest(&secret),Type::TEXT),(&(step as i64),Type::INT8)]).await.map_err(|_|IdentityError::Persistence)?.is_some())
            }
            MfaProof::RecoveryCode(code) => {
                if code.len() > 128 {
                    return Ok(false);
                }
                let mut codes: Vec<String> = serde_json::from_str(
                    &keys.decrypt(
                        &factor
                            .try_get::<_, String>("backupCodes")
                            .map_err(|_| IdentityError::Persistence)?,
                    )?,
                )
                .map_err(|_| IdentityError::InvalidEncoding)?;
                let Some(index) = codes
                    .iter()
                    .position(|stored| bool::from(stored.as_bytes().ct_eq(code.as_bytes())))
                else {
                    return Ok(false);
                };
                codes.remove(index);
                let encoded = keys.encrypt(
                    &serde_json::to_string(&codes).map_err(|_| IdentityError::InvalidEncoding)?,
                )?;
                let id: String = factor
                    .try_get("id")
                    .map_err(|_| IdentityError::Persistence)?;
                if tx
                    .execute_typed(
                        r#"UPDATE webdock_auth."twoFactor" SET "backupCodes"=$1 WHERE id=$2"#,
                        &[(&encoded, Type::TEXT), (&id, Type::TEXT)],
                    )
                    .await
                    .map_err(|_| IdentityError::Persistence)?
                    != 1
                {
                    return Err(IdentityError::SecurityState);
                }
                Ok(true)
            }
        }
    }
}
