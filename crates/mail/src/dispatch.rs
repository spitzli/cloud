//! Compatibility entry points for the retained filesystem worker.
use crate::{
    payload::PrivatePayloadStore,
    send_control::{ControlError, SendControl},
};
use std::sync::Arc;
pub use webdock_mail_runtime::dispatch::{DispatchTransport, Dispatched};

pub async fn dispatch_one(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
    transport: &dyn DispatchTransport,
) -> std::result::Result<Option<Dispatched>, ControlError> {
    webdock_mail_runtime::dispatch::dispatch_one(control, store, transport).await
}
pub async fn cleanup_payloads(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
) -> std::result::Result<usize, ControlError> {
    webdock_mail_runtime::dispatch::cleanup_payloads(control, store).await
}
