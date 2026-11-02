//! Authenticated, bounded plugin execution over private Unix sockets or explicit mTLS.
//! Authentication alone is insufficient: applications must implement live authority checks.
mod host;
mod pinned_tls;
mod storage;
mod transport;
use ed25519_dalek::VerifyingKey;
pub use host::{CorePeer, HostClient, RemoteAuthority};
pub use pinned_tls::{
    PinnedMutualTls, connect_pinned_mtls, pinned_mtls_authenticated, pinned_mtls_channel,
    pinned_mtls_channel_lazy,
};
use serde_json::Value;
use std::{
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use tokio_stream::Stream;
use tonic::transport::server::{TcpConnectInfo, TlsConnectInfo, UdsConnectInfo};
use tonic::{Request, Response, Status};
pub use transport::{
    MutualTls, UnixSocket, connect_mtls, connect_unix, unix_channel, unix_channel_lazy,
};
use webdock_plugin_protocol::{
    DOCUMENT_BYTES, ErrorCode, FRAME_BYTES, InvocationClaims, MANIFEST_BYTES, Manifest,
    PluginError, validate_value, verify_invocation,
};
// Compatibility exception for the preserved30k-character Mail action input. Select only
// from verified invocation claims; ordinary actions and every job retain FRAME_BYTES.
fn invocation_input_limit(claims: &InvocationClaims) -> usize {
    if claims.plugin_id == "mail" && claims.action == "mail.send" && claims.job.is_none() {
        128 * 1024
    } else {
        FRAME_BYTES
    }
}
#[allow(
    clippy::allow_attributes_without_reason,
    clippy::excessive_nesting,
    reason = "Tonic owns the nesting and lint attributes in this generated protocol module."
)]
pub mod wire {
    tonic::include_proto!("webdock.plugin.v1");
}
use wire::plugin_runtime_server::{PluginRuntime, PluginRuntimeServer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIdentity {
    Unix {
        uid: u32,
    },
    /// Verified TLS certificate chain, leaf first. Map its identity to an enrolled principal.
    MutualTls {
        certificates: Vec<Vec<u8>>,
    },
}

#[tonic::async_trait]
pub trait InvocationAuthority: Send + Sync + 'static {
    /// Check enrollment/revocation and permission to address this runtime, including health.
    async fn authorize_peer(&self, _peer: &PeerIdentity) -> Result<(), PluginError> {
        Err(PluginError::denied())
    }
    /// Static metadata probe only; it never permits execution or host capability calls.
    async fn authorize_probe(&self, peer: &PeerIdentity) -> Result<(), PluginError> {
        self.authorize_peer(peer).await
    }
    /// Match the actual payload to its persisted admission. Unimplemented authorities deny.
    async fn authorize_request(
        &self,
        _peer: &PeerIdentity,
        _claims: &InvocationClaims,
        _input: &[u8],
        _configuration: &[u8],
    ) -> Result<(), PluginError> {
        Err(PluginError::denied())
    }
    /// Revalidate invocation ID, session, installation/generation, grant, project and action.
    /// Called before execution, at intervals while pending, and before releasing results.
    async fn authorize_invocation(
        &self,
        _peer: &PeerIdentity,
        _claims: &InvocationClaims,
    ) -> Result<(), PluginError> {
        Err(PluginError::denied())
    }
}
#[derive(Default)]
pub struct DenyAll;
#[tonic::async_trait]
impl InvocationAuthority for DenyAll {}

#[tonic::async_trait]
pub trait ActionHandler: Send + Sync + 'static {
    /// Private, side-effect-free live resource check. Check current private state, ownership,
    /// pending lifecycle and resource_epoch; do not reuse a presign result as authority.
    async fn authorize_storage(
        &self,
        _authorization: &webdock_plugin_protocol::storage::StorageAuthorization,
    ) -> Result<(), PluginError> {
        Err(PluginError::denied())
    }

    /// This future is dropped on cancellation, deadline or revoked authority.
    /// Durable external effects still require operation IDs, receipts and fencing in the host.
    async fn invoke(
        &self,
        claims: &InvocationClaims,
        invocation_token: &str,
        input: Value,
        configuration: Value,
    ) -> Result<Value, PluginError>;
}

#[tonic::async_trait]
pub trait JobHandler: Send + Sync + 'static {
    /// Dropped on cancellation, deadline or revoked attempt authority. Effects are at least once;
    /// use the stable job ID for idempotency across fresh invocation IDs and attempts.
    async fn run(
        &self,
        claims: &InvocationClaims,
        invocation_token: &str,
        input: Value,
        configuration: Value,
    ) -> Result<Value, PluginError>;
}

