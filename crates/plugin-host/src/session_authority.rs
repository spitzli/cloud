use crate::db_error;
use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};
use spitzli_browser_session::{BrowserSessions, StoredAuthority, revalidate_stored_authority};
use spitzli_identity::IdentityService;
use spitzli_plugin_protocol::{PluginError, Result};
use std::sync::Arc;
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum AuthorityReference {
    Browser {
        authority: StoredAuthority,
    },
    Mcp {
        authority: spitzli_oauth::McpAuthority,
    },
}

pub(crate) struct SessionAuthority {
    auth: DatabaseConnection,
    identity: Arc<IdentityService>,
    browser: Option<Arc<BrowserSessions>>,
    pub mcp: Option<Arc<spitzli_oauth::OAuthService>>,
}
impl SessionAuthority {
    pub async fn new(
        auth: DatabaseConnection,
        identity: Arc<IdentityService>,
        browser: Option<Arc<BrowserSessions>>,
    ) -> Result<Self> {
        spitzli_persistence::connection::verify_runtime_role(&auth, "webdock_auth_bootstrap")
            .await
            .map_err(|_| PluginError::denied())?;
        Ok(Self {
            auth,
            identity,
            browser,
            mcp: None,
        })
    }
    pub(crate) async fn begin_principal(
        &self,
        subject: &str,
        session: &str,
        authority: &AuthorityReference,
        write: bool,
    ) -> Result<(DatabaseTransaction, spitzli_identity::Principal)> {
        let tx = self.auth.begin().await.map_err(db_error)?;
        let principal = self
            .principal(&tx, subject, session, authority, write)
            .await?;
        Ok((tx, principal))
    }
    pub async fn begin(
        &self,
        subject: &str,
        session: &str,
        authority: &AuthorityReference,
        write: bool,
    ) -> Result<DatabaseTransaction> {
        let tx = self.auth.begin().await.map_err(db_error)?;
        self.verify(&tx, subject, session, authority, write).await?;
        Ok(tx)
    }
    pub async fn verify(
        &self,
        tx: &DatabaseTransaction,
        subject: &str,
        session: &str,
        authority: &AuthorityReference,
        write: bool,
    ) -> Result<()> {
        self.principal(tx, subject, session, authority, write)
            .await?;
        Ok(())
    }
    pub async fn begin_operator(
        &self,
        subject: &str,
        session: &str,
        authority: &spitzli_browser_session::StoredAuthority,
    ) -> Result<DatabaseTransaction> {
        let tx = self.auth.begin().await.map_err(db_error)?;
        let principal = self
            .principal(
                &tx,
                subject,
                session,
                &AuthorityReference::Browser {
                    authority: authority.clone(),
                },
                true,
            )
            .await?;
        if !principal.is_operator() {
            return Err(PluginError::denied());
        }
        Ok(tx)
    }
    async fn principal(
        &self,
        tx: &DatabaseTransaction,
        subject: &str,
        session: &str,
        authority: &AuthorityReference,
        write: bool,
    ) -> Result<spitzli_identity::Principal> {
        let principal = match authority {
            AuthorityReference::Browser { authority } => {
                if write && authority.read_only() {
                    return Err(PluginError::denied());
                }
                revalidate_stored_authority(
                    &self.identity,
                    tx,
                    self.browser.as_deref(),
                    subject,
                    session,
                    authority,
                )
                .await
                .map_err(browser_error)?
            }
            AuthorityReference::Mcp { authority } => {
                if write && !authority.can_write() {
                    return Err(PluginError::denied());
                }
                self.mcp
                    .as_ref()
                    .ok_or_else(PluginError::denied)?
                    .revalidate_mcp_authority_in(tx, authority)
                    .await
                    .map_err(oauth_error)?
            }
        };
        if principal.user_id != subject
            || principal.session_id != session
            || (write && principal.in_read_only_preview())
        {
            return Err(PluginError::denied());
        }
        Ok(principal)
    }
}

fn identity_error(error: spitzli_identity::IdentityError) -> PluginError {
    use spitzli_identity::IdentityError as E;
    match error {
        E::Database(_)
        | E::Configuration
        | E::Crypto
        | E::Allocation
        | E::Worker
        | E::Persistence => crate::unavailable(),
        _ => PluginError::denied(),
    }
}
fn oauth_error(error: spitzli_oauth::Error) -> PluginError {
    use spitzli_oauth::Error as E;
    match error {
        E::Identity(error) => identity_error(error),
        E::Database(_) | E::Configuration | E::Crypto | E::Allocation => crate::unavailable(),
        _ => PluginError::denied(),
    }
}
fn browser_error(error: spitzli_browser_session::Error) -> PluginError {
    use spitzli_browser_session::Error as E;
    match error {
        E::Identity(error) => identity_error(error),
        E::OAuth(error) => oauth_error(error),
        E::Database(_) | E::Configuration => crate::unavailable(),
        _ => PluginError::denied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spitzli_plugin_protocol::ErrorCode;
    #[test]
    fn verifier_infrastructure_is_retryable_and_denial_is_terminal() {
        let database = || sea_orm::DbErr::Custom("private diagnostic".into());
        for error in [
            spitzli_browser_session::Error::Database(database()),
            spitzli_browser_session::Error::Identity(spitzli_identity::IdentityError::Persistence),
            spitzli_browser_session::Error::Identity(spitzli_identity::IdentityError::Database(
                database(),
            )),
            spitzli_browser_session::Error::OAuth(spitzli_oauth::Error::Database(database())),
        ] {
            assert_eq!(browser_error(error).code, ErrorCode::Unavailable);
        }
        assert_eq!(
            browser_error(spitzli_browser_session::Error::Unauthorized).code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            oauth_error(spitzli_oauth::Error::AccessDenied).code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            oauth_error(spitzli_oauth::Error::Identity(
                spitzli_identity::IdentityError::Database(database())
            ))
            .code,
            ErrorCode::Unavailable
        );
    }
}
