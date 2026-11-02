//! Core-independent native Mail custody, ingress and one-attempt delivery.
pub mod dispatch;
pub mod gateway;
pub mod message;
pub mod payload;
pub mod proof;
pub mod provider_transport;
pub mod secrets;
pub mod send_api;
pub mod send_control;
pub mod smtp;
pub mod outbox {
    pub use webdock_mail_protocol::outbox::*;
}
pub mod sending {
    pub use webdock_mail_protocol::outbox::Preparation;
}
pub mod submissions {
    pub use webdock_mail_protocol::outbox::CleanupItem;
}
pub use payload::{PrivatePayloadStore as FilesystemPayloadStore, SavedPayload};
pub use send_control::{ControlError, SendControl};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Mail message or submission")]
    Invalid,
    #[error("Mail credentials cannot be opened; check key version and instance binding")]
    Secret,
    #[error("private Mail storage is unavailable")]
    Storage,
    #[error("Mail submission is unavailable")]
    Unavailable,
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<webdock_mail_message::Error> for Error {
    fn from(_: webdock_mail_message::Error) -> Self {
        Self::Invalid
    }
}

#[async_trait::async_trait]
pub trait PrivatePayloadStore: Send + Sync {
    async fn put(&self, customer: &str, reference: &str, bytes: &[u8]) -> Result<SavedPayload>;
    async fn get(&self, customer: &str, reference: &str) -> Result<Vec<u8>>;
    async fn remove(&self, customer: &str, reference: &str) -> Result<()>;
}