#[derive(Clone)]
pub struct Runtime {
    manifest: Arc<Manifest>,
    generation: u64,
    key: VerifyingKey,
    authority: Arc<dyn InvocationAuthority>,
    handler: Arc<dyn ActionHandler>,
    jobs: Option<Arc<dyn JobHandler>>,
    permits: Arc<Semaphore>,
}
impl Runtime {
    pub fn new(
        manifest: Manifest,
        generation: u64,
        key: VerifyingKey,
        authority: Arc<dyn InvocationAuthority>,
        handler: Arc<dyn ActionHandler>,
    ) -> Result<Self, PluginError> {
        if generation == 0 || generation > 9_007_199_254_740_991 {
            return Err(PluginError::new(
                ErrorCode::InvalidInput,
                "Invalid runtime generation",
            ));
        }
        Ok(Self {
            manifest: Arc::new(manifest),
            generation,
            key,
            authority,
            handler,
            jobs: None,
            permits: Arc::new(Semaphore::new(32)),
        })
    }
    pub fn with_jobs(mut self, handler: Arc<dyn JobHandler>) -> Self {
        self.jobs = Some(handler);
        self
    }

    async fn run_job_inner(
        &self,
        request: Request<wire::RunJobRequest>,
    ) -> Result<Response<wire::RunJobResponse>, Status> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| Status::unavailable("Plugin job concurrency limit"))?;
        let peer = self.peer(&request).await?;
        let request = request.into_inner();
        let claims = self.check(&peer, &request.invocation_token).await?;
        if claims.job.is_none() || claims.action != request.name {
            return Err(status(PluginError::denied()));
        }
        let job = self
            .manifest
            .job(&request.name)
            .ok_or_else(|| Status::not_found("Unknown plugin job"))?;
        let handler = self
            .jobs
            .as_ref()
            .ok_or_else(|| Status::unimplemented("Plugin jobs unavailable"))?;
        if request.input_json.len() > FRAME_BYTES
            || request.configuration_json.len() > DOCUMENT_BYTES
        {
            return Err(Status::resource_exhausted("Plugin input limit"));
        }
        let input: Value = serde_json::from_slice(&request.input_json)
            .map_err(|_| Status::invalid_argument("Invalid JSON"))?;
        let configuration: Value = serde_json::from_slice(&request.configuration_json)
            .map_err(|_| Status::invalid_argument("Invalid JSON"))?;
        validate_value(job.get("input"), &input).map_err(status)?;
        validate_value(
            Some(&self.manifest.as_json()["configuration"]),
            &configuration,
        )
        .map_err(status)?;
        self.check_job_request(&peer, &claims, &request).await?;
        let execution = handler.run(&claims, &request.invocation_token, input, configuration);
        tokio::pin!(execution);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                result = &mut execution => break result.map_err(status)?,
                _ = interval.tick() => { self.check(&peer, &request.invocation_token).await?; },
            }
        };
        self.check(&peer, &request.invocation_token).await?;
        self.check_job_request(&peer, &claims, &request).await?;
        let output_json =
            serde_json::to_vec(&result).map_err(|_| Status::internal("Invalid plugin result"))?;
        if output_json.len() > FRAME_BYTES {
            return Err(Status::resource_exhausted("Plugin job output limit"));
        }
        Ok(Response::new(wire::RunJobResponse { output_json }))
    }

    async fn check_job_request(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
        request: &wire::RunJobRequest,
    ) -> Result<(), Status> {
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_request(
                peer,
                claims,
                &request.input_json,
                &request.configuration_json,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Authority deadline exceeded"))?
        .map_err(status)?;
        // Payload authorization can cross token expiry; do not await another lookup here.
        verify_invocation(
            &request.invocation_token,
            &self.key,
            self.manifest.id(),
            self.generation,
            now()?,
        )
        .map_err(status)?;
        Ok(())
    }
    /// Compose fixed-purpose HTTP routes on the same authenticated listener, retaining RPC limits.
    pub fn service(self) -> PluginRuntimeServer<Self> {
        let decoding =
            if self.manifest.id() == "mail" && self.manifest.action("mail.send").is_some() {
                224 * 1024
            } else {
                128 * 1024
            };
        PluginRuntimeServer::new(self)
            .max_decoding_message_size(decoding)
            .max_encoding_message_size(MANIFEST_BYTES + 1024)
    }
    async fn peer<T>(&self, request: &Request<T>) -> Result<PeerIdentity, Status> {
        let peer = authenticated_peer(request)?;
        self.check_peer(&peer).await?;
        Ok(peer)
    }
    async fn check_peer(&self, peer: &PeerIdentity) -> Result<(), Status> {
        tokio::time::timeout(Duration::from_secs(3), self.authority.authorize_peer(peer))
            .await
            .map_err(|_| Status::deadline_exceeded("Authority deadline exceeded"))?
            .map_err(status)
    }
    async fn check(&self, peer: &PeerIdentity, token: &str) -> Result<InvocationClaims, Status> {
        let claims = verify_invocation(
            token,
            &self.key,
            self.manifest.id(),
            self.generation,
            now()?,
        )
        .map_err(status)?;
        self.check_peer(peer).await?;
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_invocation(peer, &claims),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Authority deadline exceeded"))?
        .map_err(status)?;
        // An authority lookup may cross the expiry boundary.
        verify_invocation(
            token,
            &self.key,
            self.manifest.id(),
            self.generation,
            now()?,
        )
        .map_err(status)
    }
}
/// Read verified UDS credentials or the TLS-authenticated certificate chain.
pub fn authenticated_peer<T>(request: &Request<T>) -> Result<PeerIdentity, Status> {
    let peer = if let Some(info) = request.extensions().get::<UdsConnectInfo>() {
        let credentials = info
            .peer_cred
            .as_ref()
            .ok_or_else(|| status(PluginError::denied()))?;
        // Socket permissions are not the only check: authenticate the connected process too.
        if credentials.uid() != rustix::process::geteuid().as_raw() {
            return Err(status(PluginError::denied()));
        }
        PeerIdentity::Unix {
            uid: credentials.uid(),
        }
    } else if let Some(info) = request.extensions().get::<TlsConnectInfo<TcpConnectInfo>>() {
        let certs = info
            .peer_certs()
            .ok_or_else(|| status(PluginError::denied()))?;
        if certs.is_empty() {
            return Err(status(PluginError::denied()));
        }
        PeerIdentity::MutualTls {
            certificates: certs.iter().map(|c| c.as_ref().to_vec()).collect(),
        }
    } else {
        return Err(status(PluginError::denied()));
    };

    Ok(peer)
}

