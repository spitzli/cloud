//! Generated, provider-neutral agent management client/server contract.
//! Contains no Docker, Kubernetes, CLI, credential storage or execution implementation.
#[allow(
    clippy::allow_attributes_without_reason,
    clippy::excessive_nesting,
    reason = "Tonic owns the nesting and lint attributes in this generated protocol module."
)]
mod wire {
    tonic::include_proto!("webdock.agent.v1");
}
pub use wire::*;

mod identity;
pub use identity::{
    CertificateIdentity, TransportIdentity, certificate_identity, transport_identity,
};

/// Signed, attempt-scoped build storage authority, independent of cluster clients.
pub mod build_storage;
/// Independent database execution contracts; no persistence or provider implementations.
pub mod database;
pub mod mail;
pub mod mail_configuration;
pub mod recovery;

/// Bounded application/database frames, including public CA material.
pub const MAX_MESSAGE_BYTES: usize = 512 * 1024;

/// Guarded installation-owned runtime deployment, independent of customer workloads.
pub mod plugin;
