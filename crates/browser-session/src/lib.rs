//! First-party browser sessions. OAuth credentials stay server-side.
mod legacy;
mod service;
pub use legacy::LegacyCookies;
use serde::{Deserialize, Serialize};
pub use service::*;
use url::Url;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Sign-in could not be verified.")]
    Unauthorized,
    #[error("Invalid browser request.")]
    InvalidInput,
    #[error("Browser sign-in is unavailable.")]
    Configuration,
    #[error("Browser persistence is unavailable.")]
    Database(#[from] sea_orm::DbErr),
    #[error("Account authority is unavailable.")]
    Identity(#[from] webdock_identity::IdentityError),
    #[error("Sign-in authority is unavailable.")]
    OAuth(#[from] webdock_oauth::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
/// Server configuration contains a confidential client credential; never log or serialize it.
pub struct BrowserConfig {
    pub app_origin: Url,
    pub issuer: Url,
    pub client_id: String,
    pub(crate) client_secret: String,
    pub(crate) additional_origins: Vec<Url>,
}
impl BrowserConfig {
    pub fn new(
        app_origin: &str,
        issuer: &str,
        client_id: String,
        client_secret: String,
    ) -> Result<Self> {
        let app = origin(app_origin)?;
        let issuer = Url::parse(issuer).map_err(|_| Error::Configuration)?;
        origin(&format!(
            "{}://{}",
            issuer.scheme(),
            &issuer[url::Position::BeforeHost..url::Position::AfterPort]
        ))?;
        if app.scheme() != issuer.scheme()
            || issuer.path() != "/api/auth"
            || issuer.query().is_some()
            || issuer.fragment().is_some()
            || !issuer.username().is_empty()
            || issuer.password().is_some()
            || client_id.is_empty()
            || client_secret.is_empty()
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            app_origin: app,
            issuer,
            client_id,
            client_secret,
            additional_origins: vec![],
        })
    }
    pub fn with_additional_origins(mut self, origins: &[String]) -> Result<Self> {
        if origins.len() > 3 {
            return Err(Error::Configuration);
        }
        self.additional_origins = origins.iter().map(|v| origin(v)).collect::<Result<_>>()?;
        if self
            .additional_origins
            .iter()
            .any(|o| o.scheme() != self.app_origin.scheme())
        {
            return Err(Error::Configuration);
        }
        Ok(self)
    }
    pub(crate) fn app(&self, value: &str) -> Result<&Url> {
        std::iter::once(&self.app_origin)
            .chain(&self.additional_origins)
            .find(|o| o.origin().ascii_serialization() == value)
            .ok_or(Error::InvalidInput)
    }
    pub(crate) fn credentials(&self) -> webdock_oauth::ClientAuthentication {
        webdock_oauth::ClientAuthentication {
            client_id: self.client_id.clone(),
            client_secret: Some(self.client_secret.clone()),
            method: webdock_oauth::ClientAuthMethod::ClientSecretPost,
        }
    }
}
fn origin(raw: &str) -> Result<Url> {
    let u = Url::parse(raw).map_err(|_| Error::Configuration)?;
    let local = u.scheme() == "http"
        && [Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&u.host_str());
    if (!local && u.scheme() != "https")
        || u.host_str().is_none()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(Error::Configuration);
    }
    Ok(u)
}
pub fn safe_return_to(raw: &str) -> Option<String> {
    if raw.len() > 2048
        || !raw.starts_with('/')
        || raw.starts_with("//")
        || raw.chars().any(|c| c == '\\' || c <= ' ')
    {
        return None;
    }
    let u = Url::parse("https://console.invalid").ok()?.join(raw).ok()?;
    let lower = u.path().to_ascii_lowercase();
    if u.host_str() != Some("console.invalid")
        || lower.contains("//")
        || ["%2f", "%5c", "%25"].iter().any(|v| lower.contains(v))
    {
        return None;
    }
    let decoded = percent_encoding::percent_decode_str(u.path())
        .decode_utf8()
        .ok()?;
    if decoded.chars().any(|c| c == '\\' || c <= ' ') {
        return None;
    }
    let allowed = [
        "/account",
        "/admin",
        "/cms",
        "/system",
        "/tenants",
        "/offers",
        "/people",
        "/sites",
        "/customers",
        "/projects",
        "/activity",
        "/integrations",
        "/infrastructure",
        "/hosting",
        "/operations",
        "/extensions",
        "/audit-events",
        "/workspaces",
        "/plans",
        "/databases",
        "/clusters",
        "/access-requests",
        "/mail",
    ];
    if raw != "/"
        && !allowed
            .iter()
            .any(|p| decoded == *p || decoded.starts_with(&format!("{p}/")))
    {
        return None;
    }
    if [
        "login",
        "logout",
        "forgot",
        "reset",
        "create-first-user",
        "unauthorized",
    ]
    .contains(&decoded.to_lowercase().split('/').nth(2).unwrap_or(""))
    {
        return None;
    }
    Some(u[url::Position::BeforePath..].to_owned())
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Flow {
    pub verifier: String,
    pub state: String,
    pub nonce: String,
    pub return_to: String,
    pub origin: String,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    #[serde(default)]
    pub role_ceiling: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Safe to persist inside a server-owned operation. Never accept this as browser proof.
/// Revalidation reloads the protected row and current client/session authority.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserAuthority {
    pub reference: String,
    pub client_id: String,
    pub app_origin: String,
    pub expires_at: i64,
    pub role: String,
    pub scopes: Vec<String>,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_scope: Option<String>,
}
pub struct BrowserAuthentication {
    pub principal: webdock_identity::Principal,
    pub authority: BrowserAuthority,
}

/// A server-owned reference stored with a launch/job, never a browser authentication DTO.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StoredAuthority {
    LocalSession {},
    CustomerSession {
        customer_scope: String,
        read_only: bool,
    },
    FirstPartyBrowser {
        browser: BrowserAuthority,
    },
}
impl StoredAuthority {
    pub fn local_session(principal: &webdock_identity::Principal) -> Self {
        match principal.customer_scope() {
            Some(scope) => Self::CustomerSession {
                customer_scope: scope.into(),
                read_only: principal.in_read_only_preview(),
            },
            None => Self::LocalSession {},
        }
    }
    pub fn read_only(&self) -> bool {
        matches!(self, Self::FirstPartyBrowser { browser } if browser.read_only)
            || matches!(
                self,
                Self::CustomerSession {
                    read_only: true,
                    ..
                }
            )
    }
}
/// Revalidates canonical session plus first-party client, token, role and scope ceilings.
/// The caller retains `auth_transaction` while committing its domain operation. The stored
/// subject/session and proof must come from server-owned rows, not unauthenticated input.
pub async fn revalidate_stored_authority(
    identity: &webdock_identity::IdentityService,
    auth_transaction: &sea_orm::DatabaseTransaction,
    browser_sessions: Option<&BrowserSessions>,
    subject: &str,
    session_id: &str,
    authority: &StoredAuthority,
) -> Result<webdock_identity::Principal> {
    let mut principal = identity
        .authenticate_session_in(auth_transaction, subject, session_id)
        .await?
        .ok_or(Error::Unauthorized)?;
    if !principal.can_access() {
        return Err(Error::Unauthorized);
    }
    if let StoredAuthority::CustomerSession { customer_scope, .. } = authority {
        if principal.customer_scope() != Some(customer_scope.as_str()) {
            return Err(Error::Unauthorized);
        }
    } else if matches!(authority, StoredAuthority::LocalSession {}) && principal.in_tenant_preview()
    {
        return Err(Error::Unauthorized);
    }
    if let StoredAuthority::FirstPartyBrowser { browser } = authority {
        let current = browser_sessions
            .ok_or(Error::Unauthorized)?
            .revalidate_in(auth_transaction, browser)
            .await?;
        if current.principal.user_id != subject || current.principal.session_id != session_id {
            return Err(Error::Unauthorized);
        }
        principal = current.principal;
    }
    Ok(principal)
}

