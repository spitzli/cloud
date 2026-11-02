//! Console-only adapter retains native PKCE authority through Access's commit.
use super::*;
use axum::body::Bytes;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use webdock_access::{AllowanceAuthority, ConfigureAllowances};
use webdock_browser_session::{BrowserAuthority, BrowserSessions};

pub(super) fn routes() -> Router<Arc<ServiceState>> {
    Router::new().route(
        "/api/console/access/customers/{customer}/allowances",
        get(read).post(configure),
    )
}
fn authority_error(
    error: webdock_browser_session::Error,
    client_disabled: bool,
) -> webdock_access::Error {
    use webdock_browser_session::Error as BrowserError;
    use webdock_identity::IdentityError;
    match error {
        BrowserError::Unauthorized
        | BrowserError::InvalidInput
        | BrowserError::Identity(IdentityError::Unauthorized | IdentityError::SecurityState)
        | BrowserError::OAuth(webdock_oauth::Error::Identity(
            IdentityError::Unauthorized | IdentityError::SecurityState,
        )) => webdock_access::Error::Unauthorized,
        BrowserError::OAuth(webdock_oauth::Error::InvalidClient) if client_disabled => {
            webdock_access::Error::Unauthorized
        }
        BrowserError::OAuth(webdock_oauth::Error::InvalidClient) => {
            webdock_access::Error::Configuration
        }
        BrowserError::OAuth(error) if error.status() < 500 => webdock_access::Error::Unauthorized,
        _ => webdock_access::Error::Configuration,
    }
}
async fn browser_failure<C: ConnectionTrait>(
    auth: &C,
    client: &str,
    error: webdock_browser_session::Error,
) -> webdock_access::Error {
    // InvalidClient also means a wrong protected secret or invalid installation.
    // Only a canonical disabled row establishes revocation; no credential is read.
    let disabled = if matches!(
        &error,
        webdock_browser_session::Error::OAuth(webdock_oauth::Error::InvalidClient)
    ) {
        match auth.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT coalesce(disabled,false) AS disabled FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1 FOR SHARE",[client.into()])).await {
            Ok(Some(row)) => row.try_get::<bool>("","disabled").unwrap_or(false),
            _ => false,
        }
    } else {
        false
    };
    authority_error(error, disabled)
}
struct ConsoleAuthority<'a> {
    sessions: &'a BrowserSessions,
    proof: BrowserAuthority,
    subject: String,
    session: String,
}
impl AllowanceAuthority for ConsoleAuthority<'_> {
    async fn revalidate(&self, auth: &DatabaseTransaction) -> webdock_access::Result<Principal> {
        let current = match self.sessions.revalidate_in(auth, &self.proof).await {
            Ok(current) => current,
            Err(error) => {
                return Err(browser_failure(auth, &self.sessions.config.client_id, error).await);
            }
        };
        if current.authority.read_only
            || current.authority.customer_scope.is_some()
            || current.principal.user_id != self.subject
            || current.principal.session_id != self.session
            || !current.principal.is_operator()
        {
            return Err(webdock_access::Error::Forbidden);
        }
        Ok(current.principal)
    }
}
async fn authority<'a>(
    state: &'a ServiceState,
    headers: &HeaderMap,
) -> Result<(&'a AccessService, ConsoleAuthority<'a>), ApiFailure> {
    let service = state
        .access
        .as_deref()
        .ok_or_else(|| failure(webdock_access::Error::Configuration))?;
    if crate::browser::request_origin(state, headers) != Some(state.config.console_origin()) {
        return Err(failure(webdock_access::Error::Forbidden));
    }
    let sessions = state
        .browser
        .as_deref()
        .ok_or_else(|| failure(webdock_access::Error::Configuration))?;
    // This endpoint accepts only the same server-owned first-party browser proof;
    // typed authority failures must stay distinct from persistence/config outages.
    let actor = match sessions
        .resolve_authority(state.config.console_origin(), &auth::cookie(headers)?)
        .await
    {
        Ok(Some(actor)) => actor,
        Ok(None) => return Err(failure(webdock_access::Error::Unauthorized)),
        Err(error) => {
            return Err(failure(
                browser_failure(&state.db, &sessions.config.client_id, error).await,
            ));
        }
    };
    let browser = actor.authority;
    if browser.app_origin != state.config.console_origin()
        || browser.read_only
        || !actor.principal.is_operator()
        || actor.principal.customer_scope().is_some()
    {
        return Err(failure(webdock_access::Error::Forbidden));
    }
    Ok((
        service,
        ConsoleAuthority {
            sessions,
            proof: browser,
            subject: actor.principal.user_id,
            session: actor.principal.session_id,
        },
    ))
}
async fn read(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(customer): Path<String>,
) -> Response {
    output(
        async {
            let (service, authority) = authority(&state, &headers).await?;
            service
                .read_allowances(&authority, &customer)
                .await
                .map_err(failure)
        }
        .await,
    )
}
async fn configure(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(customer): Path<String>,
    body: Bytes,
) -> Response {
    output(
        async {
            let (service, authority) = authority(&state, &headers).await?;
            if body.len() > 4096 {
                return Err(failure(webdock_access::Error::InvalidInput));
            }
            if headers.get_all("content-type").iter().count() != 1
                || headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.split(';').next())
                    .map(str::trim)
                    != Some("application/json")
            {
                return Err(failure(webdock_access::Error::InvalidInput));
            }
            let input: ConfigureAllowances = serde_json::from_slice(&body)
                .map_err(|_| failure(webdock_access::Error::InvalidInput))?;
            service
                .configure_allowances(&authority, &customer, input)
                .await
                .map_err(failure)
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use webdock_browser_session::Error as BrowserError;
    #[test]
    fn revoked_authority_is_distinct_from_persistence_and_config_outages() {
        for error in [
            BrowserError::Unauthorized,
            BrowserError::OAuth(webdock_oauth::Error::InvalidGrant),
            BrowserError::OAuth(webdock_oauth::Error::AccessDenied),
            BrowserError::OAuth(webdock_oauth::Error::Identity(
                webdock_identity::IdentityError::SecurityState,
            )),
        ] {
            assert_eq!(authority_error(error, false).status(), 401);
        }
        assert_eq!(
            authority_error(
                BrowserError::OAuth(webdock_oauth::Error::InvalidClient),
                true
            )
            .status(),
            401
        );
        // False includes active+wrong secret, missing canonical row and failed lookup.
        assert_eq!(
            authority_error(
                BrowserError::OAuth(webdock_oauth::Error::InvalidClient),
                false
            )
            .status(),
            503
        );
        for error in [
            BrowserError::Configuration,
            BrowserError::Database(sea_orm::DbErr::Custom(
                "test persistence unavailable".into(),
            )),
            BrowserError::OAuth(webdock_oauth::Error::Crypto),
            BrowserError::OAuth(webdock_oauth::Error::Database(sea_orm::DbErr::Custom(
                "test persistence unavailable".into(),
            ))),
        ] {
            assert_eq!(authority_error(error, true).status(), 503);
        }
    }
}