fn now() -> Result<i64, Status> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .ok_or_else(|| Status::unavailable("Clock unavailable"))
}
fn status(error: PluginError) -> Status {
    let code = match error.code {
        ErrorCode::InvalidInput => tonic::Code::InvalidArgument,
        ErrorCode::NotSupported | ErrorCode::CapabilityUnavailable => tonic::Code::Unimplemented,
        ErrorCode::PermissionDenied => tonic::Code::PermissionDenied,
        ErrorCode::Conflict => tonic::Code::Aborted,
        ErrorCode::Unavailable => tonic::Code::Unavailable,
        ErrorCode::DeadlineExceeded => tonic::Code::DeadlineExceeded,
        ErrorCode::NotFound => tonic::Code::NotFound,
    };
    // Handler diagnostics may contain provider secrets; only stable codes and customer-facing
    // messages (`public_message`) cross the boundary.
    if let Some(message) = webdock_plugin_protocol::public_message(error.code, &error.message) {
        return Status::new(code, message);
    }
    Status::new(
        code,
        match error.code {
            ErrorCode::InvalidInput => "Invalid plugin input",
            ErrorCode::NotSupported => "Plugin operation not supported",
            ErrorCode::CapabilityUnavailable => "Plugin capability unavailable",
            ErrorCode::PermissionDenied => "Plugin invocation denied",
            ErrorCode::Conflict => "Plugin operation conflict",
            ErrorCode::Unavailable => "Plugin runtime unavailable",
            ErrorCode::DeadlineExceeded => "Plugin deadline exceeded",
            ErrorCode::NotFound => "Plugin operation not found",
        },
    )
}
#[tonic::async_trait]
impl PluginRuntime for Runtime {
    async fn authorize_storage(
        &self,
        request: Request<wire::StorageAuthorizationRequest>,
    ) -> Result<Response<wire::StorageAuthorizationResponse>, Status> {
        storage::authorize(self, request).await
    }

