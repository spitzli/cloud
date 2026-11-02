use super::*;
use sea_orm::TransactionTrait;
use webdock_browser_session::revalidate_stored_authority;

/// Identity locks remain live, but customer/domain locks exist only on the worker transaction.
pub(super) struct ActorGuard {
    auth: DatabaseTransaction,
    pub(super) expires_at: DateTime<Utc>,
}
impl ActorGuard {
    pub(super) async fn commit(self) -> Result<()> {
        self.auth.commit().await.map_err(|_| Error::Unavailable)
    }
}
pub(super) fn denied(error: &webdock_browser_session::Error) -> bool {
    use webdock_browser_session::Error as BrowserError;
    use webdock_identity::IdentityError;
    use webdock_oauth::Error as OAuthError;
    matches!(
        error,
        BrowserError::Unauthorized
            | BrowserError::Identity(IdentityError::Unauthorized)
            | BrowserError::OAuth(
                OAuthError::InvalidClient
                    | OAuthError::InvalidGrant
                    | OAuthError::InvalidScope
                    | OAuthError::AccessDenied
                    | OAuthError::LoginRequired
                    | OAuthError::ConsentRequired
            )
    )
}
impl IncomingAgent {
    pub(super) async fn actor(&self, record: &Record) -> Result<Option<ActorGuard>> {
        if !record.fence.module_available
            || (record.enabled && (!record.fence.customer_active || !record.fence.mail_enabled))
        {
            return Ok(None);
        }
        let stored: StoredAuthority = decode(&record.row, "native_authority")?;
        if stored.read_only() {
            return Ok(None);
        }
        let subject: String = field(&record.row, "subject")?;
        let session: String = field(&record.row, "session_id")?;
        if matches!(stored, StoredAuthority::FirstPartyBrowser { .. })
            && self.incoming.dependencies.browser.is_none()
        {
            return Err(Error::Unavailable);
        }
        let auth = self
            .incoming
            .dependencies
            .auth
            .begin()
            .await
            .map_err(|_| Error::Unavailable)?;
        one(&auth,"SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",vec![]).await?;
        let principal = match revalidate_stored_authority(
            &self.incoming.dependencies.identity,
            &auth,
            self.incoming.dependencies.browser.as_deref(),
            &subject,
            &session,
            &stored,
        )
        .await
        {
            Ok(principal) => principal,
            Err(error) if denied(&error) => return Ok(None),
            Err(_) => return Err(Error::Unavailable),
        };
        if !principal.is_operator() {
            return Ok(None);
        }
        Ok(Some(ActorGuard {
            auth,
            expires_at: principal.expires_at,
        }))
    }
}
