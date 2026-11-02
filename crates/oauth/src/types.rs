use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;
pub(crate) const SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "webdock:read",
    "webdock:write",
    "hosting:read",
    "hosting:write",
];
#[derive(Clone)]
pub struct OAuthConfig {
    pub issuer: Url,
    pub mcp_resource: Url,
    pub studio_client_id: Option<String>,
}
impl OAuthConfig {
    pub fn new(issuer: &str, mcp_resource: &str, studio_client_id: Option<String>) -> Result<Self> {
        let issuer = valid_url(issuer)?;
        let mcp_resource = valid_url(mcp_resource)?;
        if issuer.path() != "/api/auth"
            || issuer.query().is_some()
            || mcp_resource.query().is_some()
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            issuer,
            mcp_resource,
            studio_client_id,
        })
    }
}
pub(crate) fn valid_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| Error::Configuration)?;
    let local = url.scheme() == "http"
        && [Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&url.host_str());
    if (!local && url.scheme() != "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Configuration);
    }
    Ok(url)
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientAuthMethod {
    None,
    ClientSecretPost,
    ClientSecretBasic,
}
impl ClientAuthMethod {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ClientSecretPost => "client_secret_post",
            Self::ClientSecretBasic => "client_secret_basic",
        }
    }
}
/// Contains a credential; deliberately not Debug.
#[derive(Clone)]
pub struct ClientAuthentication {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub method: ClientAuthMethod,
}
pub enum ClientPurpose {
    McpOperator,
    McpHosting,
    Portal,
    Organization { organization_id: String },
    OperatorOnly { allowed_subjects: Vec<String> },
}
pub struct ClientRegistration {
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub post_logout_redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    pub auth_method: ClientAuthMethod,
    pub allow_refresh: bool,
    pub skip_consent: bool,
    pub purpose: ClientPurpose,
}
pub struct RegisteredClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub binding_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub scope: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub max_age: Option<u64>,
}
impl AuthorizationRequest {
    pub fn code(
        client_id: String,
        redirect_uri: String,
        scope: String,
        code_challenge: String,
    ) -> Self {
        Self {
            client_id,
            redirect_uri,
            response_type: "code".into(),
            scope,
            state: None,
            nonce: None,
            code_challenge,
            code_challenge_method: "S256".into(),
            resources: vec![],
            prompt: None,
            max_age: None,
        }
    }
}
#[derive(Serialize)]
pub struct ConsentRequest {
    pub request_id: String,
    pub client_id: String,
    pub client_name: Option<String>,
    pub scopes: Vec<String>,
    pub resources: Vec<String>,
}
pub enum AuthorizationOutcome {
    LoginRequired,
    ConsentRequired(ConsentRequest),
    Redirect(String),
}
pub struct CodeExchange {
    pub client: ClientAuthentication,
    pub code: String,
    pub redirect_uri: String,
    pub code_verifier: String,
}
pub struct RefreshExchange {
    pub client: ClientAuthentication,
    pub refresh_token: String,
    pub scope: Option<String>,
}
/// Bearer credentials belong only in the protected token endpoint response, never logs.
#[derive(Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct GrantContext {
    pub subject: String,
    pub session_id: String,
    pub request: AuthorizationRequest,
    pub auth_time: i64,
}
pub(crate) struct Client {
    pub id: String,
    pub secret: Option<String>,
    pub name: Option<String>,
    pub redirects: Vec<String>,
    pub logout_redirects: Vec<String>,
    pub scopes: Vec<String>,
    pub grants: Vec<String>,
    pub method: ClientAuthMethod,
    pub skip_consent: bool,
    pub end_session: bool,
    pub metadata: Value,
}
/// A signed hint is required for automatic logout; mismatched/missing hints require own UI confirmation.
pub struct LogoutRequest {
    pub id_token_hint: String,
    pub post_logout_redirect_uri: Option<String>,
    pub state: Option<String>,
}
pub enum LogoutOutcome {
    ConfirmationRequired,
    Complete {
        redirect: Option<String>,
        set_cookie: String,
    },
}
