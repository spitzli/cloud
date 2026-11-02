//! Retained-schema OAuth/OIDC provider; request state machines are supplied by oxide-auth.
mod engine;
mod service;
mod signing;
pub use signing::stage_signing_key;
mod types;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
pub use service::{McpAuthentication, McpAuthority, McpGuard, OAuthService};
use sha2::{Digest, Sha256};
pub use types::*;

pub fn legacy_token_hash(raw: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(raw.as_bytes()))
}
fn random_token() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| Error::Crypto)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid OAuth configuration")]
    Configuration,
    #[error("invalid_request")]
    InvalidRequest,
    #[error("invalid_client")]
    InvalidClient,
    #[error("invalid_grant")]
    InvalidGrant,
    #[error("invalid_scope")]
    InvalidScope,
    #[error("access_denied")]
    AccessDenied,
    #[error("login_required")]
    LoginRequired,
    #[error("consent_required")]
    ConsentRequired,
    #[error("OAuth signing operation unavailable")]
    Crypto,
    #[error("OAuth persistence operation unavailable")]
    Database(#[from] sea_orm::DbErr),
    #[error("identity operation unavailable")]
    Identity(#[from] webdock_identity::IdentityError),
    #[error("ID allocation unavailable")]
    Allocation,
}
impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidClient => "invalid_client",
            Self::InvalidGrant => "invalid_grant",
            Self::InvalidScope => "invalid_scope",
            Self::AccessDenied => "access_denied",
            Self::LoginRequired => "login_required",
            Self::ConsentRequired => "consent_required",
            _ => "server_error",
        }
    }
    pub fn status(&self) -> u16 {
        match self {
            Self::InvalidClient => 401,
            Self::AccessDenied => 403,
            Self::Configuration
            | Self::Crypto
            | Self::Database(_)
            | Self::Identity(_)
            | Self::Allocation => 503,
            _ => 400,
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;
