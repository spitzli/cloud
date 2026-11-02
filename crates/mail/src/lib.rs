//! Native Mail domain and worker contracts. Provider implementations live outside Core.
pub mod account_delivery;
mod authority;
pub mod dispatch;
pub mod incoming;
mod incoming_send;
pub mod keys;
pub mod provider_transport;
pub mod queued_webmail;
pub mod sending;
pub use webmail::WebmailDependencies as MailDependencies;
pub mod budget;
pub mod gateway;
pub mod gateway_control;
pub mod jmap;
pub mod message;
pub mod outbox;
pub mod payload;
pub mod proof;
pub mod secrets;
pub mod send_api;
pub mod send_control;
pub mod smtp;
pub mod smtp_policy;
pub mod submissions;
pub mod webmail;
pub mod webmail_send_api;

pub use webdock_mail_runtime::{Error, Result};
pub mod managed_control;
mod recovery;

pub mod transport_adoption;