    async fn describe(
        &self,
        request: Request<wire::DescribeRequest>,
    ) -> Result<Response<wire::DescribeResponse>, Status> {
        let peer = authenticated_peer(&request)?;
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_probe(&peer),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Probe authority deadline"))?
        .map_err(status)?;
        Ok(Response::new(wire::DescribeResponse {
            manifest_json: self.manifest.as_bytes().to_vec(),
        }))
    }
    async fn health(
        &self,
        request: Request<wire::HealthRequest>,
    ) -> Result<Response<wire::HealthResponse>, Status> {
        let peer = authenticated_peer(&request)?;
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_probe(&peer),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Probe authority deadline"))?
        .map_err(status)?;
        Ok(Response::new(wire::HealthResponse {
            plugin_id: self.manifest.id().into(),
            version: self.manifest.version().into(),
            generation: self.generation,
            protocol_version: 1,
        }))
    }
    async fn invoke(
        &self,
        request: Request<wire::InvokeRequest>,
    ) -> Result<Response<wire::InvokeResponse>, Status> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Plugin concurrency limit"))?;
        let peer = self.peer(&request).await?;
        let request = request.into_inner();
        let claims = self.check(&peer, &request.invocation_token).await?;
        if claims.job.is_some() || claims.action != request.action {
            return Err(status(PluginError::denied()));
        }
        let action = self
            .manifest
            .action(&claims.action)
            .ok_or_else(|| Status::not_found("Unknown plugin action"))?;
        if request.input_json.len() > invocation_input_limit(&claims)
            || request.configuration_json.len() > DOCUMENT_BYTES
        {
            return Err(Status::resource_exhausted("Plugin input limit"));
        }
        let input: Value = serde_json::from_slice(&request.input_json)
            .map_err(|_| Status::invalid_argument("Invalid JSON"))?;
        let configuration: Value = serde_json::from_slice(&request.configuration_json)
            .map_err(|_| Status::invalid_argument("Invalid JSON"))?;
        validate_value(action.get("input"), &input).map_err(status)?;
        validate_value(
            Some(&self.manifest.as_json()["configuration"]),
            &configuration,
        )
        .map_err(status)?;
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_request(
                &peer,
                &claims,
                &request.input_json,
                &request.configuration_json,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Authority deadline exceeded"))?
        .map_err(status)?;
        let execution = self.handler.invoke(
            &claims,
            &request.invocation_token,
            input.clone(),
            configuration.clone(),
        );
        tokio::pin!(execution);
        let deadline = tokio::time::sleep(Duration::from_secs(30));
        tokio::pin!(deadline);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                result=&mut execution => break result.map_err(status)?,
                _=&mut deadline => return Err(Status::deadline_exceeded("Plugin action deadline exceeded")),
                _=interval.tick() => { self.check(&peer,&request.invocation_token).await?; },
            }
        };
        self.check(&peer, &request.invocation_token).await?;
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize_request(
                &peer,
                &claims,
                &request.input_json,
                &request.configuration_json,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Authority deadline exceeded"))?
        .map_err(status)?;
        webdock_plugin_protocol::storage::validate_storage_result(action, &input, &result, now()?)
            .map_err(|_| Status::internal("Invalid plugin result"))?;
        let output_json =
            serde_json::to_vec(&result).map_err(|_| Status::internal("Invalid plugin result"))?;
        if output_json.len() > DOCUMENT_BYTES {
            return Err(Status::resource_exhausted("Plugin output limit"));
        }
        Ok(Response::new(wire::InvokeResponse { output_json }))
    }
    async fn run_job(
        &self,
        request: Request<wire::RunJobRequest>,
    ) -> Result<Response<wire::RunJobResponse>, Status> {
        // The outer deadline also bounds authority awaits and final result validation.
        tokio::time::timeout_at(
            tokio::time::Instant::now() + Duration::from_secs(45),
            self.run_job_inner(request),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Plugin job deadline exceeded"))?
    }
    type WatchOperationStream =
        Pin<Box<dyn Stream<Item = Result<wire::OperationEvent, Status>> + Send + 'static>>;
    async fn watch_operation(
        &self,
        request: Request<wire::WatchOperationRequest>,
    ) -> Result<Response<Self::WatchOperationStream>, Status> {
        let peer = self.peer(&request).await?;
        self.check(&peer, &request.into_inner().invocation_token)
            .await?;
        Err(Status::unimplemented(
            "Operation observation requires a durable host adapter",
        ))
    }
}
