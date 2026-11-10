//! Provider result → canonical login, preserving current binding and custody.
use super::{BUDGET, Outcome, User, local, locked_user, mapping};
use crate::{
    IdentityError, IdentityService, PasswordAuthentication, Result,
    neon::{NeonMapping, NeonSource},
    neon_client::NeonLogin,
    security::digest,
};
use spitzli_persistence::postgres::Transaction;
use tokio_postgres::types::Type;

impl IdentityService {
    pub(super) async fn native_provider_login(
        &self,
        email: &str,
        expected: NeonMapping,
        login: NeonLogin,
    ) -> Result<PasswordAuthentication> {
        let outcome = tokio::time::timeout(BUDGET, async {
            let mut connection = self.login_connection().await?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| IdentityError::Persistence)?;
            let actor = locked_user(&tx, &expected.user).await?;
            let current = mapping(&tx, &actor.id)
                .await?
                .ok_or(IdentityError::Unauthorized)?;
            if current != expected {
                return Err(IdentityError::Unauthorized);
            }
            self.complete_native_provider(tx, actor, email, current, login)
                .await
        })
        .await
        .map_err(|_| IdentityError::Persistence)??;
        self.finish_native_login(outcome).await
    }
    pub(super) async fn native_enroll_password(
        &self,
        tx: Transaction<'_>,
        actor: User,
        email: &str,
        password: &str,
    ) -> Result<Outcome> {
        let client = self.neon.as_ref().ok_or(IdentityError::Configuration)?;
        self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        // Exact local credential and canonical locks remain held; do not adopt an existing provider user.
        let login = client.sign_up(email, password, &actor.name).await?;
        let binding = NeonMapping {
            user: actor.id.clone(),
            subject: login.subject.clone(),
            issuer: client.issuer().into(),
            revision: 1,
            enabled: true,
        };
        tx.execute_typed("INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES($1,$2,$3::uuid,$1)",&[(&actor.id,Type::TEXT),(&binding.issuer,Type::TEXT),(&binding.subject,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
        self.complete_native_provider(tx, actor, email, binding, login)
            .await
    }
    async fn complete_native_provider(
        &self,
        tx: Transaction<'_>,
        actor: User,
        email: &str,
        binding: NeonMapping,
        login: NeonLogin,
    ) -> Result<Outcome> {
        if actor.email.to_lowercase() != email
            || !actor.verified
            || ![Some("user"), Some("operator")].contains(&actor.role.as_deref())
            || !local(&tx, &actor.id).await?
        {
            return Err(IdentityError::SecurityState);
        }
        let row = tx
            .query_typed_one(
                "SELECT webdock_rls.neon_session_facts($1) AS facts",
                &[(&login.token, Type::TEXT)],
            )
            .await
            .map_err(|_| IdentityError::Persistence)?;
        let facts: Option<serde_json::Value> = row
            .try_get("facts")
            .map_err(|_| IdentityError::Persistence)?;
        let (source, hash) = self.source_from_login_facts(
            &actor.id,
            &binding,
            login,
            facts.ok_or(IdentityError::Unauthorized)?,
        )?;
        if tx.execute_typed(r#"UPDATE webdock_auth.account SET password=$1,"updatedAt"=statement_timestamp() WHERE "userId"=$2 AND "providerId"='credential'"#,&[(&hash,Type::TEXT),(&actor.id,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?!=1{return Err(IdentityError::SecurityState);}
        if !local(&tx, &actor.id).await? || !self.native_source_live(&tx, &source).await? {
            return Err(IdentityError::Unauthorized);
        }
        if actor.mfa {
            let cookie = self.native_mfa_challenge(&tx, &actor.id, &hash).await?;
            let token = self
                .cookies
                .read_challenge(
                    cookie
                        .split(';')
                        .next()
                        .ok_or(IdentityError::InvalidEncoding)?,
                )?
                .ok_or(IdentityError::InvalidEncoding)?;
            tx.execute_typed("INSERT INTO webdock_auth.neon_mfa_challenge(token_hash,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie) VALUES($1,$2,$3::uuid,$4::uuid,$5,$6,$7)",&[(&digest(&token),Type::TEXT),(&source.user,Type::TEXT),(&source.session,Type::TEXT),(&source.subject,Type::TEXT),(&source.revision,Type::INT4),(&source.credential_digest,Type::TEXT),(&source.encrypted_cookie,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            tx.commit().await.map_err(|_| IdentityError::Persistence)?;
            return Ok(Outcome::Mfa {
                user: actor.id,
                cookie,
            });
        }
        let setup = actor.must_change || actor.role.as_deref() == Some("operator");
        let user = actor.id.clone();
        let cookie = self.native_login_cookie(tx, &actor, Some(source)).await?;
        Ok(Outcome::Session {
            user,
            cookie,
            setup,
        })
    }
    pub(super) async fn native_source_live(
        &self,
        tx: &Transaction<'_>,
        source: &NeonSource,
    ) -> Result<bool> {
        tx.query_typed_one(
            "SELECT webdock_rls.neon_binding_live($1,$2::uuid,$3::uuid,$4,$5) AS live",
            &[
                (&source.user, Type::TEXT),
                (&source.session, Type::TEXT),
                (&source.subject, Type::TEXT),
                (&source.revision, Type::INT4),
                (&source.credential_digest, Type::TEXT),
            ],
        )
        .await
        .map_err(|_| IdentityError::Persistence)?
        .try_get("live")
        .map_err(|_| IdentityError::Persistence)
    }
    pub(super) async fn native_store_source(
        &self,
        tx: &Transaction<'_>,
        session: &str,
        source: &NeonSource,
    ) -> Result<()> {
        if !self.native_source_live(tx, source).await? {
            return Err(IdentityError::Unauthorized);
        }
        tx.execute_typed("INSERT INTO webdock_auth.neon_session(session_id,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie,mfa_verified) VALUES($1,$2,$3::uuid,$4::uuid,$5,$6,$7,$8)",&[(&session,Type::TEXT),(&source.user,Type::TEXT),(&source.session,Type::TEXT),(&source.subject,Type::TEXT),(&source.revision,Type::INT4),(&source.credential_digest,Type::TEXT),(&source.encrypted_cookie,Type::TEXT),(&source.mfa_verified,Type::BOOL)]).await.map_err(|_|IdentityError::Persistence)?;
        Ok(())
    }
}
