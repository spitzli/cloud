//! Privileged executors. Keep this crate and its credentials out of the public API process.
pub mod config;
#[cfg(feature = "docker")]
pub mod docker;
#[cfg(feature = "kubernetes")]
pub mod kubernetes;
pub mod mail;
pub mod management;
pub mod policy;

#[cfg(feature = "kubernetes")]
pub mod legacy;

pub mod outbox;

pub mod storage;

mod logs;

#[cfg(feature = "docker")]
pub mod recovery;
pub mod registry;

#[cfg(feature = "kubernetes")]
pub mod database;

#[cfg(feature = "kubernetes")]
pub mod workloads;

#[cfg(feature = "kubernetes")]
pub mod plugin_runtime;
