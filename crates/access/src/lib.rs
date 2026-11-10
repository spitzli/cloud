//! Retained tenant, invitation and account administration using live identity and signed RLS.
mod admin;
mod allowances;
pub use allowances::{AllowanceAuthority, ConfigureAllowances, FiniteAllowance, HostingAllowances};
pub mod api_tokens;
mod branding;
mod contracts;
pub mod domains;
mod migration;
mod projects;
mod service;
mod waitlist;
pub use contracts::*;
pub use migration::*;
pub use service::AccessService;
pub use waitlist::{WaitlistAction, WaitlistDecision, WaitlistSubmission};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Sign in and complete account setup first.")]
    Unauthorized,
    #[error("Access is not permitted.")]
    Forbidden,
    #[error("Record not found or access unavailable.")]
    NotFound,
    #[error("Input is invalid.")]
    InvalidInput,
    #[error("The record changed or has already been answered.")]
    Conflict,
    #[error("Keep at least one active verified customer owner or administrator.")]
    LastAdministrator,
    #[error("This organization or account requires reviewed local-authority activation.")]
    ActivationRequired,
    #[error("The plan allows no more projects.")]
    ProjectLimit,
    #[error("Another customer already verified this domain.")]
    DomainTaken,
    #[error("Something still uses this domain.")]
    DomainInUse,
    #[error("Access persistence is unavailable.")]
    Database(#[from] sea_orm::DbErr),
    #[error("Identity operation is unavailable.")]
    Identity(#[from] spitzli_identity::IdentityError),
    #[error("ID allocation is unavailable.")]
    Allocation,
    #[error("Too many requests. Try again later.")]
    RateLimited,
    #[error("Access configuration is invalid.")]
    Configuration,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn status(&self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::Conflict
            | Self::LastAdministrator
            | Self::ActivationRequired
            | Self::ProjectLimit
            | Self::DomainTaken
            | Self::DomainInUse => 409,
            Self::InvalidInput => 400,
            Self::RateLimited => 429,
            _ => 503,
        }
    }
}
