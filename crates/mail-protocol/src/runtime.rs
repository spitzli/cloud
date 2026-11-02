//! Bounded authenticated streaming. Only Core's durable attempt callback can release provider
//! configuration, and it is called once after the entire MIME artifact has been verified.
use crate::*;
use serde_json::Value;
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use tonic::{
    Request, Response, Status,
    transport::{Server, ServerTlsConfig},
};
pub use webdock_plugin_runtime::{MutualTls, PeerIdentity, UnixSocket};
use wire::mail_runtime_server::{MailRuntime, MailRuntimeServer};

#[async_trait::async_trait]
pub trait Authority: Send + Sync + 'static {
    /// Static metadata may be inspected by the explicitly pinned Core peer before activation.
    /// This must never grant Submit or release provider configuration.
    async fn authorize_probe(&self, _peer: &PeerIdentity) -> Result<()> {
        Err(Denied)
    }
    /// Check the actual authenticated worker peer and live persisted job/source/runtime binding.
    async fn guard(&self, peer: &PeerIdentity, claims: &LeaseClaims, token: &str) -> Result<()>;
    /// Atomic one-use attempt. Lost acknowledgement must deny a retry, including after restart.
    async fn begin_attempt(
        &self,
        peer: &PeerIdentity,
        claims: &LeaseClaims,
        token: &str,
    ) -> Result<Value>;
}
#[async_trait::async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn submit(
        &self,
        claims: &LeaseClaims,
        metadata: Value,
        mime: Vec<u8>,
        private_configuration: Value,
    ) -> wire::SendReceipt;
}
#[derive(Clone)]
pub struct Runtime {
    plugin: String,
    version: String,
    generation: u64,
    descriptor: String,
    issuer: VerifyingKey,
    authority: Arc<dyn Authority>,
    handler: Arc<dyn Handler>,
    slots: Arc<Semaphore>,
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn denied() -> Status {
    Status::permission_denied("Mail provider authority denied")
}
fn unknown() -> wire::SendReceipt {
    wire::SendReceipt {
        state: wire::send_receipt::State::Unknown as i32,
        provider_id: String::new(),
    }
}
impl Runtime {
    pub fn new(
        plugin: String,
        version: String,
        generation: u64,
        descriptor: String,
        issuer: VerifyingKey,
        authority: Arc<dyn Authority>,
        handler: Arc<dyn Handler>,
    ) -> Result<Self> {
        if !slug(&plugin)
            || version.len() > 64
            || semver::Version::parse(&version).is_err()
            || generation == 0
            || generation > i64::MAX as u64
            || !valid_digest(&descriptor)
        {
            return Err(Denied);
        }
        Ok(Self {
            plugin,
            version,
            generation,
            descriptor,
            issuer,
            authority,
            handler,
            slots: Arc::new(Semaphore::new(2)),
        })
    }
    pub fn service(self) -> MailRuntimeServer<Self> {
        MailRuntimeServer::new(self)
            .max_decoding_message_size(64 * 1024)
            .max_encoding_message_size(1024)
    }
    pub async fn serve_unix(
        self,
        mut socket: UnixSocket,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::result::Result<(), tonic::transport::Error> {
        let incoming = socket.incoming().expect("owned Mail socket");
        let result = Server::builder()
            .concurrency_limit_per_connection(4)
            .add_service(self.service())
            .serve_with_incoming_shutdown(incoming, shutdown)
            .await;
        drop(socket);
        result
    }
    pub async fn serve_mtls(
        self,
        listener: tokio::net::TcpListener,
        tls: MutualTls,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::result::Result<(), tonic::transport::Error> {
        Server::builder()
            .tls_config(
                ServerTlsConfig::new()
                    .identity(tls.identity)
                    .client_ca_root(tls.peer_ca)
                    .client_auth_optional(false)
                    .timeout(Duration::from_secs(5)),
            )?
            .concurrency_limit_per_connection(4)
            .add_service(self.service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                shutdown,
            )
            .await
    }
    async fn guard(
        &self,
        peer: &PeerIdentity,
        token: &str,
    ) -> std::result::Result<LeaseClaims, Status> {
        let claims = verify_lease(token, &self.issuer, &self.plugin, self.generation, now())
            .map_err(|_| denied())?;
        if claims.descriptor_sha256 != self.descriptor {
            return Err(denied());
        }
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.guard(peer, &claims, token),
        )
        .await
        .map_err(|_| denied())?
        .map_err(|_| denied())?;
        Ok(claims)
    }
    async fn receive(
        &self,
        peer: &PeerIdentity,
        stream: &mut tonic::Streaming<wire::SendFrame>,
    ) -> std::result::Result<(LeaseClaims, String, Value, Vec<u8>), Status> {
        let first = tokio::time::timeout(Duration::from_secs(5), stream.message())
            .await
            .map_err(|_| denied())??
            .ok_or_else(denied)?;
        if first.offset != 0 || !first.data.is_empty() {
            return Err(denied());
        }
        let header = first.header.ok_or_else(denied)?;
        if header.metadata_json.len() > MAX_METADATA_BYTES {
            return Err(denied());
        }
        let claims = self.guard(peer, &header.lease_token).await?;
        if sha256(&header.metadata_json) != claims.metadata_sha256 {
            return Err(denied());
        }
        let metadata: Value =
            serde_json::from_slice(&header.metadata_json).map_err(|_| denied())?;
        if !metadata.is_object() {
            return Err(denied());
        }
        let mut data = Vec::with_capacity(claims.mime_bytes as usize);
        let mut checked_at = 0;
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.tick().await;
        loop {
            let frame = tokio::select! {
                result=stream.message()=>result?,
                _=interval.tick()=>{self.guard(peer,&header.lease_token).await?;continue;}
            };
            let Some(frame) = frame else {
                break;
            };
            if now() >= claims.exp
                || frame.header.is_some()
                || frame.offset != data.len() as u64
                || frame.data.is_empty()
                || frame.data.len() > FRAME_BYTES
                || data.len() + frame.data.len() > claims.mime_bytes as usize
            {
                return Err(denied());
            }
            data.extend_from_slice(&frame.data);
            if data.len() - checked_at >= 1024 * 1024 {
                self.guard(peer, &header.lease_token).await?;
                checked_at = data.len();
            }
        }
        if data.len() as u64 != claims.mime_bytes || sha256(&data) != claims.mime_sha256 {
            return Err(denied());
        }
        self.guard(peer, &header.lease_token).await?;
        Ok((claims, header.lease_token, metadata, data))
    }
}
#[tonic::async_trait]
impl MailRuntime for Runtime {
    async fn describe(
        &self,
        request: Request<wire::DescribeRequest>,
    ) -> std::result::Result<Response<wire::DescriptorReply>, Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_probe(&peer),
        )
        .await
        .map_err(|_| denied())?
        .map_err(|_| denied())?;
        Ok(Response::new(wire::DescriptorReply {
            plugin_id: self.plugin.clone(),
            version: self.version.clone(),
            generation: self.generation,
            protocol: "webdock.mail.v1".into(),
            protocol_version: 1,
            descriptor_sha256: self.descriptor.clone(),
        }))
    }
    async fn submit(
        &self,
        request: Request<tonic::Streaming<wire::SendFrame>>,
    ) -> std::result::Result<Response<wire::SendReceipt>, Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Mail provider busy"))?;
        let mut stream = request.into_inner();
        let (claims, token, metadata, data) =
            tokio::time::timeout(Duration::from_secs(60), self.receive(&peer, &mut stream))
                .await
                .map_err(|_| Status::deadline_exceeded("Mail transfer deadline"))??;
        let private = tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.begin_attempt(&peer, &claims, &token),
        )
        .await
        .map_err(|_| denied())?
        .map_err(|_| denied())?;
        if !private.is_object() || serde_json::to_vec(&private).map_err(|_| denied())?.len() > 65536
        {
            return Ok(Response::new(unknown()));
        }
        // Cancellation or timeout after this boundary is uncertain. Core's persisted attempt
        // denies any subsequent invocation; never retry an external provider request here.
        let receipt = tokio::time::timeout(
            Duration::from_secs(90),
            self.handler.submit(&claims, metadata, data, private),
        )
        .await
        .unwrap_or_else(|_| unknown());
        let valid = match wire::send_receipt::State::try_from(receipt.state) {
            Ok(wire::send_receipt::State::Accepted) => {
                !receipt.provider_id.is_empty()
                    && receipt.provider_id.len() <= 255
                    && receipt
                        .provider_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            }
            Ok(wire::send_receipt::State::Rejected | wire::send_receipt::State::Unknown) => {
                receipt.provider_id.is_empty()
            }
            Err(_) => false,
        };
        Ok(Response::new(if valid { receipt } else { unknown() }))
    }
}

