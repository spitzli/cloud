//! Privileged helpers constructed only by the separately supervised install worker.
//! No platform database handle or request-serving authority is accepted here.
pub mod storage;
pub mod tls;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningError {
    Invalid,
    Trust,
    Ownership,
    Unavailable,
}
impl std::fmt::Display for ProvisioningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid plugin provisioning configuration",
            Self::Trust => "plugin provisioning trust rejected",
            Self::Ownership => "plugin storage ownership mismatch",
            Self::Unavailable => "plugin provisioning unavailable",
        })
    }
}
impl std::error::Error for ProvisioningError {}
pub type Result<T> = std::result::Result<T, ProvisioningError>;
