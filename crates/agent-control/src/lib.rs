//! Core-owned outbound-agent control. No Docker socket or Kubernetes credentials are used here.
mod completion;
mod database;
mod mail;
mod mail_configuration;
mod plugin;
mod recovery;
mod workload;
pub use plugin::PluginRuntimeBackend;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tonic::{Request, Response, Status};
use webdock_agent_protocol as wire;
use webdock_hosting::{AgentIdentity, HostingService};
use webdock_supervisor::{
    Action, Command, ObservedState, ReadCommand, ReadKind, ResourceMetrics, RuntimeSpec,
};
use wire::agent_control_server::{AgentControl, AgentControlServer};

/// A private Unix listener is bound to one enrollment, never selected by request metadata.
#[derive(Clone)]
pub struct LocalBinding {
    pub installation: String,
    pub agent_id: String,
    pub uid: u32,
}
#[derive(Clone)]
pub struct ControlPlane {
    db: DatabaseConnection,
    hosting: Arc<HostingService>,
    hosting_enabled: bool,
    plugin_runtime: Option<Arc<dyn PluginRuntimeBackend>>,
    local: Option<LocalBinding>,
    databases: Option<Arc<webdock_databases::DatabaseService>>,
    mail: Option<Arc<webdock_mail::incoming::agent::IncomingAgent>>,
}
fn now() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}
fn unavailable(_: impl std::fmt::Display) -> Status {
    Status::unavailable("Agent control persistence unavailable")
}
fn denied() -> Status {
    Status::permission_denied("Current agent enrollment required")
}
fn invalid() -> Status {
    Status::invalid_argument("Invalid agent request")
}
fn hosting_error(error: webdock_hosting::HostingError) -> Status {
    match error.status {
        401 => denied(),
        // A revoked user/grant cancels this operation, not the enrolled agent itself.
        403 => Status::failed_precondition("Operation authority changed"),
        400 => invalid(),
        404 => Status::not_found("Resource unavailable"),
        409 => Status::failed_precondition("Operation authority changed"),
        429 => Status::resource_exhausted("Agent rate limit exceeded"),
        _ => Status::unavailable("Runtime operation unavailable"),
    }
}
fn validate_identity(identity: &wire::AgentIdentity) -> Result<(), Status> {
    fn slug(value: &str, max: usize) -> bool {
        !value.is_empty()
            && value.len() <= max
            && value.as_bytes()[0].is_ascii_lowercase()
            && value
                .bytes()
                .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'-')
    }
    if !slug(&identity.installation, 24)
        || !slug(&identity.agent_id, 64)
        || identity.generation == 0
        || identity.generation > i32::MAX as u64
    {
        return Err(invalid());
    }
    Ok(())
}
fn verified_peer<T>(
    request: &Request<T>,
    local: Option<&LocalBinding>,
) -> Result<wire::TransportIdentity, Status> {
    let peer = wire::transport_identity(request, now())?;
    if let wire::TransportIdentity::Unix { uid, .. } = &peer {
        if local.is_none_or(|binding| binding.uid != *uid) {
            return Err(denied());
        }
    } else if local.is_some() {
        return Err(denied());
    }
    Ok(peer)
}
// Untagged preserves the exact stored legacy mutation shape. Strict DTO fields
// reject ambiguous action+kind objects; reads never masquerade as mutations.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum StoredCommand {
    Mutation(Command),
    Read(ReadCommand),
}
impl StoredCommand {
    fn spec(&self) -> &RuntimeSpec {
        match self {
            Self::Mutation(c) => &c.spec,
            Self::Read(c) => &c.spec,
        }
    }
    fn operation_id(&self) -> &str {
        match self {
            Self::Mutation(c) => &c.operation_id,
            Self::Read(c) => &c.operation_id,
        }
    }
    fn generation(&self) -> u64 {
        match self {
            Self::Mutation(c) => c.generation,
            Self::Read(c) => c.generation,
        }
    }
    fn lease_until(&self) -> u64 {
        match self {
            Self::Mutation(c) => c.lease_until,
            Self::Read(c) => c.lease_until,
        }
    }
    fn authorization(&self) -> &str {
        match self {
            Self::Mutation(c) => &c.authorization,
            Self::Read(c) => &c.authorization,
        }
    }
    fn fingerprint(&self) -> Result<String, Status> {
        match self {
            Self::Mutation(c) => c.fingerprint(),
            Self::Read(c) => c.fingerprint(),
        }
        .map_err(|_| invalid())
    }
    fn validate(&self) -> Result<(), Status> {
        match self {
            Self::Mutation(c) => c.validate(),
            Self::Read(c) => c.validate(),
        }
        .map_err(|_| invalid())
    }
    fn intent(&self) -> wire::Intent {
        match self {
            Self::Mutation(c) => match c.action {
                Action::Apply => wire::Intent::Apply,
                Action::Stop => wire::Intent::Stop,
                Action::Retire => wire::Intent::Retire,
                Action::PurgeStorage => wire::Intent::PurgeStorage,
            },
            Self::Read(c) => match c.kind {
                ReadKind::Inspect => wire::Intent::Inspect,
                ReadKind::Logs => wire::Intent::Logs,
                ReadKind::Metrics => wire::Intent::Metrics,
            },
        }
    }
    fn wire(&self) -> Result<wire::Command, Status> {
        Ok(wire::Command {
            material_json: Vec::new(),
            operation_id: self.operation_id().into(),
            generation: self.generation(),
            lease_until: self.lease_until(),
            authorization: self.authorization().into(),
            intent: self.intent() as i32,
            spec_json: serde_json::to_vec(self.spec()).map_err(|_| invalid())?,
            log_tail: match self {
                Self::Mutation(_) => 0,
                Self::Read(c) => c.tail,
            },
            expected_runtime_id: match self {
                Self::Mutation(c) => c.expected_runtime_id.clone(),
                Self::Read(c) => c.expected_runtime_id.clone(),
            },
        })
    }
}
fn fingerprint(command: &StoredCommand) -> Result<String, Status> {
    command.fingerprint()
}
fn intent(command: &StoredCommand) -> wire::Intent {
    command.intent()
}
fn wire_command(command: &StoredCommand) -> Result<wire::Command, Status> {
    command.wire()
}
impl ControlPlane {
    pub async fn new(
        db: DatabaseConnection,
        hosting: Arc<HostingService>,
        local: Option<LocalBinding>,
    ) -> Result<Self, Status> {
        webdock_persistence::connection::verify_runtime_role(&db, "webdock_auth_bootstrap")
            .await
            .map_err(|_| denied())?;
        if let Some(binding) = &local {
            validate_identity(&wire::AgentIdentity {
                installation: binding.installation.clone(),
                agent_id: binding.agent_id.clone(),
                generation: 1,
            })?;
        }
        Ok(Self {
            db,
            hosting,
            hosting_enabled: true,
            plugin_runtime: None,
            local,
            databases: None,
            mail: None,
        })
    }
    pub fn with_databases(mut self, databases: Arc<webdock_databases::DatabaseService>) -> Self {
        self.databases = Some(databases);
        self
    }
    pub fn database_service(self) -> wire::database_control_server::DatabaseControlServer<Self> {
        wire::database_control_server::DatabaseControlServer::new(self)
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES)
    }
    pub fn with_mail(mut self, mail: Arc<webdock_mail::incoming::agent::IncomingAgent>) -> Self {
        self.mail = Some(mail);
        self
    }
    pub fn with_hosting_enabled(mut self, enabled: bool) -> Self {
        self.hosting_enabled = enabled;
        self
    }
    pub fn mail_service(self) -> wire::mail_control_server::MailControlServer<Self> {
        wire::mail_control_server::MailControlServer::new(self)
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES)
    }
    pub fn service(self) -> AgentControlServer<Self> {
        AgentControlServer::new(self)
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES)
    }
    async fn authorize<T>(
        &self,
        request: &Request<T>,
        identity: Option<&wire::AgentIdentity>,
    ) -> Result<AgentIdentity, Status> {
        let identity = identity.ok_or_else(invalid)?;
        validate_identity(identity)?;
        let peer = verified_peer(request, self.local.as_ref())?;
        if let Some(binding) = &self.local
            && (binding.installation != identity.installation
                || binding.agent_id != identity.agent_id)
        {
            return Err(denied());
        }
        let row=self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT e.cluster_id,e.credential_hash,e.transport,e.unix_uid,e.certificate_sha256 FROM webdock_auth.agent_transport_enrollment e WHERE e.installation=$1 AND e.agent_id=$2 AND e.generation=$3 AND NOT e.revoked",
            [identity.installation.clone().into(),identity.agent_id.clone().into(),(identity.generation as i32).into()])).await.map_err(unavailable)?.ok_or_else(denied)?;
        let transport: String = row.try_get("", "transport").map_err(unavailable)?;
        match peer {
            wire::TransportIdentity::Unix { uid, .. } => {
                if transport != "unix"
                    || row
                        .try_get::<Option<i64>>("", "unix_uid")
                        .map_err(unavailable)?
                        != Some(i64::from(uid))
                {
                    return Err(denied());
                }
            }
            wire::TransportIdentity::MutualTls(cert) => {
                if transport != "mtls"
                    || row
                        .try_get::<Option<String>>("", "certificate_sha256")
                        .map_err(unavailable)?
                        .as_deref()
                        != Some(cert.sha256.as_str())
                {
                    return Err(denied());
                }
            }
        }
        self.hosting
            .authenticate_enrolled_agent(
                &row.try_get::<String>("", "cluster_id")
                    .map_err(unavailable)?,
                identity.generation as i32,
                &row.try_get::<String>("", "credential_hash")
                    .map_err(unavailable)?,
            )
            .await
            .map_err(hosting_error)
    }
    async fn prepared_wire(
        &self,
        agent: &AgentIdentity,
        command: &StoredCommand,
    ) -> Result<wire::Command, Status> {
        let mut wire = wire_command(command)?;
        if let StoredCommand::Mutation(command) = command
            && command.action == Action::Apply
            && command.spec.hosted_access.is_some()
        {
            let material = self
                .hosting
                .runtime_material(agent, command)
                .await
                .map_err(hosting_error)?
                .ok_or_else(|| Status::failed_precondition("Runtime material unavailable"))?;
            let mut bound = command.clone();
            bound.material = Some(material.clone());
            bound.verify_material().map_err(|_| invalid())?;
            wire.material_json = serde_json::to_vec(&material).map_err(|_| invalid())?;
            if wire.material_json.len() > webdock_supervisor::MAX_RUNTIME_MATERIAL_BYTES {
                return Err(invalid());
            }
        }
        Ok(wire)
    }
    async fn guard_stored(
        &self,
        agent: &AgentIdentity,
        command: &StoredCommand,
    ) -> Result<(), Status> {
        match command {
            StoredCommand::Mutation(command) => self
                .hosting
                .guard_runtime_command(agent, command)
                .await
                .map_err(hosting_error),
            StoredCommand::Read(command) => self
                .hosting
                .guard_read(agent, command)
                .await
                .map_err(hosting_error),
        }
    }
    async fn dispatched(
        &self,
        identity: &wire::AgentIdentity,
        operation: &str,
        generation: u64,
    ) -> Result<StoredCommand, Status> {
        if generation == 0
            || generation > i64::MAX as u64
            || operation.len() > 19
            || !operation.bytes().all(|v| v.is_ascii_digit())
        {
            return Err(invalid());
        }
        let row=self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT command,request_sha256 FROM webdock_auth.agent_runtime_dispatch WHERE operation_id=$1 AND generation=$2 AND installation=$3 AND agent_id=$4 AND agent_generation=$5",
            [operation.into(),(generation as i64).into(),identity.installation.clone().into(),identity.agent_id.clone().into(),(identity.generation as i32).into()])).await.map_err(unavailable)?.ok_or_else(denied)?;
        let command: StoredCommand =
            serde_json::from_value(row.try_get("", "command").map_err(unavailable)?)
                .map_err(|_| Status::internal("Stored command invalid"))?;
        command.validate()?;
        if command.spec().installation != identity.installation
            || command.operation_id() != operation
            || command.generation() != generation
            || command.fingerprint()?
                != row
                    .try_get::<String>("", "request_sha256")
                    .map_err(unavailable)?
        {
            return Err(Status::unavailable(
                "Stored dispatch provenance unavailable",
            ));
        }
        Ok(command)
    }
}
#[tonic::async_trait]
impl AgentControl for ControlPlane {
    async fn register(
        &self,
        request: Request<wire::Hello>,
    ) -> Result<Response<wire::Welcome>, Status> {
        let hello = request.get_ref();
        self.authorize(&request, hello.identity.as_ref()).await?;
        if hello.protocol_version != 1 || hello.capabilities_json.len() > 65536 {
            return Err(invalid());
        }
        let capabilities: Value =
            serde_json::from_slice(&hello.capabilities_json).map_err(|_| invalid())?;
        if !capabilities.is_object() {
            return Err(invalid());
        }
        match verified_peer(&request, self.local.as_ref())? {
            wire::TransportIdentity::Unix { .. } => {
                if !hello.certificate_sha256.is_empty() {
                    return Err(denied());
                }
            }
            wire::TransportIdentity::MutualTls(cert) => {
                if cert.sha256 != hello.certificate_sha256 {
                    return Err(denied());
                }
            }
        }
        Ok(Response::new(wire::Welcome {
            accepted: true,
            protocol_version: 1,
            generation: hello.identity.as_ref().unwrap().generation,
        }))
    }
    async fn heartbeat(
        &self,
        request: Request<wire::HeartbeatRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let identity = input.identity.as_ref().unwrap();
        if input.sequence == 0
            || input.sequence > i64::MAX as u64
            || input.observed_at > now() + 30
            || input.observed_at < now().saturating_sub(90)
            || input.observation_json.len() > 65536
        {
            return Err(invalid());
        }
        if !input.observation_json.is_empty() {
            let mut observation: Value =
                serde_json::from_slice(&input.observation_json).map_err(|_| invalid())?;
            let value = observation.as_object_mut().ok_or_else(invalid)?;
            // The protocol envelope is authoritative for generation, sequence and observation time.
            value.insert("sequence".into(), json!(input.sequence));
            value.insert("generation".into(), json!(identity.generation));
            value.insert(
                "observedAt".into(),
                json!(
                    chrono::DateTime::from_timestamp(input.observed_at as i64, 0)
                        .ok_or_else(invalid)?
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                ),
            );
            self.hosting
                .agent_request(&agent, "heartbeat", observation)
                .await
                .map_err(hosting_error)?;
        }
        // Empty observations update only transport liveness, never capacity freshness.
        let count=self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.agent_transport_enrollment SET last_sequence=$4,last_seen=now() WHERE installation=$1 AND agent_id=$2 AND generation=$3 AND NOT revoked AND last_sequence<$4",[identity.installation.clone().into(),identity.agent_id.clone().into(),(identity.generation as i32).into(),(input.sequence as i64).into()])).await.map_err(unavailable)?.rows_affected();
        if count != 1 {
            return Err(Status::failed_precondition(
                "Heartbeat sequence was already processed",
            ));
        }
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn poll(
        &self,
        request: Request<wire::PollRequest>,
    ) -> Result<Response<wire::Dispatch>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        if !self.hosting_enabled {
            return Ok(Response::new(wire::Dispatch { command: None }));
        }
        let identity = input.identity.as_ref().unwrap();
        // A lost Poll response must redeliver the same lease, rather than strand it.
        if let Some(row) = self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT command FROM webdock_auth.agent_runtime_dispatch WHERE installation=$1 AND agent_id=$2 AND agent_generation=$3 AND receipt_sha256 IS NULL AND (command->>'leaseUntil')::bigint>$4 ORDER BY created_at,operation_id LIMIT 1",
            [identity.installation.clone().into(),identity.agent_id.clone().into(),(identity.generation as i32).into(),(now() as i64).into()])).await.map_err(unavailable)? {
        if !self
            .hosting
            .agent_dispatch_available(&agent)
            .await
            .map_err(hosting_error)?
        {
            self.authorize(&request, input.identity.as_ref()).await?;
            return Ok(Response::new(wire::Dispatch { command: None }));
        }
            let command:StoredCommand=serde_json::from_value(row.try_get("","command").map_err(unavailable)?).map_err(|_|Status::internal("Stored command invalid"))?;
            self.guard_stored(&agent,&command).await?;
            let wire=self.prepared_wire(&agent,&command).await?;
            self.authorize(&request,input.identity.as_ref()).await?;
            return Ok(Response::new(wire::Dispatch {command:Some(wire)}));
        }
        let command = if let Some(command) = self
            .hosting
            .claim_runtime(&agent)
            .await
            .map_err(hosting_error)?
        {
            StoredCommand::Mutation(command)
        } else if let Some(command) = self
            .hosting
            .claim_read(&agent)
            .await
            .map_err(hosting_error)?
        {
            StoredCommand::Read(command)
        } else {
            return Ok(Response::new(wire::Dispatch { command: None }));
        };
        command.validate()?;
        if command.spec().installation != identity.installation {
            return Err(denied());
        }
        let hash = fingerprint(&command)?;
        self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.agent_runtime_dispatch(operation_id,generation,installation,agent_id,agent_generation,request_sha256,command) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(operation_id,generation) DO NOTHING",
            [command.operation_id().to_owned().into(),(command.generation() as i64).into(),identity.installation.clone().into(),identity.agent_id.clone().into(),(identity.generation as i32).into(),hash.clone().into(),serde_json::to_value(&command).map_err(|_|invalid())?.into()])).await.map_err(unavailable)?;
        let stored = self
            .dispatched(identity, command.operation_id(), command.generation())
            .await?;
        if fingerprint(&stored)? != hash || stored.lease_until() != command.lease_until() {
            return Err(denied());
        }
        let wire = self.prepared_wire(&agent, &stored).await?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Dispatch {
            command: Some(wire),
        }))
    }
    async fn guard(
        &self,
        request: Request<wire::GuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        if !self.hosting_enabled {
            return Err(Status::failed_precondition("Hosting execution is disabled"));
        }
        let command = self
            .dispatched(
                input.identity.as_ref().unwrap(),
                &input.operation_id,
                input.generation,
            )
            .await?;
        if command.lease_until() != input.lease_until
            || intent(&command) as i32 != input.intent
            || fingerprint(&command)? != input.request_sha256
            || !bool::from(
                command
                    .authorization()
                    .as_bytes()
                    .ct_eq(input.authorization.as_bytes()),
            )
        {
            return Err(denied());
        }
        self.guard_stored(&agent, &command).await?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::GuardReply { allowed: true }))
    }
    async fn complete(
        &self,
        request: Request<wire::Receipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let identity = input.identity.as_ref().unwrap();
        let command = self
            .dispatched(identity, &input.operation_id, input.generation)
            .await?;
        if fingerprint(&command)? != input.request_sha256 {
            return Err(denied());
        }
        // Hash the exact wire receipt for idempotency; no plaintext environment/logging in diagnostics.
        let receipt_hash = hex::encode(Sha256::digest(prost::Message::encode_to_vec(input)));
        let row=self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT receipt_sha256 FROM webdock_auth.agent_runtime_dispatch WHERE operation_id=$1 AND generation=$2",[command.operation_id().to_owned().into(),(command.generation() as i64).into()])).await.map_err(unavailable)?.ok_or_else(denied)?;
        if let Some(old) = row
            .try_get::<Option<String>>("", "receipt_sha256")
            .map_err(unavailable)?
        {
            if old != receipt_hash {
                return Err(Status::failed_precondition(
                    "Completion differs from accepted receipt",
                ));
            }
            return Ok(Response::new(wire::Ack { accepted: true }));
        }
        if !self.hosting_enabled {
            return Err(Status::failed_precondition("Hosting execution is disabled"));
        }
        let completion = completion::Completion::parse(&command, input)?;
        if !self
            .retire_stale_receipt(&agent, &command, &receipt_hash)
            .await?
        {
            completion
                .complete(self, &agent, &command, &receipt_hash)
                .await?;
        }
        self.authorize(&request, input.identity.as_ref()).await?;
        let count=self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.agent_runtime_dispatch SET receipt_sha256=$3,completed_at=now() WHERE operation_id=$1 AND generation=$2 AND (receipt_sha256 IS NULL OR receipt_sha256=$3)",[command.operation_id().to_owned().into(),(command.generation() as i64).into(),receipt_hash.into()])).await.map_err(unavailable)?.rows_affected();
        if count != 1 {
            return Err(Status::failed_precondition("Completion receipt changed"));
        }
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}

