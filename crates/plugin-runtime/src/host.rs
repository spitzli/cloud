//! Plugin-side authenticated callbacks; public action payloads never select a host endpoint.
use crate::*;
use sha2::{Digest, Sha256};
#[derive(Clone, serde::Serialize, serde::Deserialize)]
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
impl CorePeer {
    pub fn permits(&self, peer: &PeerIdentity) -> bool {
        match (self, peer) {
            (Self::Unix { uid: a }, PeerIdentity::Unix { uid: b }) => a == b,
            (Self::MutualTls { certificate_sha256 }, PeerIdentity::MutualTls { certificates }) => {
                certificates.first().is_some_and(|leaf| {
                    leaf.len() <= 65536
                        && hex::encode(Sha256::digest(leaf)) == *certificate_sha256
                        && x509_parser::parse_x509_certificate(leaf)
                            .is_ok_and(|(rest, cert)| rest.is_empty() && cert.validity().is_valid())
                })
            }
            _ => false,
        }
    }
}
#[derive(Clone)]
pub struct HostClient {
    client: wire::plugin_host_client::PluginHostClient<tonic::transport::Channel>,
    channel: tonic::transport::Channel,
}
impl HostClient {
    pub async fn connect_unix(
        path: impl AsRef<std::path::Path>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self::new(crate::transport::unix_channel(path).await?))
    }
    pub async fn connect_mtls(
        origin: String,
        domain: String,
        tls: MutualTls,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self::new(
            crate::transport::mtls_channel(origin, domain, tls).await?,
        ))
    }

    /// Channel must use the private socket or explicitly configured mTLS connector.
    pub fn new(channel: tonic::transport::Channel) -> Self {
        Self {
            channel: channel.clone(),
            client: wire::plugin_host_client::PluginHostClient::new(channel)
                .max_decoding_message_size(1048576 + 1024)
                .max_encoding_message_size(1048576 + 32768),
        }
    }
    /// Reuse the authenticated connector for fixed-purpose callbacks; no new credentials.
    pub fn channel(&self) -> tonic::transport::Channel {
        self.channel.clone()
    }
    /// Ordinary current-runtime authority over this client's authenticated callback transport.
    pub async fn authorize_runtime(
        &self,
        plugin: &str,
        generation: u64,
    ) -> Result<(), PluginError> {
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            self.client.clone().authorize_peer(wire::HostPeerRequest {
                plugin_id: plugin.into(),
                generation,
            }),
        )
        .await
        .map_err(|_| PluginError::new(ErrorCode::DeadlineExceeded, "Host deadline exceeded"))?
        .map_err(map_status)?
        .into_inner();
        if !result.active {
            return Err(PluginError::denied());
        }
        Ok(())
    }
    pub async fn call(
        &self,
        token: &str,
        service: &str,
        method: &str,
        input: Value,
    ) -> Result<Value, PluginError> {
        let limit = match (service, method) {
            ("jobs", "enqueue") => 32768 + 4096,
            ("jobs", "get" | "cancel") => 256,
            ("data", "read") | ("apps", "request") => 32768,
            ("data", "write") => 131072,
            ("http", "request") => 1048576,
            _ => {
                return Err(PluginError::new(
                    ErrorCode::CapabilityUnavailable,
                    "Host capability unavailable",
                ));
            }
        };
        let bytes = serde_json::to_vec(&input).map_err(|_| PluginError::denied())?;
        if bytes.len() > limit || token.len() > 16384 {
            return Err(PluginError::new(
                ErrorCode::InvalidInput,
                "Host request limit",
            ));
        }
        let response = tokio::time::timeout(
            Duration::from_secs(20),
            self.client.clone().call(wire::HostCallRequest {
                invocation_token: token.into(),
                service: service.into(),
                method: method.into(),
                input_json: bytes,
            }),
        )
        .await
        .map_err(|_| PluginError::new(ErrorCode::DeadlineExceeded, "Host deadline exceeded"))?
        .map_err(map_status)?
        .into_inner();
        if response.output_json.len() > 1048576 {
            return Err(PluginError::new(
                ErrorCode::Unavailable,
                "Host result limit",
            ));
        }
        serde_json::from_slice(&response.output_json)
            .map_err(|_| PluginError::new(ErrorCode::Unavailable, "Invalid host result"))
    }
    /// `plugin.workloads` (`apply`, `observe`, `remove`) as this plugin runtime; the host checks
    /// the connection's peer against the runtime binding for `plugin_id` and `generation`.
    pub async fn workloads(
        &self,
        plugin_id: &str,
        generation: u64,
        method: &str,
        input: Value,
    ) -> Result<Value, PluginError> {
        let bytes = serde_json::to_vec(&input).map_err(|_| PluginError::denied())?;
        if bytes.len() > 262144 + 4096 {
            return Err(PluginError::new(
                ErrorCode::InvalidInput,
                "Host request limit",
            ));
        }
        let response = tokio::time::timeout(
            Duration::from_secs(20),
            self.client.clone().workloads(wire::HostWorkloadRequest {
                plugin_id: plugin_id.into(),
                generation,
                method: method.into(),
                input_json: bytes,
            }),
        )
        .await
        .map_err(|_| PluginError::new(ErrorCode::DeadlineExceeded, "Host deadline exceeded"))?
        .map_err(map_status)?
        .into_inner();
        serde_json::from_slice(&response.output_json)
            .map_err(|_| PluginError::new(ErrorCode::Unavailable, "Invalid host result"))
    }
}
fn map_status(status: Status) -> PluginError {
    let code = match status.code() {
        tonic::Code::PermissionDenied | tonic::Code::Unauthenticated => ErrorCode::PermissionDenied,
        tonic::Code::InvalidArgument | tonic::Code::ResourceExhausted => ErrorCode::InvalidInput,
        tonic::Code::NotFound => ErrorCode::NotFound,
        tonic::Code::Aborted => ErrorCode::Conflict,
        tonic::Code::Unimplemented => ErrorCode::CapabilityUnavailable,
        tonic::Code::DeadlineExceeded => ErrorCode::DeadlineExceeded,
        _ => ErrorCode::Unavailable,
    };
    let message = webdock_plugin_protocol::public_message(code, status.message())
        .unwrap_or_else(|| "Host request could not be completed".into());
    PluginError::new(code, message)
}
pub struct RemoteAuthority {
    host: HostClient,
    plugin_id: String,
    generation: u64,
    peers: Vec<CorePeer>,
}
impl RemoteAuthority {
    pub fn new(
        host: HostClient,
        plugin_id: String,
        generation: u64,
        peers: Vec<CorePeer>,
    ) -> Result<Self, PluginError> {
        if plugin_id.is_empty()||plugin_id.len()>64||!plugin_id.as_bytes()[0].is_ascii_lowercase()||!plugin_id.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'-')||generation==0||generation>9_007_199_254_740_991||peers.is_empty()||peers.len()>8||peers.iter().any(|peer|matches!(peer,CorePeer::MutualTls{certificate_sha256} if certificate_sha256.len()!=64||!certificate_sha256.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)))){return Err(PluginError::denied());}
        Ok(Self {
            host,
            plugin_id,
            generation,
            peers,
        })
    }
    fn peer(&self, peer: &PeerIdentity) -> Result<(), PluginError> {
        if !self.peers.iter().any(|pin| pin.permits(peer)) {
            return Err(PluginError::denied());
        }
        Ok(())
    }
    fn scope(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
    ) -> Result<Vec<u8>, PluginError> {
        self.peer(peer)?;
        if claims.plugin_id != self.plugin_id || claims.generation != self.generation {
            return Err(PluginError::denied());
        }
        serde_json::to_vec(claims).map_err(|_| PluginError::denied())
    }
}
#[tonic::async_trait]
impl InvocationAuthority for RemoteAuthority {
    async fn authorize_probe(&self, peer: &PeerIdentity) -> Result<(), PluginError> {
        self.peer(peer)
    }
    async fn authorize_peer(&self, peer: &PeerIdentity) -> Result<(), PluginError> {
        self.peer(peer)?;
        self.host
            .authorize_runtime(&self.plugin_id, self.generation)
            .await
    }
    async fn authorize_invocation(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
    ) -> Result<(), PluginError> {
        let result = self
            .host
            .client
            .clone()
            .authorize_invocation(wire::HostInvocationRequest {
                claims_json: self.scope(peer, claims)?,
            })
            .await
            .map_err(map_status)?
            .into_inner();
        if !result.active {
            return Err(PluginError::denied());
        }
        Ok(())
    }
    async fn authorize_request(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
        input: &[u8],
        configuration: &[u8],
    ) -> Result<(), PluginError> {
        let claims_json = self.scope(peer, claims)?;
        let input_json = input.to_vec();
        let configuration_json = configuration.to_vec();
        if input_json.len() > crate::invocation_input_limit(claims)
            || configuration_json.len() > DOCUMENT_BYTES
        {
            return Err(PluginError::denied());
        }
        let result = self
            .host
            .client
            .clone()
            .authorize_request(wire::HostPayloadRequest {
                claims_json,
                input_json,
                configuration_json,
            })
            .await
            .map_err(map_status)?
            .into_inner();
        if !result.active {
            return Err(PluginError::denied());
        }
        Ok(())
    }
}
