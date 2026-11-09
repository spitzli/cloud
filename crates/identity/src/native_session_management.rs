//! Explicit native session domain operations, retaining canonical lock/authority order.
use super::{IdentityService, native_session};
use crate::{IdentityError, IssuedSession, Result, SessionSummary};
use std::time::Duration;
use tokio_postgres::types::Type;
use webdock_persistence::postgres::{Connection, Transaction};
const BUDGET: Duration = Duration::from_secs(30);
impl IdentityService {
    async fn management_connection(&self) -> Result<Connection> {
        self.native_management_pool()
            .ok_or(IdentityError::Configuration)?
            .acquire()
            .await
            .map_err(|_| IdentityError::Persistence)
    }
    async fn locked_native_actor(
        &self,
        tx: &Transaction<'_>,
        token: &str,
    ) -> Result<(String, String)> {
        let row=tx.query_typed_opt(r#"SELECT u.id,s.id AS session_id FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id WHERE s.token=$1 AND s."expiresAt">statement_timestamp() AND NOT coalesce(u.banned,false) AND u.role IN ('user','operator') FOR UPDATE OF u,s"#,&[(&token,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
        let user: String = row.try_get("id").map_err(|_| IdentityError::Persistence)?;
        let session: String = row
            .try_get("session_id")
            .map_err(|_| IdentityError::Persistence)?;
        // A separate statement is intentional: observe provider/local revocation committed
        // while the canonical user/session row lock was being acquired.
        if !native_session::live(tx, &user, &session).await? {
            return Err(IdentityError::Unauthorized);
        }
        Ok((user, session))
    }
    // Trusted workspace-token issuance. This never returns a canonical cookie token,
    // and workspace authorization/binding remains the AccessService caller's job.
    pub(crate) async fn native_issue_api_session(
        &self,
        user: &str,
        label: &str,
        days: i64,
    ) -> Result<String> {
        tokio::time::timeout(BUDGET,async {
            let allocator=self.allocator.as_ref().ok_or(IdentityError::Configuration)?;
            let mut connection=self.management_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let id=allocator.allocate().await.map_err(|_|IdentityError::Allocation)?.to_string();
            let token=hex::encode(crate::crypto::random::<32>()?);let days=days as i32;
            if tx.execute_typed(r#"INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt","userAgent") VALUES($1,$2,$3,statement_timestamp()+make_interval(days=>$4::int),statement_timestamp(),$5)"#,&[(&id,Type::TEXT),(&token,Type::TEXT),(&user,Type::TEXT),(&days,Type::INT4),(&label,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?!=1{return Err(IdentityError::Persistence);}
            tx.commit().await.map_err(|_|IdentityError::Persistence)?;Ok(id)
        }).await.map_err(|_|IdentityError::Persistence)?
    }
    pub(crate) async fn native_revoke_session(&self, user: &str, session: &str) -> Result<bool> {
        tokio::time::timeout(BUDGET, async {
            let mut connection = self.management_connection().await?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| IdentityError::Persistence)?;
            let changed = tx
                .execute_typed(
                    r#"DELETE FROM webdock_auth.session WHERE id=$1 AND "userId"=$2"#,
                    &[(&session, Type::TEXT), (&user, Type::TEXT)],
                )
                .await
                .map_err(|_| IdentityError::Persistence)?
                == 1;
            tx.commit().await.map_err(|_| IdentityError::Persistence)?;
            Ok(changed)
        })
        .await
        .map_err(|_| IdentityError::Persistence)?
    }
    pub(crate) async fn native_list_sessions(&self, cookie: &str) -> Result<Vec<SessionSummary>> {
        let token = self
            .cookies
            .read(cookie)?
            .ok_or(IdentityError::Unauthorized)?;
        tokio::time::timeout(BUDGET,async {
            let mut connection=self.management_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let(user,current)=self.locked_native_actor(&tx,&token).await?;
            let rows=tx.query_typed(r#"SELECT id,"expiresAt","createdAt" FROM webdock_auth.session WHERE "userId"=$1 AND "expiresAt">statement_timestamp() ORDER BY "createdAt",id"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            let sessions=rows.iter().map(|row|->std::result::Result<SessionSummary,tokio_postgres::Error>{let id:String=row.try_get("id")?;Ok(SessionSummary{current:id==current,id,expires_at:row.try_get("expiresAt")?,created_at:row.try_get("createdAt")?})}).collect::<std::result::Result<Vec<_>,_>>().map_err(|_|IdentityError::Persistence)?;
            tx.commit().await.map_err(|_|IdentityError::Persistence)?;Ok(sessions)
        }).await.map_err(|_|IdentityError::Persistence)?
    }
    pub(crate) async fn native_revoke_other_sessions(&self, cookie: &str) -> Result<u64> {
        let token = self
            .cookies
            .read(cookie)?
            .ok_or(IdentityError::Unauthorized)?;
        tokio::time::timeout(BUDGET, async {
            let mut connection = self.management_connection().await?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| IdentityError::Persistence)?;
            let (user, current) = self.locked_native_actor(&tx, &token).await?;
            tx.execute_typed(
                "DELETE FROM webdock_auth.identity_mfa_challenge WHERE user_id=$1",
                &[(&user, Type::TEXT)],
            )
            .await
            .map_err(|_| IdentityError::Persistence)?;
            let changed = tx
                .execute_typed(
                    r#"DELETE FROM webdock_auth.session WHERE "userId"=$1 AND id<>$2"#,
                    &[(&user, Type::TEXT), (&current, Type::TEXT)],
                )
                .await
                .map_err(|_| IdentityError::Persistence)?;
            tx.commit().await.map_err(|_| IdentityError::Persistence)?;
            Ok(changed)
        })
        .await
        .map_err(|_| IdentityError::Persistence)?
    }
    pub(crate) async fn native_revoke_target(
        &self,
        cookie: &str,
        target: &str,
        by_token: bool,
    ) -> Result<bool> {
        let token = self
            .cookies
            .read(cookie)?
            .ok_or(IdentityError::Unauthorized)?;
        tokio::time::timeout(BUDGET, async {
            let mut connection = self.management_connection().await?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| IdentityError::Persistence)?;
            let (user, _) = self.locked_native_actor(&tx, &token).await?;
            let sql = if by_token {
                r#"DELETE FROM webdock_auth.session WHERE token=$1 AND "userId"=$2"#
            } else {
                r#"DELETE FROM webdock_auth.session WHERE id=$1 AND "userId"=$2"#
            };
            let changed = tx
                .execute_typed(sql, &[(&target, Type::TEXT), (&user, Type::TEXT)])
                .await
                .map_err(|_| IdentityError::Persistence)?
                == 1;
            tx.commit().await.map_err(|_| IdentityError::Persistence)?;
            Ok(changed)
        })
        .await
        .map_err(|_| IdentityError::Persistence)?
    }
    pub(crate) async fn native_refresh_session(&self, cookie: &str) -> Result<IssuedSession> {
        let token = self
            .cookies
            .read(cookie)?
            .ok_or(IdentityError::Unauthorized)?;
        tokio::time::timeout(BUDGET,async {
            let set_cookie={
                let mut connection=self.management_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
                let(_,session)=self.locked_native_actor(&tx,&token).await?;
                let row=tx.query_typed_opt(r#"UPDATE webdock_auth.session SET "expiresAt"=CASE WHEN "updatedAt"<=statement_timestamp()-interval '15 minutes' THEN statement_timestamp()+interval '8 hours' ELSE "expiresAt" END,"updatedAt"=CASE WHEN "updatedAt"<=statement_timestamp()-interval '15 minutes' THEN statement_timestamp() ELSE "updatedAt" END WHERE id=$1 AND "expiresAt">statement_timestamp() RETURNING token,greatest(1,extract(epoch FROM ("expiresAt"-statement_timestamp()))::bigint) AS age"#,&[(&session,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
                let token:String=row.try_get("token").map_err(|_|IdentityError::Persistence)?;let age:i64=row.try_get("age").map_err(|_|IdentityError::Persistence)?;
                let cookie=self.cookies.header(&token,age.try_into().map_err(|_|IdentityError::InvalidInput)?)?;
                tx.commit().await.map_err(|_|IdentityError::Persistence)?;cookie
            }; // Release the connection before rereading, including in max1 pools.
            let principal=self.authenticate_cookie(set_cookie.split(';').next().expect("cookie")).await?.ok_or(IdentityError::Unauthorized)?;
            Ok(IssuedSession{principal,set_cookie})
        }).await.map_err(|_|IdentityError::Persistence)?
    }
    pub(crate) async fn native_logout(&self, cookie: &str) -> Result<()> {
        let Some(token) = self.cookies.read(cookie)? else {
            return Ok(());
        };
        let deadline = tokio::time::Instant::now() + BUDGET;
        let provider_cookie=tokio::time::timeout_at(deadline,async {
            let mut connection=self.management_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let row=tx.query_typed_opt(r#"SELECT id,"userId" FROM webdock_auth.session WHERE token=$1 FOR UPDATE"#,&[(&token,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            let mut provider_cookie=None;
            if let Some(row)=row {
                let user:String=row.try_get("userId").map_err(|_|IdentityError::Persistence)?;let session:String=row.try_get("id").map_err(|_|IdentityError::Persistence)?;
                if let Some(provider)=tx.query_typed_opt("SELECT provider_subject::text,provider_session_id::text,encrypted_cookie FROM webdock_auth.neon_session WHERE session_id=$1 AND user_id=$2",&[(&session,Type::TEXT),(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)? {
                    provider_cookie=Some(self.decrypt_provider_cookie(&user,&provider.try_get::<_,String>("provider_subject").map_err(|_|IdentityError::Persistence)?,&provider.try_get::<_,String>("provider_session_id").map_err(|_|IdentityError::Persistence)?,&provider.try_get::<_,String>("encrypted_cookie").map_err(|_|IdentityError::Persistence)?)?);
                }
                tx.execute_typed(r#"DELETE FROM webdock_auth.session WHERE id=$1 AND "userId"=$2"#,&[(&session,Type::TEXT),(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            }
            tx.commit().await.map_err(|_|IdentityError::Persistence)?;Ok::<_,IdentityError>(provider_cookie)
        }).await.map_err(|_|IdentityError::Persistence)??;
        // Once local revocation is confirmed, provider unavailability cannot undo logout.
        if let (Some(client), Some(cookie)) = (&self.neon, provider_cookie) {
            let _ = tokio::time::timeout_at(deadline, client.sign_out(&cookie)).await;
        }
        Ok(())
    }
}
