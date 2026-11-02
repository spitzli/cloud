use crate::model::*;
/// No arbitrary endpoints, policy documents or object bytes cross the product boundary.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn backend(&self) -> &str;
    fn endpoint(&self) -> &str;
    async fn head_object(
        &self,
        _binding: &Binding,
        _key: &str,
    ) -> ProviderResult<crate::data::ObjectMetadata> {
        Err(ProviderError::Unavailable)
    }
    fn validate_lifecycle(&self, _policy: &Lifecycle) -> ProviderResult<()> {
        Ok(())
    }
    async fn presign(
        &self,
        _binding: &Binding,
        _direction: webdock_plugin_protocol::storage::StorageDirection,
        _request: &crate::data::DataRequest,
    ) -> ProviderResult<crate::data::PresignedObject> {
        Err(ProviderError::Unavailable)
    }
    async fn provision(&self, binding: &Binding) -> ProviderResult<()>;
    async fn observe(&self, binding: &Binding) -> ProviderResult<Observation>;
    async fn keys(&self, binding: &Binding) -> ProviderResult<Vec<String>>;
    async fn create_key(&self, binding: &Binding) -> ProviderResult<SecretKey>;
    async fn revoke_key(&self, binding: &Binding, key: &str) -> ProviderResult<()>;
    async fn set_versioning(&self, binding: &Binding, enabled: bool) -> ProviderResult<()>;
    async fn set_lifecycle(
        &self,
        binding: &Binding,
        policy: Option<&Lifecycle>,
    ) -> ProviderResult<()>;
    async fn inventory(
        &self,
        binding: &Binding,
        cursor: Option<&Cursor>,
    ) -> ProviderResult<InventoryPage>;
    async fn purge(&self, binding: &Binding) -> ProviderResult<bool>;
    /// Must reject nonempty and verify absence, including the exact managed identity.
    async fn delete_empty(&self, binding: &Binding) -> ProviderResult<()>;
}