impl ControlPlane {
    pub async fn serve_unix(
        self,
        mut socket: webdock_plugin_runtime::UnixSocket,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.local.is_none() {
            return Err("Unix enrollment binding required".into());
        }
        let incoming = socket.incoming()?;
        tonic::transport::Server::builder()
            .timeout(std::time::Duration::from_secs(30))
            .concurrency_limit_per_connection(32)
            .add_service(self.clone().database_service())
            .add_service(self.clone().recovery_service())
            .add_service(self.clone().mail_service())
            .add_service(self.clone().mail_configuration_service())
            .add_service(self.clone().workload_service())
            .add_service(self.clone().plugin_runtime_service())
            .add_service(self.service())
            .serve_with_incoming_shutdown(incoming, shutdown)
            .await?;
        drop(socket);
        Ok(())
    }
    pub async fn serve_mtls(
        self,
        listener: tokio::net::TcpListener,
        tls: webdock_plugin_runtime::MutualTls,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.local.is_some() {
            return Err("Unix enrollment binding cannot accept remote transport".into());
        }
        tonic::transport::Server::builder()
            .tls_config(
                tonic::transport::ServerTlsConfig::new()
                    .identity(tls.identity)
                    .client_ca_root(tls.peer_ca)
                    .client_auth_optional(false)
                    .timeout(std::time::Duration::from_secs(5)),
            )?
            .timeout(std::time::Duration::from_secs(30))
            .concurrency_limit_per_connection(32)
            .add_service(self.clone().database_service())
            .add_service(self.clone().recovery_service())
            .add_service(self.clone().mail_service())
            .add_service(self.clone().mail_configuration_service())
            .add_service(self.clone().workload_service())
            .add_service(self.clone().plugin_runtime_service())
            .add_service(self.service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                shutdown,
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_identity_claim_without_verified_transport_is_not_authentication() {
        let request = tonic::Request::new(());
        assert!(verified_peer(&request, None).is_err());
        let mut request = tonic::Request::new(());
        request.metadata_mut().insert(
            "x-client-cert-sha256",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap(),
        );
        assert!(verified_peer(&request, None).is_err());
    }
    #[test]
    fn command_scope_matches_the_claimed_installation_and_operation() {
        assert!(
            validate_identity(&wire::AgentIdentity {
                installation: "../outside".into(),
                agent_id: "agent".into(),
                generation: 1
            })
            .is_err()
        );
        assert!(
            validate_identity(&wire::AgentIdentity {
                installation: "local".into(),
                agent_id: "agent".into(),
                generation: 0
            })
            .is_err()
        );
        assert!(
            validate_identity(&wire::AgentIdentity {
                installation: "local".into(),
                agent_id: "agent".into(),
                generation: 1
            })
            .is_ok()
        );
    }
    #[test]
    fn stored_dispatch_keeps_legacy_mutation_json_and_distinguishes_reads() {
        let spec = json!({"version":1,"installation":"test","projectId":"123","resourceId":"456","revision":1,"image":format!("docker.io/library/busybox@sha256:{}","a".repeat(64)),"arguments":[],"cpuMillicores":100,"memoryBytes":33554432,"ephemeralBytes":16777216,"replicas":1,"port":8080,"healthPath":null,"volume":null});
        let mutation:Command=serde_json::from_value(json!({"operationId":"789","generation":1,"leaseUntil":9999999999u64,"authorization":"fixture-mac","action":"apply","spec":spec})).unwrap();
        let old = serde_json::to_value(&mutation).unwrap();
        let stored: StoredCommand = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(&stored).unwrap(), old);
        assert_eq!(
            stored.fingerprint().unwrap(),
            mutation.fingerprint().unwrap()
        );
        assert_eq!(stored.wire().unwrap().intent, wire::Intent::Apply as i32);
        let read:ReadCommand=serde_json::from_value(json!({"operationId":"790","generation":1,"leaseUntil":9999999999u64,"authorization":"fixture-mac","kind":"logs","tail":100,"expectedRuntimeId":"owned-runtime","spec":spec})).unwrap();
        let value = serde_json::to_value(&read).unwrap();
        let stored: StoredCommand = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&stored).unwrap(), value);
        let wire = stored.wire().unwrap();
        assert_eq!(wire.intent, wire::Intent::Logs as i32);
        assert_eq!(wire.expected_runtime_id.as_deref(), Some("owned-runtime"));
        assert_eq!(wire.log_tail, 100);
        let mut ambiguous = value;
        ambiguous["action"] = "apply".into();
        assert!(serde_json::from_value::<StoredCommand>(ambiguous).is_err());
    }
}
