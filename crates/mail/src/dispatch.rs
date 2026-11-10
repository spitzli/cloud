//! Compatibility entry points for the retained filesystem worker.
use crate::{
    payload::PrivatePayloadStore,
    send_control::{ControlError, SendControl},
};
pub use spitzli_mail_runtime::dispatch::{DispatchTransport, Dispatched};
use std::sync::Arc;

pub async fn dispatch_one(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
    transport: &dyn DispatchTransport,
) -> std::result::Result<Option<Dispatched>, ControlError> {
    spitzli_mail_runtime::dispatch::dispatch_one(control, store, transport).await
}
pub async fn cleanup_payloads(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
) -> std::result::Result<usize, ControlError> {
    spitzli_mail_runtime::dispatch::cleanup_payloads(control, store).await
}
