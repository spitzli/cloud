pub mod data;
pub mod engine;
#[cfg(feature = "providers")]
pub mod minio;
pub mod model;
pub mod provider;
#[cfg(feature = "providers")]
pub mod sdk;
pub mod store;

use serde_json::Value;
use std::sync::Arc;
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::ActionHandler;
pub struct Handler(pub Arc<engine::Engine>);
#[tonic::async_trait]
impl ActionHandler for Handler {
    async fn authorize_storage(
        &self,
        authorization: &webdock_plugin_protocol::storage::StorageAuthorization,
    ) -> Result<(), PluginError> {
        self.0
            .authorize_data(authorization)
            .await
            .map_err(|_| PluginError::denied())
    }

    async fn invoke(
        &self,
        claims: &InvocationClaims,
        _token: &str,
        input: Value,
        configuration: Value,
    ) -> Result<Value, PluginError> {
        if claims.plugin_id != "object-storage"
            || claims.job.is_some()
            || configuration != serde_json::json!({})
        {
            return Err(PluginError::denied());
        }
        self.0
            .execute(
                &claims.customer_id,
                &claims.project_id,
                &claims.action,
                input,
            )
            .await
            .map_err(|message| PluginError::new(ErrorCode::Conflict, message))
    }
}
pub fn validate_manifest(manifest: &Manifest) -> Result<(), PluginError> {
    let expected: Value =
        serde_json::from_slice(include_bytes!("../manifest.json")).expect("reviewed manifest");
    let mut actual = manifest.as_json().clone();
    let mut expected = expected;
    actual
        .as_object_mut()
        .expect("validated manifest")
        .remove("image");
    expected
        .as_object_mut()
        .expect("reviewed manifest")
        .remove("image");
    if actual != expected {
        return Err(PluginError::new(
            ErrorCode::InvalidInput,
            "Object storage manifest differs from compiled contract",
        ));
    }
    Ok(())
}
