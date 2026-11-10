//! Canonical session reads only. Other identity writes retain explicit legacy paths.
use super::Principal;
use crate::{IdentityError, Result};
use spitzli_persistence::postgres::{Pool, Transaction};
use std::time::Duration;
use tokio_postgres::types::Type;

// Both functions are established database authorities. Missing/unknown schema is an error,
// never a compatibility grant or an automatic fallback to the legacy reader.
const SESSION: &str = r#"SELECT u.id,u.name,u.email,u.role,u."emailVerified",u."mustChangePassword",coalesce(u."twoFactorEnabled",false) AS "twoFactorEnabled",s.id AS session_id,s."expiresAt",webdock_rls.identity_has_preview(u.id,s.id) AS preview,webdock_rls.identity_is_acting(u.id,s.id) AS acting,webdock_rls.identity_customer_scope(u.id,s.id) AS customer_scope
FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id
WHERE s.token=$1 AND s."expiresAt">statement_timestamp() AND NOT coalesce(u.banned,false)
AND webdock_rls.local_identity_authority(u.id) AND webdock_rls.native_provider_session_live(u.id,s.id)"#;

pub(super) async fn authenticate(pool: &Pool, token: &str) -> Result<Option<Principal>> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut connection = pool
            .acquire()
            .await
            .map_err(|_| IdentityError::Persistence)?;
        let tx = connection
            .transaction()
            .await
            .map_err(|_| IdentityError::Persistence)?;
        let row = tx
            .query_typed_opt(SESSION, &[(&token, Type::TEXT)])
            .await
            .map_err(|_| IdentityError::Persistence)?;
        let principal = row
            .as_ref()
            .map(decode)
            .transpose()
            .map_err(|_| IdentityError::Persistence)?;
        tx.commit().await.map_err(|_| IdentityError::Persistence)?;
        Ok(principal)
    })
    .await
    .map_err(|_| IdentityError::Persistence)?
}

fn decode(row: &tokio_postgres::Row) -> std::result::Result<Principal, tokio_postgres::Error> {
    Ok(Principal {
        user_id: row.try_get("id")?,
        session_id: row.try_get("session_id")?,
        name: row.try_get("name")?,
        email: row.try_get("email")?,
        expires_at: row.try_get("expiresAt")?,
        role: row
            .try_get::<_, Option<String>>("role")?
            .unwrap_or_default(),
        email_verified: row.try_get("emailVerified")?,
        must_change_password: row.try_get("mustChangePassword")?,
        two_factor_enabled: row.try_get("twoFactorEnabled")?,
        tenant_preview: row.try_get("preview")?,
        tenant_acting: row.try_get("acting")?,
        customer_scope: row.try_get("customer_scope")?,
    })
}

// The canonical row locks precede a fresh-statement policy check: provider state may
// change while this query waits, independently of the locked canonical rows.
const FENCED_SESSION: &str = r#"SELECT u.id FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id WHERE u.id=$1 AND s.id=$2 AND s."expiresAt">statement_timestamp() AND NOT coalesce(u.banned,false) FOR SHARE OF u,s"#;
const LOCKED_SESSION_FACTS: &str = r#"SELECT u.id,u.name,u.email,u.role,u."emailVerified",u."mustChangePassword",coalesce(u."twoFactorEnabled",false) AS "twoFactorEnabled",s.id AS session_id,s."expiresAt",webdock_rls.identity_has_preview(u.id,s.id) AS preview,webdock_rls.identity_is_acting(u.id,s.id) AS acting,webdock_rls.identity_customer_scope(u.id,s.id) AS customer_scope
FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id WHERE u.id=$1 AND s.id=$2 AND s."expiresAt">statement_timestamp() AND NOT coalesce(u.banned,false)
AND CASE WHEN webdock_rls.local_identity_authority(u.id) THEN webdock_rls.native_provider_session_live(u.id,s.id) ELSE false END"#;
pub(super) async fn live(tx: &Transaction<'_>, user: &str, session: &str) -> Result<bool> {
    tx.query_typed_one("SELECT EXISTS(SELECT 1 FROM webdock_auth.\"user\" u JOIN webdock_auth.session s ON s.\"userId\"=u.id WHERE u.id=$1 AND s.id=$2 AND s.\"expiresAt\">statement_timestamp() AND NOT coalesce(u.banned,false) AND u.role IN ('user','operator') AND CASE WHEN webdock_rls.local_identity_authority(u.id) THEN webdock_rls.native_provider_session_live(u.id,s.id) ELSE false END) AS allowed", &[(&user,Type::TEXT),(&session,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.try_get("allowed").map_err(|_|IdentityError::Persistence)
}
pub(super) async fn revalidate<'a>(
    tx: Transaction<'a>,
    user: &str,
    session: &str,
) -> Result<(Transaction<'a>, Option<Principal>)> {
    tokio::time::timeout(Duration::from_secs(30), async move {
        if tx
            .query_typed_opt(
                FENCED_SESSION,
                &[(&user, Type::TEXT), (&session, Type::TEXT)],
            )
            .await
            .map_err(|_| IdentityError::Persistence)?
            .is_none()
        {
            return Ok((tx, None));
        }
        // Read all authority and acting facts in a fresh statement after the locks,
        // rather than returning the projection captured before a lock wait.
        let row = tx
            .query_typed_opt(
                LOCKED_SESSION_FACTS,
                &[(&user, Type::TEXT), (&session, Type::TEXT)],
            )
            .await
            .map_err(|_| IdentityError::Persistence)?;
        let principal = row
            .as_ref()
            .map(decode)
            .transpose()
            .map_err(|_| IdentityError::Persistence)?;
        Ok((tx, principal))
    })
    .await
    .map_err(|_| IdentityError::Persistence)?
}
