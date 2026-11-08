//! Webdock-owned identity against the retained PostgreSQL identity schema.
//! Account recovery and passkey ceremonies use maintained cryptographic libraries.
//! OAuth/OIDC and external identity cutover remain separate unfinished ports.
mod crypto;
mod neon;
mod neon_client;
pub use neon_client::NeonAuthClient;
mod passkeys;
mod recovery;
mod scope;
mod security;
mod service;
pub use crypto::{CookiePolicy, SecretKeys, hash_password, verify_password};
pub use passkeys::{LegacyPasskey, PasskeyChallenge, PasskeySummary, import_legacy_passkey};
pub use recovery::{
    Delivery, DeliveryBatch, DeliveryError, DeliveryKind, DeliveryRequest, UnavailableDelivery,
    validate_account_origin,
};
pub use scope::RlsSigner;
pub use security::{MfaProof, SessionSummary, TotpEnrollment};
pub use service::{IdentityService, IssuedSession, PasswordAuthentication, Principal};
pub use webauthn_rs::prelude::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("invalid credential encoding")]
    InvalidEncoding,
    #[error("invalid identity configuration")]
    Configuration,
    #[error("cryptographic operation failed")]
    Crypto,
    #[error("authentication required")]
    Unauthorized,
    #[error("invalid or already used verification code")]
    InvalidCode,
    #[error("security setup is not permitted in this state")]
    SecurityState,
    #[error("too many sign-in attempts")]
    RateLimited,
    #[error("invalid account input")]
    InvalidInput,
    #[error("identity database operation failed")]
    Database(#[from] sea_orm::DbErr),
    #[error("native identity persistence is unavailable")]
    Persistence,
    #[error("identity allocation is unavailable")]
    Allocation,
    #[error("credential worker failed")]
    Worker,
}
pub type Result<T> = std::result::Result<T, IdentityError>;
