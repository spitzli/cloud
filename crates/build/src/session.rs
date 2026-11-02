//! Holds real session/client authority across browser mutations and manual publication commits.
use super::*;
use webdock_browser_session::{BrowserSessions, StoredAuthority, revalidate_stored_authority};
pub(super) struct SessionVerifier {
    auth: DatabaseConnection,
    identity: Arc<webdock_identity::IdentityService>,
    browser: Option<Arc<BrowserSessions>>,
}
pub(super) struct SessionGuard {
    tx: DatabaseTransaction,
    verifier: Arc<SessionVerifier>,
    subject: String,
    session: String,
    authority: StoredAuthority,
    write: bool,
}
impl SessionVerifier {
    pub async fn new(
        auth: DatabaseConnection,
        identity: Arc<webdock_identity::IdentityService>,
        browser: Option<Arc<BrowserSessions>>,
    ) -> Result<Self> {
        webdock_persistence::connection::verify_runtime_role(&auth, "webdock_auth_bootstrap")
            .await
            .map_err(|_| error("invalid_database_role"))?;
        Ok(Self {
            auth,
            identity,
            browser,
        })
    }
    pub async fn begin(
        self: &Arc<Self>,
        subject: &str,
        session: &str,
        authority: &StoredAuthority,
        write: bool,
    ) -> Result<SessionGuard> {
        let guard = SessionGuard {
            tx: self.auth.begin().await.map_err(db_error)?,
            verifier: self.clone(),
            subject: subject.into(),
            session: session.into(),
            authority: authority.clone(),
            write,
        };
        guard.verify().await?;
        Ok(guard)
    }
}
impl SessionGuard {
    pub fn transaction(&self) -> &DatabaseTransaction {
        &self.tx
    }
    pub async fn verify(&self) -> Result<()> {
        if self.write && self.authority.read_only() {
            return Err(error("permission_denied"));
        }
        let principal = revalidate_stored_authority(
            &self.verifier.identity,
            &self.tx,
            self.verifier.browser.as_deref(),
            &self.subject,
            &self.session,
            &self.authority,
        )
        .await
        .map_err(|_| error("session_authority_revoked"))?;
        if self.write && principal.in_read_only_preview() {
            return Err(error("permission_denied"));
        }
        Ok(())
    }
    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await.map_err(db_error)
    }
}
impl BuildService {
    pub async fn with_session_authority(
        mut self,
        auth: DatabaseConnection,
        identity: Arc<webdock_identity::IdentityService>,
        browser: Option<Arc<BrowserSessions>>,
    ) -> Result<Self> {
        self.authority = Some(Arc::new(
            SessionVerifier::new(auth, identity, browser).await?,
        ));
        Ok(self)
    }
    pub(super) async fn begin_authority(
        &self,
        principal: &Principal,
        authority: Option<&StoredAuthority>,
        write: bool,
    ) -> Result<Option<SessionGuard>> {
        match authority {
            Some(authority) => Ok(Some(
                self.authority
                    .as_ref()
                    .ok_or_else(|| error("session_authority_unavailable"))?
                    .begin(&principal.user_id, &principal.session_id, authority, write)
                    .await?,
            )),
            None => Ok(None),
        }
    }
    pub(super) async fn release_authority(&self, release: &Value) -> Result<Option<SessionGuard>> {
        if release["approved_source"] == "git-policy" {
            return Ok(None);
        }
        let Some(value) = release["publication_target"]
            .get("approvedAuthority")
            .filter(|v| !v.is_null())
        else {
            return Ok(None);
        };
        let authority: StoredAuthority = serde_json::from_value(value.clone())
            .map_err(|_| error("session_authority_revoked"))?;
        Ok(Some(
            self.authority
                .as_ref()
                .ok_or_else(|| error("session_authority_unavailable"))?
                .begin(
                    field(release, "approved_by")?,
                    field(release, "approved_session_id")?,
                    &authority,
                    true,
                )
                .await?,
        ))
    }
}
pub(super) async fn verify(guard: &Option<SessionGuard>) -> Result<()> {
    if let Some(guard) = guard {
        guard.verify().await?;
    }
    Ok(())
}
pub(super) async fn commit(guard: Option<SessionGuard>) -> Result<()> {
    if let Some(guard) = guard {
        guard.commit().await?;
    }
    Ok(())
}