/// Native trusted server-owned proof. Consume and retain the returned checked transaction.
/// First-party browser validation precedes canonical reads to keep client→identity lock order.
pub async fn revalidate_stored_authority_native_in<'a>(
    identity: &webdock_identity::IdentityService,
    auth_transaction: webdock_persistence::postgres::Transaction<'a>,
    browser_sessions: Option<&BrowserSessions>,
    subject: &str,
    session_id: &str,
    authority: &StoredAuthority,
) -> Result<(
    webdock_persistence::postgres::Transaction<'a>,
    webdock_identity::Principal,
)> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async move {
        let (tx, principal) = if let StoredAuthority::FirstPartyBrowser { browser } = authority {
            let (tx, current) = browser_sessions
                .ok_or(Error::Unauthorized)?
                .revalidate_native_in(auth_transaction, browser)
                .await?;
            (tx, current.principal)
        } else {
            let (tx, principal) = identity
                .authenticate_session_native_in(auth_transaction, subject, session_id)
                .await?;
            (tx, principal.ok_or(Error::Unauthorized)?)
        };
        if !principal.can_access()
            || principal.user_id != subject
            || principal.session_id != session_id
        {
            return Err(Error::Unauthorized);
        }
        if let StoredAuthority::CustomerSession { customer_scope, .. } = authority {
            if principal.customer_scope() != Some(customer_scope.as_str()) {
                return Err(Error::Unauthorized);
            }
        } else if matches!(authority, StoredAuthority::LocalSession {})
            && principal.in_tenant_preview()
        {
            return Err(Error::Unauthorized);
        }
        Ok((tx, principal))
    })
    .await
    .map_err(|_| Error::Configuration)?
}
