//! Password and step-up login domain operations; no retained database fallback.
#[path = "native_login_mfa.rs"]
mod mfa;
#[path = "native_login_neon.rs"]
mod neon;
#[cfg(test)]
#[path = "native_login_tests.rs"]
mod tests;
use super::IdentityService;
use crate::{
    IdentityError, IssuedSession, PasswordAuthentication, Result,
    crypto::random,
    neon::{NeonMapping, NeonSource},
    security::digest,
};
use spitzli_persistence::postgres::{Connection, Pool, Transaction};
use std::time::Duration;
use tokio_postgres::types::Type;
const BUDGET: Duration = Duration::from_secs(30);
const MISSING_PASSWORD: &str = "000102030405060708090a0b0c0d0e0f:c7877280641a617ec087f9365d3ad7facccc9681e2af94d0a148b29a67b413ab0e08c7f08f9bd6232c369f5bbd3eec03acc2a170c24fd179aecc67c282756d12";
enum Outcome {
    Invalid,
    Setup(String),
    Mfa {
        user: String,
        cookie: String,
    },
    Session {
        user: String,
        cookie: String,
        setup: bool,
    },
}
struct User {
    id: String,
    name: String,
    email: String,
    role: Option<String>,
    verified: bool,
    must_change: bool,
    mfa: bool,
}
async fn locked_user(tx: &Transaction<'_>, user: &str) -> Result<User> {
    let row=tx.query_typed_opt(r#"SELECT id,name,email,role,"emailVerified","mustChangePassword",coalesce("twoFactorEnabled",false) AS mfa FROM webdock_auth."user" WHERE id=$1 AND NOT coalesce(banned,false) FOR UPDATE"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
    Ok(User {
        id: row.try_get("id").map_err(|_| IdentityError::Persistence)?,
        name: row
            .try_get("name")
            .map_err(|_| IdentityError::Persistence)?,
        email: row
            .try_get("email")
            .map_err(|_| IdentityError::Persistence)?,
        role: row
            .try_get("role")
            .map_err(|_| IdentityError::Persistence)?,
        verified: row
            .try_get("emailVerified")
            .map_err(|_| IdentityError::Persistence)?,
        must_change: row
            .try_get("mustChangePassword")
            .map_err(|_| IdentityError::Persistence)?,
        mfa: row.try_get("mfa").map_err(|_| IdentityError::Persistence)?,
    })
}
async fn local(tx: &Transaction<'_>, user: &str) -> Result<bool> {
    tx.query_typed_one(
        "SELECT webdock_rls.local_identity_authority($1) AS allowed",
        &[(&user, Type::TEXT)],
    )
    .await
    .map_err(|_| IdentityError::Persistence)?
    .try_get("allowed")
    .map_err(|_| IdentityError::Persistence)
}
async fn required(tx: &Transaction<'_>) -> Result<bool> {
    tx.query_typed_one("SELECT webdock_rls.neon_required() AS required", &[])
        .await
        .map_err(|_| IdentityError::Persistence)?
        .try_get("required")
        .map_err(|_| IdentityError::Persistence)
}
async fn mapping(tx: &Transaction<'_>, user: &str) -> Result<Option<NeonMapping>> {
    // Metadata remains SELECT-only. Canonical FOR UPDATE fences new mapping FK admission.
    tx.query_typed_opt("SELECT user_id,provider_subject::text,provider_origin,revision,enabled FROM webdock_auth.neon_identity WHERE user_id=$1",&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.map(|row|->Result<_>{Ok(NeonMapping{user:row.try_get("user_id").map_err(|_|IdentityError::Persistence)?,subject:row.try_get("provider_subject").map_err(|_|IdentityError::Persistence)?,issuer:row.try_get("provider_origin").map_err(|_|IdentityError::Persistence)?,revision:row.try_get("revision").map_err(|_|IdentityError::Persistence)?,enabled:row.try_get("enabled").map_err(|_|IdentityError::Persistence)?})}).transpose()
}
async fn credential(tx: &Transaction<'_>, user: &str) -> Result<String> {
    let rows=tx.query_typed(r#"SELECT password FROM webdock_auth.account WHERE "userId"=$1 AND "providerId"='credential' FOR UPDATE"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
    if rows.len() != 1 {
        return Err(IdentityError::Unauthorized);
    }
    rows[0]
        .try_get::<_, Option<String>>("password")
        .map_err(|_| IdentityError::Persistence)?
        .ok_or(IdentityError::Unauthorized)
}
impl IdentityService {
    pub fn with_native_password_login(mut self, pool: Pool) -> Self {
        self.native_login = Some(pool.clone());
        self.with_native_session_management(pool)
    }
    async fn login_connection(&self) -> Result<Connection> {
        self.native_login
            .as_ref()
            .ok_or(IdentityError::Configuration)?
            .acquire()
            .await
            .map_err(|_| IdentityError::Persistence)
    }
    async fn login_id(&self) -> Result<String> {
        self.allocator
            .as_ref()
            .ok_or(IdentityError::Configuration)?
            .allocate()
            .await
            .map(|id| id.to_string())
            .map_err(|_| IdentityError::Allocation)
    }
    async fn finish_native_login(&self, outcome: Outcome) -> Result<PasswordAuthentication> {
        Ok(match outcome {
            Outcome::Invalid => PasswordAuthentication::Invalid,
            Outcome::Setup(user_id) => PasswordAuthentication::SetupRequired {
                user_id,
                session: None,
            },
            Outcome::Mfa {
                user: user_id,
                cookie: set_cookie,
            } => PasswordAuthentication::MfaRequired {
                user_id,
                set_cookie,
            },
            Outcome::Session {
                user: user_id,
                cookie: set_cookie,
                setup,
            } => {
                let principal = self
                    .authenticate_cookie(
                        set_cookie
                            .split(';')
                            .next()
                            .ok_or(IdentityError::InvalidEncoding)?,
                    )
                    .await?
                    .ok_or(IdentityError::Unauthorized)?;
                if principal.user_id != user_id {
                    return Err(IdentityError::Unauthorized);
                }
                let session = IssuedSession {
                    principal,
                    set_cookie,
                };
                if setup {
                    PasswordAuthentication::SetupRequired {
                        user_id,
                        session: Some(session),
                    }
                } else {
                    PasswordAuthentication::Authenticated(session)
                }
            }
        })
    }
    pub(crate) async fn native_authenticate_password(
        &self,
        email: &str,
        password: &str,
    ) -> Result<PasswordAuthentication> {
        let (user,hash,managed)=tokio::time::timeout(BUDGET,async {
            let id=self.login_id().await?;let key=format!("identity:password:{}",digest(email));
            let mut connection=self.login_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let rate=tx.query_typed_one("INSERT INTO webdock_auth.\"rateLimit\"(id,key,count,\"lastRequest\") VALUES($2,$1,1,floor(extract(epoch FROM statement_timestamp())*1000)::bigint) ON CONFLICT(key) DO UPDATE SET count=CASE WHEN webdock_auth.\"rateLimit\".\"lastRequest\"<=floor(extract(epoch FROM statement_timestamp())*1000)::bigint-60000 THEN 1 ELSE webdock_auth.\"rateLimit\".count+1 END,\"lastRequest\"=CASE WHEN webdock_auth.\"rateLimit\".\"lastRequest\"<=floor(extract(epoch FROM statement_timestamp())*1000)::bigint-60000 THEN floor(extract(epoch FROM statement_timestamp())*1000)::bigint ELSE webdock_auth.\"rateLimit\".\"lastRequest\" END RETURNING count",&[(&key,Type::TEXT),(&id,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.try_get::<_,i32>("count").map_err(|_|IdentityError::Persistence)?;
            tx.commit().await.map_err(|_|IdentityError::Persistence)?;if rate>10 {return Err(IdentityError::RateLimited);}
            let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let users=tx.query_typed(r#"SELECT id FROM webdock_auth."user" WHERE lower(email)=$1 LIMIT 2"#,&[(&email,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            let user=if users.len()==1 {Some(users[0].try_get::<_,String>("id").map_err(|_|IdentityError::Persistence)?)}else{None};
            let managed=if let Some(user)=&user {mapping(&tx,user).await?}else{None};
            let rows=tx.query_typed(r#"SELECT a.password FROM webdock_auth."user" u JOIN webdock_auth.account a ON a."userId"=u.id AND a."providerId"='credential' WHERE lower(u.email)=$1 LIMIT 2"#,&[(&email,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            let hash=if rows.len()==1 {rows[0].try_get::<_,Option<String>>("password").map_err(|_|IdentityError::Persistence)?}else{None};
            tx.commit().await.map_err(|_|IdentityError::Persistence)?;Ok::<_,IdentityError>((user,hash,managed))
        }).await.map_err(|_|IdentityError::Persistence)??;
        if let Some(mapping) = managed {
            if !mapping.enabled {
                return Ok(PasswordAuthentication::Invalid);
            }
            let login = self
                .neon
                .as_ref()
                .ok_or(IdentityError::Configuration)?
                .sign_in(email, password)
                .await?;
            return self.native_provider_login(email, mapping, login).await;
        }
        // Release all DB connections before bounded equal-cost password verification.
        match self
            .verify_current_password(password, hash.as_deref().unwrap_or(MISSING_PASSWORD))
            .await
        {
            Ok(()) => {}
            Err(IdentityError::Unauthorized) => return Ok(PasswordAuthentication::Invalid),
            Err(error) => return Err(error),
        }
        let (Some(user), Some(hash)) = (user, hash) else {
            return Ok(PasswordAuthentication::Invalid);
        };
        let outcome = tokio::time::timeout(BUDGET, async {
            let mut connection = self.login_connection().await?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| IdentityError::Persistence)?;
            let actor = match locked_user(&tx, &user).await {
                Ok(user) => user,
                Err(IdentityError::Unauthorized) => return Ok(Outcome::Invalid),
                Err(error) => return Err(error),
            };
            let current = match credential(&tx, &user).await {
                Ok(current) => current,
                Err(IdentityError::Unauthorized) => return Ok(Outcome::Invalid),
                Err(error) => return Err(error),
            };
            if actor.email.to_lowercase() != email
                || current != hash
                || !local(&tx, &user).await?
                || mapping(&tx, &user).await?.is_some()
            {
                return Ok(Outcome::Invalid);
            }
            if !actor.verified {
                tx.commit().await.map_err(|_| IdentityError::Persistence)?;
                return Ok(Outcome::Setup(user));
            }
            if ![Some("user"), Some("operator")].contains(&actor.role.as_deref()) {
                return Ok(Outcome::Invalid);
            }
            if required(&tx).await? {
                return self
                    .native_enroll_password(tx, actor, email, password)
                    .await;
            }
            if actor.mfa {
                let cookie = self.native_mfa_challenge(&tx, &user, &hash).await?;
                tx.commit().await.map_err(|_| IdentityError::Persistence)?;
                return Ok(Outcome::Mfa { user, cookie });
            }
            let setup = actor.must_change || actor.role.as_deref() == Some("operator");
            let cookie = self.native_login_cookie(tx, &actor, None).await?;
            Ok(Outcome::Session {
                user,
                cookie,
                setup,
            })
        })
        .await
        .map_err(|_| IdentityError::Persistence)??;
        self.finish_native_login(outcome).await
    }
    async fn native_mfa_challenge(
        &self,
        tx: &Transaction<'_>,
        user: &str,
        hash: &str,
    ) -> Result<String> {
        let token = hex::encode(random::<32>()?);
        tx.execute_typed("DELETE FROM webdock_auth.identity_mfa_challenge WHERE user_id=$1 AND expires_at<=statement_timestamp()",&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
        tx.execute_typed("INSERT INTO webdock_auth.identity_mfa_challenge(token_hash,user_id,credential_digest,expires_at) VALUES($1,$2,$3,statement_timestamp()+interval '5 minutes')",&[(&digest(&token),Type::TEXT),(&user,Type::TEXT),(&digest(hash),Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
        self.cookies.challenge_header(&token)
    }
    async fn native_login_cookie(
        &self,
        tx: Transaction<'_>,
        user: &User,
        source: Option<NeonSource>,
    ) -> Result<String> {
        if !local(&tx, &user.id).await? {
            return Err(IdentityError::Unauthorized);
        }
        if let Some(source) = &source {
            if source.user != user.id || user.mfa && !source.mfa_verified {
                return Err(IdentityError::SecurityState);
            }
            self.provider_cookie(source)?;
            if !self.native_source_live(&tx, source).await? {
                return Err(IdentityError::Unauthorized);
            }
        } else if mapping(&tx, &user.id).await?.is_some() || required(&tx).await? {
            return Err(IdentityError::SecurityState);
        }
        let id = self.login_id().await?;
        let token = hex::encode(random::<32>()?);
        tx.execute_typed(r#"INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$2,$3,statement_timestamp()+interval '8 hours',statement_timestamp())"#,&[(&id,Type::TEXT),(&token,Type::TEXT),(&user.id,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
        if let Some(source) = &source {
            self.native_store_source(&tx, &id, source).await?;
        }
        let cookie = self.cookies.header(&token, 28800)?;
        tx.commit().await.map_err(|_| IdentityError::Persistence)?;
        Ok(cookie)
    }
}
