//! Authenticated bounded repository runtime. Production authority queries live Core admission.
use crate::*;
use async_trait::async_trait;
use serde_json::Value;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use tonic::transport::server::{TcpConnectInfo, TlsConnectInfo, UdsConnectInfo};
use tonic::{Request, Response, Status};
pub use webdock_plugin_runtime::{MutualTls, PeerIdentity, UnixSocket};
use wire::repository_runtime_server::{RepositoryRuntime, RepositoryRuntimeServer};
#[async_trait]
pub trait Authority: Send + Sync + 'static {
    async fn authorize_probe(&self, peer: &PeerIdentity) -> Result<()> {
        self.authorize_peer(peer).await
    }
    async fn authorize_peer(&self, peer: &PeerIdentity) -> Result<()>;
    async fn authorize_lease(
        &self,
        peer: &PeerIdentity,
        claims: &LeaseClaims,
        token: &str,
    ) -> Result<()>;
}
#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn resolve(
        &self,
        claims: &LeaseClaims,
        input: Value,
        configuration: Value,
    ) -> Result<Value>;
    async fn archive(
        &self,
        claims: &LeaseClaims,
        input: Value,
        configuration: Value,
    ) -> Result<Vec<u8>>;
    async fn check(
        &self,
        _claims: &LeaseClaims,
        _input: Value,
        _configuration: Value,
    ) -> Result<Value> {
        Err(PluginError::new(
            ErrorCode::CapabilityUnavailable,
            "Repository checks unavailable",
        ))
    }
}
#[derive(Clone)]
pub struct Runtime {
    descriptor: Descriptor,
    descriptor_token: String,
    generation: u64,
    issuer: VerifyingKey,
    authority: Arc<dyn Authority>,
    handler: Arc<dyn Handler>,
    permits: Arc<Semaphore>,
    archives: Arc<Semaphore>,
}
fn denied() -> Status {
    Status::permission_denied("Repository lease denied")
}
fn unavailable() -> Status {
    Status::unavailable("Repository operation unavailable")
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn peer<T>(request: &Request<T>) -> std::result::Result<PeerIdentity, Status> {
    if let Some(info) = request.extensions().get::<UdsConnectInfo>() {
        let credential = info.peer_cred.as_ref().ok_or_else(denied)?;
        if credential.uid() != rustix::process::geteuid().as_raw() {
            return Err(denied());
        }
        Ok(PeerIdentity::Unix {
            uid: credential.uid(),
        })
    } else if let Some(info) = request.extensions().get::<TlsConnectInfo<TcpConnectInfo>>() {
        let certificates = info
            .peer_certs()
            .filter(|c| !c.is_empty())
            .ok_or_else(denied)?;
        Ok(PeerIdentity::MutualTls {
            certificates: certificates.iter().map(|c| c.as_ref().to_vec()).collect(),
        })
    } else {
        Err(denied())
    }
}
impl Runtime {
    pub fn new(
        descriptor_token: String,
        publisher: &VerifyingKey,
        generation: u64,
        issuer: VerifyingKey,
        authority: Arc<dyn Authority>,
        handler: Arc<dyn Handler>,
    ) -> Result<Self> {
        let descriptor = verify_descriptor(&descriptor_token, publisher)?;
        if generation == 0 || generation > 9_007_199_254_740_991 {
            return Err(invalid("Invalid runtime generation"));
        }
        Ok(Self {
            descriptor,
            descriptor_token,
            generation,
            issuer,
            authority,
            handler,
            permits: Arc::new(Semaphore::new(32)),
            archives: Arc::new(Semaphore::new(2)),
        })
    }
    pub fn service(self) -> RepositoryRuntimeServer<Self> {
        RepositoryRuntimeServer::new(self)
            .max_decoding_message_size(128 * 1024)
            .max_encoding_message_size(JSON_BYTES + 1024)
    }
    pub async fn serve_unix(
        self,
        mut socket: UnixSocket,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::result::Result<(), tonic::transport::Error> {
        let incoming = socket.incoming().expect("owned listener");
        let result = tonic::transport::Server::builder()
            .concurrency_limit_per_connection(32)
            .add_service(self.service())
            .serve_with_incoming_shutdown(incoming, shutdown)
            .await;
        drop(socket);
        result
    }
    async fn guard(
        &self,
        peer: &PeerIdentity,
        token: &str,
    ) -> std::result::Result<LeaseClaims, Status> {
        let claims = verify_lease(
            token,
            &self.issuer,
            &self.descriptor.plugin_id,
            self.generation,
            now(),
        )
        .map_err(|_| denied())?;
        if claims.descriptor_sha256 != sha256(self.descriptor_token.as_bytes()) {
            return Err(denied());
        }
        tokio::time::timeout(Duration::from_secs(3), self.authority.authorize_peer(peer))
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| denied())?;
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_lease(peer, &claims, token),
        )
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| denied())?;
        Ok(claims)
    }
    async fn input(
        &self,
        request: &Request<wire::LeaseRequest>,
        action: Action,
    ) -> std::result::Result<(PeerIdentity, LeaseClaims, Value, Value), Status> {
        let peer = peer(request)?;
        let input = request.get_ref();
        if input.input_json.len() > FRAME_BYTES || input.configuration_json.len() > JSON_BYTES {
            return Err(Status::resource_exhausted("Repository frame limit"));
        }
        let claims = self.guard(&peer, &input.lease_token).await?;
        if claims.action != action
            || claims.request_sha256 != sha256(&input.input_json)
            || claims.configuration_sha256 != sha256(&input.configuration_json)
        {
            return Err(denied());
        }
        let data = serde_json::from_slice(&input.input_json)
            .map_err(|_| Status::invalid_argument("Invalid repository input"))?;
        let config = serde_json::from_slice(&input.configuration_json)
            .map_err(|_| Status::invalid_argument("Invalid repository configuration"))?;
        Ok((peer, claims, data, config))
    }
    async fn bounded<T>(
        &self,
        peer: &PeerIdentity,
        token: &str,
        work: impl Future<Output = Result<T>>,
    ) -> std::result::Result<T, Status> {
        tokio::pin!(work);
        let deadline = tokio::time::sleep(Duration::from_secs(30));
        tokio::pin!(deadline);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {result=&mut work=>{let result=result.map_err(|e|match e.code{ErrorCode::PermissionDenied=>denied(),ErrorCode::InvalidInput=>Status::invalid_argument("Invalid repository request"),ErrorCode::CapabilityUnavailable=>Status::unimplemented("Repository capability unavailable"),_=>unavailable()})?;self.guard(peer,token).await?;return Ok(result);},_=&mut deadline=>return Err(Status::deadline_exceeded("Repository deadline exceeded")),_=interval.tick()=>{self.guard(peer,token).await?;}}
        }
    }
    async fn json(
        &self,
        request: Request<wire::LeaseRequest>,
        action: Action,
    ) -> std::result::Result<Response<wire::JsonReply>, Status> {
        let (peer, claims, input, config) = self.input(&request, action).await?;
        let _permit = tokio::time::timeout(Duration::from_secs(1), self.permits.acquire())
            .await
            .map_err(|_| Status::resource_exhausted("Repository runtime busy"))?
            .map_err(|_| unavailable())?;
        let value = match action {
            Action::Resolve => {
                self.bounded(
                    &peer,
                    &request.get_ref().lease_token,
                    self.handler.resolve(&claims, input, config),
                )
                .await?
            }
            Action::Check => {
                self.bounded(
                    &peer,
                    &request.get_ref().lease_token,
                    self.handler.check(&claims, input, config),
                )
                .await?
            }
            Action::Archive => return Err(denied()),
        };
        let bytes = serde_json::to_vec(&value).map_err(|_| unavailable())?;
        if bytes.len() > JSON_BYTES {
            return Err(Status::resource_exhausted("Repository result limit"));
        }
        Ok(Response::new(wire::JsonReply { output_json: bytes }))
    }
}
#[tonic::async_trait]
impl RepositoryRuntime for Runtime {
    async fn describe(
        &self,
        request: Request<wire::DescribeRequest>,
    ) -> std::result::Result<Response<wire::DescriptorReply>, Status> {
        let peer = peer(&request)?;
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_probe(&peer),
        )
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| denied())?;
        Ok(Response::new(wire::DescriptorReply {
            signed_descriptor_json: serde_json::to_vec(&self.descriptor_token)
                .map_err(|_| unavailable())?,
        }))
    }
    async fn resolve(
        &self,
        request: Request<wire::LeaseRequest>,
    ) -> std::result::Result<Response<wire::JsonReply>, Status> {
        self.json(request, Action::Resolve).await
    }
    async fn check(
        &self,
        request: Request<wire::LeaseRequest>,
    ) -> std::result::Result<Response<wire::JsonReply>, Status> {
        self.json(request, Action::Check).await
    }
    type ArchiveStream = Pin<
        Box<
            dyn tokio_stream::Stream<Item = std::result::Result<wire::ArchiveFrame, Status>> + Send,
        >,
    >;
    async fn archive(
        &self,
        request: Request<wire::LeaseRequest>,
    ) -> std::result::Result<Response<Self::ArchiveStream>, Status> {
        let (peer, claims, input, config) = self.input(&request, Action::Archive).await?;
        let permit = tokio::time::timeout(
            Duration::from_secs(1),
            self.archives.clone().acquire_owned(),
        )
        .await
        .map_err(|_| Status::resource_exhausted("Repository archive capacity"))?
        .map_err(|_| unavailable())?;
        let bytes = self
            .bounded(
                &peer,
                &request.get_ref().lease_token,
                self.handler.archive(&claims, input, config),
            )
            .await?;
        if bytes.is_empty() || bytes.len() as u64 > SOURCE_BYTES {
            return Err(Status::resource_exhausted("Source archive limit"));
        }
        let metadata = wire::ArchiveMetadata {
            size_bytes: bytes.len() as u64,
            sha256: sha256(&bytes),
            format: "tar".into(),
        };
        let runtime = self.clone();
        let token = request.into_inner().lease_token;
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        tokio::spawn(async move {
            let _permit = permit;
            let result: std::result::Result<(), Status> = async {
                runtime.guard(&peer, &token).await?;
                tx.send(Ok(wire::ArchiveFrame {
                    offset: 0,
                    data: vec![],
                    metadata: Some(metadata),
                }))
                .await
                .map_err(|_| Status::cancelled("Archive receiver closed"))?;
                let mut checked = Instant::now();
                let mut offset = 0u64;
                for chunk in bytes.chunks(FRAME_BYTES) {
                    if checked.elapsed() >= Duration::from_millis(250) {
                        runtime.guard(&peer, &token).await?;
                        checked = Instant::now();
                    }
                    if now() >= claims.exp {
                        return Err(denied());
                    }
                    let frame = wire::ArchiveFrame {
                        offset,
                        data: chunk.to_vec(),
                        metadata: None,
                    };
                    let send = tx.send(Ok(frame));
                    tokio::pin!(send);
                    let mut tick = tokio::time::interval(Duration::from_millis(250));
                    loop {
                        tokio::select! {
                            result = &mut send => {
                                result.map_err(|_| Status::cancelled("Archive receiver closed"))?;
                                break;
                            },
                            _ = tick.tick() => { runtime.guard(&peer, &token).await?; }
                        }
                    }
                    offset += chunk.len() as u64;
                }
                runtime.guard(&peer, &token).await?;
                Ok(())
            }
            .await;
            if let Err(e) = result {
                let _ = tokio::time::timeout(Duration::from_secs(1), tx.send(Err(e))).await;
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}
/// Validates a private socket before obtaining a generic channel; remote clients must configure mTLS.
pub async fn connect_unix(
    path: &std::path::Path,
) -> std::result::Result<tonic::transport::Channel, Box<dyn std::error::Error + Send + Sync>> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let parent = path.parent().ok_or("private directory required")?;
    let directory = parent.symlink_metadata()?;
    let socket = path.symlink_metadata()?;
    if !path.is_absolute()
        || parent.canonicalize()? != parent
        || !directory.is_dir()
        || directory.mode() & 0o077 != 0
        || directory.uid() != rustix::process::geteuid().as_raw()
        || !socket.file_type().is_socket()
        || socket.mode() & 0o077 != 0
        || socket.uid() != directory.uid()
    {
        return Err("untrusted repository socket".into());
    }
    Ok(tonic::transport::Endpoint::from_shared(format!(
        "unix://{}",
        path.to_str().ok_or("invalid socket path")?
    ))?
    .connect_timeout(Duration::from_secs(3))
    .timeout(Duration::from_secs(60))
    .connect()
    .await?)
}
/// Consume the bounded binary stream only after complete digest/length verification.
pub async fn collect_archive(mut stream: tonic::Streaming<wire::ArchiveFrame>) -> Result<Vec<u8>> {
    let header = stream
        .message()
        .await
        .map_err(|_| PluginError::denied())?
        .ok_or_else(PluginError::denied)?;
    let metadata = header
        .metadata
        .filter(|m| {
            m.size_bytes > 0
                && m.size_bytes <= SOURCE_BYTES
                && m.format == "tar"
                && valid_digest(&m.sha256)
        })
        .ok_or_else(PluginError::denied)?;
    if header.offset != 0 || !header.data.is_empty() {
        return Err(PluginError::denied());
    }
    let mut bytes = Vec::new();
    while let Some(frame) = stream.message().await.map_err(|_| PluginError::denied())? {
        if frame.metadata.is_some()
            || frame.offset != bytes.len() as u64
            || frame.data.is_empty()
            || frame.data.len() > FRAME_BYTES
            || bytes.len() as u64 + frame.data.len() as u64 > metadata.size_bytes
        {
            return Err(PluginError::denied());
        }
        bytes.extend_from_slice(&frame.data);
    }
    if bytes.len() as u64 != metadata.size_bytes || sha256(&bytes) != metadata.sha256 {
        return Err(PluginError::denied());
    }
    Ok(bytes)
}
impl Runtime {
    pub async fn serve_mtls(
        self,
        address: std::net::SocketAddr,
        tls: MutualTls,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = tokio::net::TcpListener::bind(address).await?;
        tonic::transport::Server::builder()
            .tls_config(
                tonic::transport::ServerTlsConfig::new()
                    .identity(tls.identity)
                    .client_ca_root(tls.peer_ca),
            )?
            .concurrency_limit_per_connection(32)
            .add_service(self.service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                shutdown,
            )
            .await?;
        Ok(())
    }
}
pub async fn connect_mtls(
    origin: String,
    domain: String,
    tls: MutualTls,
) -> std::result::Result<tonic::transport::Channel, Box<dyn std::error::Error + Send + Sync>> {
    let uri: tonic::codegen::http::Uri = origin.parse()?;
    if uri.scheme_str() != Some("https")
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
        || uri.path_and_query().is_some_and(|p| p.as_str() != "/")
    {
        return Err("HTTPS mTLS repository origin required".into());
    }
    Ok(tonic::transport::Endpoint::from_shared(origin)?
        .tls_config(
            tonic::transport::ClientTlsConfig::new()
                .identity(tls.identity)
                .ca_certificate(tls.peer_ca)
                .domain_name(domain),
        )?
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(60))
        .connect()
        .await?)
}

/// Core's transport identity is operator-pinned; every admission is also checked live by Core.
/// The callback channel must be created with `connect_unix` or `connect_mtls`.
pub struct RemoteAuthority {
    client: wire::repository_authority_client::RepositoryAuthorityClient<tonic::transport::Channel>,
    plugin_id: String,
    generation: u64,
    descriptor_sha256: String,
    core_peers: Vec<CorePeer>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CorePeer {
    Unix { uid: u32 },
    MutualTls { certificate_sha256: String },
}
impl RemoteAuthority {
    pub fn new(
        channel: tonic::transport::Channel,
        plugin_id: String,
        generation: u64,
        descriptor_sha256: String,
        core_peers: Vec<CorePeer>,
    ) -> Result<Self> {
        if !slug(&plugin_id)||generation==0||!valid_digest(&descriptor_sha256)||core_peers.is_empty()||core_peers.len()>8||core_peers.iter().any(|peer|matches!(peer,CorePeer::MutualTls{certificate_sha256} if !valid_digest(certificate_sha256))){return Err(invalid("Invalid repository authority configuration"));}
        Ok(Self {
            client: wire::repository_authority_client::RepositoryAuthorityClient::new(channel)
                .max_decoding_message_size(1024)
                .max_encoding_message_size(32 * 1024),
            plugin_id,
            generation,
            descriptor_sha256,
            core_peers,
        })
    }
    fn permits(&self, peer: &PeerIdentity) -> bool {
        self.core_peers
            .iter()
            .any(|expected| match (expected, peer) {
                (CorePeer::Unix { uid: a }, PeerIdentity::Unix { uid: b }) => a == b,
                (
                    CorePeer::MutualTls { certificate_sha256 },
                    PeerIdentity::MutualTls { certificates },
                ) => certificates
                    .first()
                    .is_some_and(|certificate| sha256(certificate) == *certificate_sha256),
                _ => false,
            })
    }
}
#[async_trait]
impl Authority for RemoteAuthority {
    async fn authorize_probe(&self, peer: &PeerIdentity) -> Result<()> {
        if self.permits(peer) {
            Ok(())
        } else {
            Err(PluginError::denied())
        }
    }
    async fn authorize_peer(&self, peer: &PeerIdentity) -> Result<()> {
        if !self.permits(peer) {
            return Err(PluginError::denied());
        }
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            self.client.clone().authorize_peer(wire::PeerProof {
                plugin_id: self.plugin_id.clone(),
                generation: self.generation,
                descriptor_sha256: self.descriptor_sha256.clone(),
            }),
        )
        .await
        .map_err(|_| PluginError::denied())?
        .map_err(|_| PluginError::denied())?
        .into_inner();
        if !result.active {
            return Err(PluginError::denied());
        }
        Ok(())
    }
    async fn authorize_lease(
        &self,
        peer: &PeerIdentity,
        claims: &LeaseClaims,
        token: &str,
    ) -> Result<()> {
        if !self.permits(peer)
            || claims.plugin_id != self.plugin_id
            || claims.generation != self.generation
            || claims.descriptor_sha256 != self.descriptor_sha256
            || token.len() > 16384
        {
            return Err(PluginError::denied());
        }
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            self.client.clone().authorize(wire::LeaseProof {
                lease_token: token.into(),
            }),
        )
        .await
        .map_err(|_| PluginError::denied())?
        .map_err(|_| PluginError::denied())?
        .into_inner();
        if !result.active {
            return Err(PluginError::denied());
        }
        Ok(())
    }
}
