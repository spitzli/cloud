//! The transport fixture uses a new enrollment only; installation never needs a customer/project.
use spitzli_agent_control::PluginRuntimeBackend;
use spitzli_agent_protocol::plugin::*;
use spitzli_hosting::AgentIdentity;
use std::sync::Arc;
use tonic::Status;
struct EmptyBackend;
#[tonic::async_trait]
impl PluginRuntimeBackend for EmptyBackend {
    async fn poll(
        &self,
        _: &AgentIdentity,
        _: u32,
    ) -> std::result::Result<Option<PluginRuntimeClaim>, Status> {
        Ok(None)
    }
    async fn guard(
        &self,
        _: &AgentIdentity,
        _: &PluginRuntimeSpec,
        _: &PluginRuntimeLease,
    ) -> std::result::Result<bool, Status> {
        Ok(false)
    }
    async fn checkpoint(
        &self,
        _: &AgentIdentity,
        _: &PluginRuntimeSpec,
        _: &PluginRuntimeLease,
        _: &PluginRuntimeCheckpoint,
    ) -> std::result::Result<bool, Status> {
        Ok(false)
    }
    async fn complete(
        &self,
        _: &AgentIdentity,
        _: &PluginRuntimeSpec,
        _: &PluginRuntimeLease,
        _: &PluginRuntimeCompletion,
    ) -> std::result::Result<bool, Status> {
        Ok(false)
    }
}
#[test]
fn backend_contract_has_no_plugin_host_dependency() {
    let _: Arc<dyn PluginRuntimeBackend> = Arc::new(EmptyBackend);
}
