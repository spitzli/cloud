//! A plugin's `plugin.workloads` calls to Core (`apply`, `observe`, `remove`) behind a trait, so
//! plugin tests can stand in for Core and the cluster.
use serde_json::Value;
use webdock_plugin_protocol::PluginError;

#[tonic::async_trait]
pub trait Workloads: Send + Sync + 'static {
    async fn call(&self, method: &str, input: Value) -> Result<Value, PluginError>;
}
/// The plugin runtime's own calls, authenticated as its runtime binding.
pub struct HostWorkloads {
    pub client: webdock_plugin_runtime::HostClient,
    pub plugin: String,
    pub generation: u64,
}
#[tonic::async_trait]
impl Workloads for HostWorkloads {
    async fn call(&self, method: &str, input: Value) -> Result<Value, PluginError> {
        self.client
            .workloads(&self.plugin, self.generation, method, input)
            .await
    }
}
