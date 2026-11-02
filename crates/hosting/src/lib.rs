pub mod contracts;
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct HostingError {
    pub status: u16,
    pub message: String,
}
pub type Result<T> = std::result::Result<T, HostingError>;
pub(crate) fn error(status: u16, message: &str) -> HostingError {
    HostingError {
        status,
        message: message.into(),
    }
}
mod entities;
mod environment;
mod read_view;
mod recovery_signing;
mod service;
pub use read_view::{LogOutput, ReadOutput, ReadStatus, ReadView};
pub use recovery_signing::RecoverySigner;
pub use service::{AgentIdentity, HostingCeiling, HostingService, canonical_command};

pub use service::{DeliveryCommand, DeliveryDisposition};
