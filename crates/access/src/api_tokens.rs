//! API tokens for one workspace. A token is its own session of the member who made it: it acts with
//! that member's role, only in that workspace, and only reads unless it may write.
use crate::{
    contracts::{id, text},
    service::json_rows,
    *,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use webdock_identity::Principal;

/// Who a bearer token speaks for.
pub struct ApiActor {
    pub principal: Principal,
    pub customer: String,
    pub can_write: bool,
}
const PREFIX: &str = "wdk_";
fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}
const VIEW: &str = "jsonb_build_object('id',t.id,'name',t.name,'canWrite',t.can_write,'createdAt',t.created_at,'expiresAt',t.expires_at,'lastUsedAt',t.last_used_at,\
    'createdBy',coalesce((SELECT u.name FROM webdock_auth.\"user\" u WHERE u.id=t.user_id),'')) AS data";

impl AccessService {
    /// Owners, admins and operators see the workspace's tokens, never the secrets.
    pub async fn list_api_tokens_as(
        &self,
        actor: &Principal,
        customer: &str,
    ) -> Result<Vec<Value>> {
        let (_, tx) = self.domain_writer(actor, customer).await?;
        let rows = json_rows(
            &tx,
            &format!("SELECT {VIEW} FROM webdock_auth.api_token t WHERE t.customer_id=$1 AND t.expires_at>now() ORDER BY t.created_at DESC"),
            vec![id(customer)?.into()],
        )
        .await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// The token is returned once; only its hash is kept.
    pub async fn create_api_token_as(
        &self,
        actor: &Principal,
        customer: &str,
        name: &str,
        can_write: bool,
        days: i64,
    ) -> Result<Value> {
        let name = text(name, 80)?;
        if !(1..=366).contains(&days) {
            return Err(Error::InvalidInput);
        }
        if self.current_actor(actor).await?.in_tenant_preview() {
            return Err(Error::Forbidden);
        }
        let (actor, tx) = self.domain_writer(actor, customer).await?;
        let session = self
            .identity
            .issue_api_session(&actor.user_id, &format!("API token: {name}"), days)
            .await
            .map_err(|_| Error::Allocation)?;
        let mut secret = [0u8; 32];
        getrandom::getrandom(&mut secret).map_err(|_| Error::Allocation)?;
        let token = format!("{PREFIX}{}", hex::encode(secret));
        let row_id = self.next_id().await?;
        let inserted = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO webdock_auth.api_token(id,customer_id,session_id,user_id,name,token_hash,can_write,expires_at) \
                 SELECT $1,$2,$3,$4,$5,$6,$7,s.\"expiresAt\" FROM webdock_auth.session s WHERE s.id=$3",
                [row_id.clone().into(), id(customer)?.into(), session.clone().into(), actor.user_id.clone().into(), name.clone().into(), hash(&token).into(), can_write.into()],
            ))
            .await;
        if !inserted.as_ref().is_ok_and(|r| r.rows_affected() == 1) {
            let _ = self.identity.revoke_session(&actor.user_id, &session).await;
            inserted?;
            return Err(Error::Conflict);
        }
        self.audit(&tx, &actor, "api-token-created", Some(customer), false)
            .await?;
        let mut row = json_rows(
            &tx,
            &format!("SELECT {VIEW} FROM webdock_auth.api_token t WHERE t.id=$1"),
            vec![row_id.into()],
        )
        .await?
        .into_iter()
        .next()
        .ok_or(Error::NotFound)?;
        tx.commit().await?;
        row["token"] = json!(token);
        Ok(row)
    }
    /// Ends the token's session; the token goes with it.
    pub async fn revoke_api_token_as(
        &self,
        actor: &Principal,
        customer: &str,
        token: &str,
    ) -> Result<()> {
        if self.current_actor(actor).await?.in_tenant_preview() {
            return Err(Error::Forbidden);
        }
        let (actor, tx) = self.domain_writer(actor, customer).await?;
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.api_token WHERE id=$1 AND customer_id=$2 RETURNING user_id,session_id",
                [id(token)?.into(), id(customer)?.into()],
            ))
            .await?
            .ok_or(Error::NotFound)?;
        let user: String = row.try_get("", "user_id")?;
        let session: String = row.try_get("", "session_id")?;
        self.audit(&tx, &actor, "api-token-revoked", Some(customer), false)
            .await?;
        tx.commit().await?;
        self.identity
            .revoke_session(&user, &session)
            .await
            .map_err(|_| Error::Allocation)?;
        Ok(())
    }
    /// The member, workspace and write permission behind a bearer token, or Unauthorized.
    pub async fn api_actor(&self, bearer: &str) -> Result<ApiActor> {
        let secret = bearer.strip_prefix(PREFIX).ok_or(Error::Unauthorized)?;
        if secret.len() != 64 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Unauthorized);
        }
        let tx = self.business.begin().await?;
        let found: Option<Value> = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.api_token_session($1) AS found",
                [hash(bearer).into()],
            ))
            .await?
            .ok_or(Error::Unauthorized)?
            .try_get("", "found")?;
        tx.commit().await?;
        let found = found.ok_or(Error::Unauthorized)?;
        let field = |key: &str| {
            found[key]
                .as_str()
                .map(str::to_owned)
                .ok_or(Error::Unauthorized)
        };
        let tx = self.auth.begin().await?;
        let principal = self
            .identity
            .authenticate_session_in(&tx, &field("user")?, &field("session")?)
            .await?
            .filter(|p| p.can_access() && !p.in_tenant_preview())
            .ok_or(Error::Unauthorized)?;
        tx.commit().await?;
        Ok(ApiActor {
            principal,
            customer: field("customer")?,
            can_write: found["write"] == true,
        })
    }
}