/// Plugin-side callback client. The configured worker leaf/UID and authenticated Core channel
/// are independent of anything in an incoming request or message metadata.
pub struct RemoteAuthority {
    worker: webdock_plugin_runtime::CorePeer,
    probe: webdock_plugin_runtime::CorePeer,
    client: wire::mail_authority_client::MailAuthorityClient<tonic::transport::Channel>,
}
impl RemoteAuthority {
    pub fn new(
        worker: webdock_plugin_runtime::CorePeer,
        probe: webdock_plugin_runtime::CorePeer,
        channel: tonic::transport::Channel,
    ) -> Self {
        Self {
            worker,
            probe,
            client: wire::mail_authority_client::MailAuthorityClient::new(channel)
                .max_encoding_message_size(17000)
                .max_decoding_message_size(65536 + 1024),
        }
    }
}
#[async_trait::async_trait]
impl Authority for RemoteAuthority {
    async fn authorize_probe(&self, peer: &PeerIdentity) -> Result<()> {
        if self.probe.permits(peer) {
            Ok(())
        } else {
            Err(Denied)
        }
    }
    async fn guard(&self, peer: &PeerIdentity, _claims: &LeaseClaims, token: &str) -> Result<()> {
        if !self.worker.permits(peer) || token.len() > 16384 {
            return Err(Denied);
        }
        let reply = self
            .client
            .clone()
            .guard(wire::LeaseProof {
                lease_token: token.into(),
            })
            .await
            .map_err(|_| Denied)?
            .into_inner();
        if !reply.active {
            return Err(Denied);
        }
        Ok(())
    }
    async fn begin_attempt(
        &self,
        peer: &PeerIdentity,
        _claims: &LeaseClaims,
        token: &str,
    ) -> Result<Value> {
        if !self.worker.permits(peer) || token.len() > 16384 {
            return Err(Denied);
        }
        let reply = self
            .client
            .clone()
            .begin_attempt(wire::LeaseProof {
                lease_token: token.into(),
            })
            .await
            .map_err(|_| Denied)?
            .into_inner();
        if !reply.admitted || reply.private_configuration_json.len() > 65536 {
            return Err(Denied);
        }
        let configuration: Value =
            serde_json::from_slice(&reply.private_configuration_json).map_err(|_| Denied)?;
        if !configuration.is_object() {
            return Err(Denied);
        }
        Ok(configuration)
    }
}
