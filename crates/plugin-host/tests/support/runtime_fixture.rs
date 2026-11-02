use tonic::{Request, Response, Status};
use webdock_plugin_runtime::wire;
/// The fixture callback grants no candidate invocation or workload authority.
#[derive(Clone)]
pub struct DenyHost;
#[tonic::async_trait]
impl wire::plugin_host_server::PluginHost for DenyHost {
    async fn authorize_peer(
        &self,
        _: Request<wire::HostPeerRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Err(Status::permission_denied("candidate"))
    }
    async fn authorize_invocation(
        &self,
        _: Request<wire::HostInvocationRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Err(Status::permission_denied("candidate"))
    }
    async fn authorize_request(
        &self,
        _: Request<wire::HostPayloadRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Err(Status::permission_denied("candidate"))
    }
    async fn call(
        &self,
        _: Request<wire::HostCallRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::permission_denied("candidate"))
    }
    async fn workloads(
        &self,
        _: Request<wire::HostWorkloadRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::permission_denied("candidate"))
    }
}
pub struct Child(pub std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub fn private(path: &std::path::Path, bytes: impl AsRef<[u8]>) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes.as_ref()).unwrap();
}
