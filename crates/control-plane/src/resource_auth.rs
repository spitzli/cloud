//! Shared bearer-only resource boundary for retained Studio protocols.
use crate::http::{ApiFailure, ServiceState, reject};
use axum::{
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use webdock_oauth::{ClientAuthMethod, ClientAuthentication, McpAuthentication};

/// A bounded HTTP failure preserves protocol headers without bloating every result.
pub struct ResourceFailure(Box<Response>);
impl ResourceFailure {
    pub(crate) fn new(response: Response) -> Self {
        Self(Box::new(response))
    }
    pub fn status(&self) -> StatusCode {
        self.0.status()
    }
}
impl IntoResponse for ResourceFailure {
    fn into_response(self) -> Response {
        *self.0
    }
}
impl From<ApiFailure> for ResourceFailure {
    fn from(error: ApiFailure) -> Self {
        Self::new(error.into_response())
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceClientConfig {
    pub client_id: String,
    pub secret: PathBuf,
}
impl ResourceClientConfig {
    pub fn from_environment() -> Result<Option<Self>, &'static str> {
        let id = std::env::var("WEBDOCK_RESOURCE_CLIENT_ID");
        let secret = std::env::var("WEBDOCK_RESOURCE_CLIENT_SECRET_FILE");
        match (id, secret) {
            (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => Ok(None),
            (Ok(client_id), Ok(secret)) if !client_id.is_empty() && !secret.is_empty() => {
                let secret = PathBuf::from(secret);
                crate::config::read_private_file(&secret)
                    .map_err(|_| "Resource client secret is unavailable.")?;
                Ok(Some(Self { client_id, secret }))
            }
            _ => Err("Configure both resource client ID and private secret file."),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Registry,
    Hosting,
}
impl Resource {
    pub fn scope(self, write: bool) -> &'static str {
        match (self, write) {
            (Self::Registry, false) => "webdock:read",
            (Self::Registry, true) => "webdock:write",
            (Self::Hosting, false) => "hosting:read",
            (Self::Hosting, true) => "hosting:write",
        }
    }
}
pub fn protocol_path(path: &str) -> bool {
    path == "/api/registry"
        || path.starts_with("/api/registry/")
        || path == "/api/hosting"
        || path.starts_with("/api/hosting/")
}
fn unavailable() -> ApiFailure {
    reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Resource authorization is unavailable.",
    )
}
pub fn challenge(state: &ServiceState, scope: &str, status: StatusCode) -> ResourceFailure {
    let mut response = reject(
        status,
        if status == StatusCode::FORBIDDEN {
            "insufficient_scope"
        } else {
            "unauthorized"
        },
        "Bearer authorization with current resource scope is required.",
    )
    .into_response();
    let suffix = if status == StatusCode::FORBIDDEN {
        ", error=\"insufficient_scope\""
    } else {
        ""
    };
    let value = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/api/mcp\", scope=\"{scope}\"{suffix}",
        state.config.studio_origin,
    );
    if let Ok(value) = HeaderValue::from_str(&value) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    ResourceFailure::new(response)
}
pub async fn authenticate(
    state: &ServiceState,
    headers: &HeaderMap,
    resource: Resource,
    write: bool,
) -> Result<Arc<McpAuthentication>, ResourceFailure> {
    if headers
        .get(header::ORIGIN)
        .is_some_and(|v| v != state.config.studio_origin.as_str())
    {
        return Err(reject(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "Invalid resource origin.",
        )
        .into());
    }
    let scope = resource.scope(write);
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty() && v.len() <= 16384 && !v.chars().any(char::is_whitespace))
        .ok_or_else(|| challenge(state, scope, StatusCode::UNAUTHORIZED))?;
    let config = state
        .resource_client
        .as_ref()
        .or_else(|| state.plugins.as_ref().and_then(|p| p.mcp_client.as_ref()));
    let config = config.ok_or_else(|| ResourceFailure::from(unavailable()))?;
    let bytes = crate::config::read_private_file(&config.secret)
        .map_err(|_| ResourceFailure::from(unavailable()))?;
    let secret = std::str::from_utf8(&bytes)
        .map_err(|_| ResourceFailure::from(unavailable()))?
        .trim()
        .to_owned();
    let oauth = state
        .oauth
        .as_ref()
        .ok_or_else(|| ResourceFailure::from(unavailable()))?;
    let credentials = ClientAuthentication {
        client_id: config.client_id.clone(),
        client_secret: Some(secret),
        method: ClientAuthMethod::ClientSecretPost,
    };
    // Check read authority first to distinguish an authenticated read-only caller
    // from an invalid/revoked bearer without weakening the write scope.
    let actor = oauth
        .authorize_mcp_reference(&credentials, token, resource.scope(false))
        .await
        .map_err(|error| {
            if error.status() == 503 {
                ResourceFailure::from(unavailable())
            } else {
                challenge(state, scope, StatusCode::UNAUTHORIZED)
            }
        })?;
    if write && !actor.authority.can_write() {
        return Err(challenge(
            state,
            &format!("{} {scope}", resource.scope(false)),
            StatusCode::FORBIDDEN,
        ));
    }
    Ok(Arc::new(actor))
}
