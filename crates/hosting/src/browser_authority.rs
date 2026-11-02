//! Manual Git deployment authority remains tied to the exact first-party browser grant.
use super::*;
use webdock_browser_session::{BrowserSessions, StoredAuthority, revalidate_stored_authority};
pub(super) struct BrowserVerifier {
    identity: Arc<webdock_identity::IdentityService>,
    browser: Option<Arc<BrowserSessions>>,
}
pub(super) struct BrowserGuard {
    tx: DatabaseTransaction,
    verifier: Arc<BrowserVerifier>,
    subject: String,
    session: String,
    authority: StoredAuthority,
}
impl BrowserGuard {
    pub async fn verify(&self) -> Result<()> {
        self.verify_expiry().await.map(|_| ())
    }
    pub async fn verify_expiry(&self) -> Result<chrono::DateTime<chrono::Utc>> {
        if self.authority.read_only() {
            return Err(error(403, "Git approval is read-only."));
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
        .map_err(|failure| match failure {
            webdock_browser_session::Error::Unauthorized
            | webdock_browser_session::Error::Identity(
                webdock_identity::IdentityError::Unauthorized
                | webdock_identity::IdentityError::SecurityState,
            )
            | webdock_browser_session::Error::OAuth(
                webdock_oauth::Error::AccessDenied
                | webdock_oauth::Error::InvalidGrant
                | webdock_oauth::Error::InvalidClient
                | webdock_oauth::Error::InvalidScope
                | webdock_oauth::Error::LoginRequired,
            ) => error(403, "Git browser approval is no longer valid."),
            _ => error(503, "Git browser authority is unavailable."),
        })?;
        if principal.in_read_only_preview() {
            return Err(error(403, "Git approval is read-only."));
        }
        Ok(principal.expires_at)
    }
}
impl HostingService {
    pub fn with_session_authority(
        mut self,
        identity: Arc<webdock_identity::IdentityService>,
        browser: Option<Arc<BrowserSessions>>,
    ) -> Self {
        self.browser_authority = Some(Arc::new(BrowserVerifier { identity, browser }));
        self
    }
    pub(super) async fn browser_guard(
        &self,
        subject: &str,
        session: &str,
        proof: &Value,
    ) -> Result<BrowserGuard> {
        let authority = serde_json::from_value(proof.clone())
            .map_err(|_| error(403, "Git browser approval is invalid."))?;
        let guard = BrowserGuard {
            tx: self.auth.begin().await.map_err(db_error)?,
            verifier: self
                .browser_authority
                .as_ref()
                .ok_or_else(|| error(503, "Git browser authority is not configured."))?
                .clone(),
            subject: subject.into(),
            session: session.into(),
            authority,
        };
        guard.verify().await?;
        Ok(guard)
    }
    pub(super) async fn operation_browser_guard(&self, op: &Value) -> Result<Option<BrowserGuard>> {
        if op["git_release_id"].is_null() {
            return Ok(None);
        }
        let Some(proof) = op["desired"]
            .get("gitApprovalAuthority")
            .filter(|v| !v.is_null())
        else {
            return Ok(None);
        };
        let subject = field(&op["desired"], "gitApprovalSubject")?;
        let session = field(&op["desired"], "gitApprovalSession")?;
        if op["subject"] != subject || op["actor_session"] != session {
            return Err(error(403, "Git approval binding changed."));
        }
        Ok(Some(self.browser_guard(subject, session, proof).await?))
    }
    pub(super) async fn verify_release_browser_in(
        &self,
        release: &Value,
        auth: Option<&DatabaseTransaction>,
    ) -> Result<()> {
        let Some(tx) = auth else {
            return Ok(());
        };
        if release["approved_source"] == "git-policy" {
            return Ok(());
        }
        let Some(proof) = release["publication_target"]
            .get("approvedAuthority")
            .filter(|v| !v.is_null())
        else {
            return Ok(());
        };
        let authority: StoredAuthority = serde_json::from_value(proof.clone())
            .map_err(|_| error(403, "Git browser approval is invalid."))?;
        if authority.read_only() {
            return Err(error(403, "Git approval is read-only."));
        }
        let verifier = self
            .browser_authority
            .as_ref()
            .ok_or_else(|| error(503, "Git browser authority is not configured."))?;
        let principal = revalidate_stored_authority(
            &verifier.identity,
            tx,
            verifier.browser.as_deref(),
            field(release, "approved_by")?,
            field(release, "approved_session_id")?,
            &authority,
        )
        .await
        .map_err(|_| error(403, "Git browser approval is no longer valid."))?;
        if principal.in_read_only_preview() {
            return Err(error(403, "Git approval is read-only."));
        }
        Ok(())
    }
    pub(super) async fn release_browser_guard(
        &self,
        release: &Value,
    ) -> Result<Option<BrowserGuard>> {
        if release["approved_source"] == "git-policy" {
            return Ok(None);
        }
        let Some(proof) = release["publication_target"]
            .get("approvedAuthority")
            .filter(|v| !v.is_null())
        else {
            return Ok(None);
        };
        Ok(Some(
            self.browser_guard(
                field(release, "approved_by")?,
                field(release, "approved_session_id")?,
                proof,
            )
            .await?,
        ))
    }
}
pub(super) async fn verify(guard: &Option<BrowserGuard>) -> Result<()> {
    if let Some(guard) = guard {
        guard.verify().await?;
    }
    Ok(())
}
