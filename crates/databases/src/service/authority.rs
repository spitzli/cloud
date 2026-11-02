//! Retains existing authentication guards for real Browser commands and MCP reads.
//! Worker claim/lease/reconciliation activation still requires the early 0056 epoch fence.
use super::*;
use crate::database_authority::DatabaseAuthority;
use chrono::{DateTime, Utc};
use webdock_oauth::{McpAuthority, McpGuard, OAuthService};

enum Authentication {
    Browser {
        transaction: DatabaseTransaction,
        principal: Box<Principal>,
    },
    Mcp(Box<McpGuard>),
}
pub(super) struct RetainedAuthority {
    proof: DatabaseAuthority,
    authentication: Authentication,
    valid_until: DateTime<Utc>,
}
impl RetainedAuthority {
    pub(super) fn principal(&self) -> &Principal {
        match &self.authentication {
            Authentication::Browser { principal, .. } => principal,
            Authentication::Mcp(guard) => guard.principal(),
        }
    }
    pub(super) fn read_only(&self) -> bool {
        self.proof.read_only()
    }
    pub(super) async fn verify(&self, service: &DatabaseService) -> Result<()> {
        cap_verified_lifetime(Utc::now(), self.valid_until, self.valid_until)?;
        match (&self.proof, &self.authentication) {
            (
                DatabaseAuthority::BrowserStored(proof),
                Authentication::Browser {
                    transaction,
                    principal,
                },
            ) => {
                let current = webdock_browser_session::revalidate_stored_authority(
                    &service.identity,
                    transaction,
                    service.browser.as_deref(),
                    &principal.user_id,
                    &principal.session_id,
                    proof,
                )
                .await
                .map_err(browser_failure)?;
                if current.user_id != principal.user_id
                    || current.session_id != principal.session_id
                    || current.customer_scope() != principal.customer_scope()
                {
                    return Err(denied());
                }
            }
            (DatabaseAuthority::Mcp(_), Authentication::Mcp(guard)) => {
                guard.verify().await.map_err(oauth_failure)?
            }
            _ => return Err(denied()),
        }
        cap_verified_lifetime(Utc::now(), self.valid_until, self.valid_until)?;
        Ok(())
    }
    pub(super) async fn finish(
        self,
        service: &DatabaseService,
        business: DatabaseTransaction,
    ) -> Result<()> {
        self.verify(service).await?;
        business.commit().await?;
        if let Authentication::Browser { transaction, .. } = self.authentication {
            transaction.commit().await?;
        }
        // MCP's request-owned guard is dropped only after the business commit.
        Ok(())
    }
}
impl DatabaseService {
    async fn session_context(&self, session: &str) -> Result<DatabaseTransaction> {
        let tx = self.db.begin().await?;
        tx.execute_unprepared("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='10s'")
            .await?;
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock_shared(hashtextextended($1,0))",
            [format!("native-session-context:{session}").into()],
        ))
        .await?;
        Ok(tx)
    }
    async fn retained_actor(&self, tx: &DatabaseTransaction, current: &Principal) -> Result<()> {
        self.signer
            .apply(tx, current)
            .await
            .map_err(identity_failure)?;
        if current.is_operator() {
            let mapped=one(tx,"SELECT jsonb_build_object('subject',auth_subject,'role',role) AS value FROM webdock_admin.users WHERE auth_subject=$1 FOR SHARE",vec![current.user_id.clone().into()]).await?;
            if mapped["subject"] != current.user_id || mapped["role"] != "operator" {
                return Err(denied());
            }
        }
        Ok(())
    }
    pub(super) async fn retain_browser(
        &self,
        principal: &Principal,
        proof: &StoredAuthority,
    ) -> Result<(DatabaseTransaction, RetainedAuthority)> {
        let business = self.session_context(&principal.session_id).await?;
        let authentication = self.auth.begin().await?;
        let current = webdock_browser_session::revalidate_stored_authority(
            &self.identity,
            &authentication,
            self.browser.as_deref(),
            &principal.user_id,
            &principal.session_id,
            proof,
        )
        .await
        .map_err(browser_failure)?;
        let original_expiry = match proof {
            StoredAuthority::FirstPartyBrowser { browser } => {
                DateTime::from_timestamp(browser.expires_at, 0).ok_or_else(denied)?
            }
            _ => current.expires_at,
        };
        let valid_until = cap_verified_lifetime(Utc::now(), current.expires_at, original_expiry)?;
        self.retained_actor(&business, &current).await?;
        Ok((
            business,
            RetainedAuthority {
                proof: DatabaseAuthority::BrowserStored(proof.clone()),
                authentication: Authentication::Browser {
                    transaction: authentication,
                    principal: Box::new(current),
                },
                valid_until,
            },
        ))
    }
    pub(super) async fn retain_mcp(
        &self,
        oauth: &Arc<OAuthService>,
        reference: &McpAuthority,
    ) -> Result<(DatabaseTransaction, RetainedAuthority)> {
        if reference.registry_scope() {
            return Err(denied());
        }
        let business = self.session_context(reference.session_id()).await?;
        let guard = oauth
            .begin_mcp_authority(reference)
            .await
            .map_err(oauth_failure)?;
        let current = guard.principal();
        if !current.can_access() || current.in_tenant_preview() {
            return Err(denied());
        }
        // The original typed getter is used only AFTER actual retained OAuth verification.
        let original_expiry =
            DateTime::from_timestamp(reference.expires_at(), 0).ok_or_else(denied)?;
        let valid_until = cap_verified_lifetime(Utc::now(), current.expires_at, original_expiry)?;
        self.retained_actor(&business, current).await?;
        Ok((
            business,
            RetainedAuthority {
                proof: DatabaseAuthority::Mcp(Box::new(reference.clone())),
                authentication: Authentication::Mcp(Box::new(guard)),
                valid_until,
            },
        ))
    }
}
/// Arithmetic shared with the staged lease rule; inputs confer no authority.
fn cap_verified_lifetime(
    now: DateTime<Utc>,
    normal: DateTime<Utc>,
    original: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let limit = normal.min(original);
    if limit <= now {
        return Err(denied());
    }
    Ok(limit)
}
pub(super) fn oauth_failure(error: webdock_oauth::Error) -> Error {
    if error.status() >= 500 {
        unavailable()
    } else {
        denied()
    }
}
pub(super) fn browser_failure(error: webdock_browser_session::Error) -> Error {
    use webdock_browser_session::Error as BrowserError;
    match error {
        BrowserError::Unauthorized | BrowserError::InvalidInput => denied(),
        BrowserError::OAuth(error) => oauth_failure(error),
        BrowserError::Identity(error) => identity_failure(error),
        BrowserError::Configuration | BrowserError::Database(_) => unavailable(),
    }
}

#[cfg(test)]
mod staged;

fn identity_failure(error: webdock_identity::IdentityError) -> Error {
    use webdock_identity::IdentityError;
    match error {
        IdentityError::Unauthorized
        | IdentityError::InvalidEncoding
        | IdentityError::InvalidCode
        | IdentityError::SecurityState
        | IdentityError::InvalidInput => denied(),
        IdentityError::Configuration
        | IdentityError::Crypto
        | IdentityError::RateLimited
        | IdentityError::Database(_)
        | IdentityError::Allocation
        | IdentityError::Worker => unavailable(),
    }
}
#[cfg(test)]
mod scope_error_tests {
    use super::*;
    #[test]
    fn business_scope_database_failure_is_transient_not_authority_loss() {
        let error = identity_failure(webdock_identity::IdentityError::Database(
            sea_orm::DbErr::Custom("isolated scope failure".into()),
        ));
        assert_eq!(error.status, 503);
        assert_eq!(
            identity_failure(webdock_identity::IdentityError::Unauthorized).status,
            404
        );
    }
}
