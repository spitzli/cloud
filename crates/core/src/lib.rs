//! Provider-neutral platform contracts shared by Webdock services.
pub mod config;
pub mod features;
pub mod id;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ContractError(pub String);
