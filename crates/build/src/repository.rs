//! Generic external repository admission. It never creates synthetic interactive principals.
use super::*;
use crate::io;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use webdock_plugin_host::{HostService, RepositoryPolicySnapshot};
use webdock_plugin_runtime::{InvocationAuthority, PeerIdentity};
use webdock_repository_protocol::{self as protocol, Action, LeaseClaims, runtime, wire};
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RepositoryTransport {
    Unix {
        path: PathBuf,
    },
    MutualTls {
        origin: String,
        domain: String,
        certificate: PathBuf,
        private_key: PathBuf,
        authority: PathBuf,
        server_certificate_sha256: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryBinding {
    pub descriptor: String,
    pub publisher_key_id: String,
    pub publisher_public_key_hex: String,
    pub registry_url: String,
    pub transport: RepositoryTransport,
}
#[async_trait]
pub trait RepositoryBindings: Send + Sync {
    async fn binding(&self, plugin: &str) -> Result<Option<RepositoryBinding>>;
}
pub struct FileRepositoryBindings {
    path: PathBuf,
}
impl FileRepositoryBindings {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}
fn owned_file(path: &Path, max: u64, private: bool) -> Result<Vec<u8>> {
    if !path.is_absolute() {
        return Err(error("untrusted_repository_configuration"));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io)?;
    let m = file.metadata().map_err(io)?;
    if !m.is_file()
        // SAFETY: geteuid has no preconditions or pointer arguments.
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & if private { 0o077 } else { 0o022 } != 0
        || m.nlink() != 1
        || m.len() > max
    {
        return Err(error("untrusted_repository_configuration"));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > max {
        return Err(error("repository_configuration_limit"));
    }
    Ok(bytes)
}
#[async_trait]
impl RepositoryBindings for FileRepositoryBindings {
    async fn binding(&self, plugin: &str) -> Result<Option<RepositoryBinding>> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Config {
            runtimes: BTreeMap<String, RepositoryBinding>,
        }
        let raw = owned_file(&self.path, 1024 * 1024, true)?;
        let mut config: Config =
            serde_json::from_slice(&raw).map_err(|_| error("invalid_repository_configuration"))?;
        Ok(config.runtimes.remove(plugin))
    }
}
impl RepositoryBinding {
    fn descriptor(&self) -> Result<protocol::Descriptor> {
        let bytes = hex::decode(&self.publisher_public_key_hex)
            .map_err(|_| error("repository_signature_denied"))?;
        let key = VerifyingKey::from_bytes(
            &bytes
                .try_into()
                .map_err(|_| error("repository_signature_denied"))?,
        )
        .map_err(|_| error("repository_signature_denied"))?;
        let descriptor = protocol::verify_descriptor(&self.descriptor, &key)
            .map_err(|_| error("repository_signature_denied"))?;
        if descriptor.signer_key_id != self.publisher_key_id {
            return Err(error("repository_signature_denied"));
        }
        Ok(descriptor)
    }
    pub(crate) fn fingerprint(&self, policy: &RepositoryPolicySnapshot) -> Result<String> {
        let mut hash = Sha256::new();
        hash.update(
            serde_json::to_vec(self).map_err(|_| error("invalid_repository_configuration"))?,
        );
        hash.update(
            serde_json::to_vec(&policy.binding)
                .map_err(|_| error("invalid_repository_configuration"))?,
        );
        if let RepositoryTransport::MutualTls {
            certificate,
            private_key,
            authority,
            ..
        } = &self.transport
        {
            for (path, private) in [
                (certificate, false),
                (private_key, true),
                (authority, false),
            ] {
                let bytes = owned_file(path, 65536, private)?;
                hash.update((bytes.len() as u64).to_be_bytes());
                hash.update(bytes);
            }
        }
        Ok(hex::encode(hash.finalize()))
    }
    async fn connect(&self) -> Result<tonic::transport::Channel> {
        match &self.transport {
            RepositoryTransport::Unix { path } => runtime::connect_unix(path)
                .await
                .map_err(|_| error("repository_transport_unavailable")),
            RepositoryTransport::MutualTls {
                origin,
                domain,
                certificate,
                private_key,
                authority,
                server_certificate_sha256,
            } => webdock_plugin_runtime::pinned_mtls_channel(
                origin.clone(),
                domain.clone(),
                webdock_plugin_runtime::PinnedMutualTls {
                    certificate: owned_file(certificate, 65536, false)?,
                    private_key: owned_file(private_key, 65536, true)?,
                    authority: owned_file(authority, 65536, false)?,
                    server_certificate_sha256: server_certificate_sha256.clone(),
                },
            )
            .await
            .map_err(|_| error("repository_transport_unavailable")),
        }
    }
}
#[derive(Clone)]
pub struct RepositoryAdmission {
    worker: DatabaseConnection,
    secret: String,
    pub(crate) host: Arc<HostService>,
    issuer: Arc<SigningKey>,
    bindings: Arc<dyn RepositoryBindings>,
    authority: Option<Arc<super::session::SessionVerifier>>,
}
struct Admitted {
    claims: LeaseClaims,
    token: String,
    input: Vec<u8>,
    configuration: Vec<u8>,
    binding: RepositoryBinding,
}
struct Domain {
    kind: &'static str,
    subject: String,
    session: Option<String>,
    id: Option<String>,
    generation: Option<i32>,
    worker: Option<String>,
    worker_generation: Option<i32>,
    worker_hash: Option<String>,
    actor_authority: Option<Value>,
    archive_authority: Option<Value>,
    deadline: Option<i64>,
}
impl RepositoryAdmission {
    pub async fn new(
        worker: DatabaseConnection,
        secret: String,
        host: Arc<HostService>,
        issuer: SigningKey,
        bindings: Arc<dyn RepositoryBindings>,
    ) -> Result<Self> {
        if secret.len() < 32 {
            return Err(error("invalid_signer"));
        }
        webdock_persistence::connection::verify_runtime_role(&worker, "webdock_worker_runtime")
            .await
            .map_err(|_| error("invalid_database_role"))?;
        Ok(Self {
            worker,
            secret,
            host,
            issuer: Arc::new(issuer),
            bindings,
            authority: None,
        })
    }
    pub async fn with_session_authority(
        mut self,
        auth: DatabaseConnection,
        identity: Arc<webdock_identity::IdentityService>,
        browser: Option<Arc<webdock_browser_session::BrowserSessions>>,
    ) -> Result<Self> {
        self.authority = Some(Arc::new(
            super::session::SessionVerifier::new(auth, identity, browser).await?,
        ));
        Ok(self)
    }
    pub(crate) async fn tx(&self, git: bool) -> Result<DatabaseTransaction> {
        let tx = self.worker.begin().await.map_err(db_error)?;
        let purpose = if git {
            "git-worker"
        } else {
            "repository-admission"
        };
        let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":purpose,"authProvider":"better-auth"}}).to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes())
            .map_err(|_| error("invalid_signer"))?;
        mac.update(raw.as_bytes());
        execute(&tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true),set_config('lock_timeout','3s',true),set_config('statement_timeout','5s',true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await?;
        Ok(tx)
    }
    async fn domain(&self, source: &RepositoryRef<'_>) -> Result<Domain> {
        match &source.authority {
            RepositoryContext::SourceTrigger { tx } => {
                let row = BuildService::source(tx, &source.source_id, &source.project_id).await?;
                if row["revision"] != source.source_revision
                    || row["connection_generation"] != source.connection_generation
                    || row["webhook_trigger"] != "workflow_run"
                    || row["execution_backend"] != "kubernetes"
                {
                    return Err(error("repository_scope_changed"));
                }
                let actor = rows(*tx, "SELECT coalesce(webdock_rls.git_source_build_trigger_authority($1,$2),'null'::jsonb) AS value", vec![source.source_id.clone().into(),source.source_revision.into()])
                    .await?.pop().unwrap_or(Value::Null);
                if actor.is_null() {
                    return Err(error("repository_authority_revoked"));
                }
                Ok(Domain {
                    kind: "source-trigger",
                    subject: field(&actor, "subject")?.into(),
                    session: None,
                    id: Some(source.source_id.clone()),
                    generation: Some(
                        row["environment_revision"]
                            .as_i64()
                            .ok_or_else(|| error("invalid_source"))? as i32,
                    ),
                    worker: None,
                    worker_generation: None,
                    worker_hash: None,
                    actor_authority: None,
                    archive_authority: None,
                    deadline: None,
                })
            }
            RepositoryContext::PluginJob { proof } => {
                let proof = self
                    .host
                    .validate_export_job(
                        &proof
                            .snapshot()
                            .map_err(|_| error("repository_authority_revoked"))?,
                    )
                    .await
                    .map_err(|_| error("repository_authority_revoked"))?;
                let c = proof.claims();
                if c.customer_id != source.customer_id || c.project_id != source.project_id {
                    return Err(error("repository_scope_changed"));
                }
                let tx = self.tx(true).await?;
                let row = BuildService::source(&tx, &source.source_id, &source.project_id).await?;
                if row["revision"] != source.source_revision
                    || row["connection_generation"] != source.connection_generation
                {
                    return Err(error("repository_scope_changed"));
                }
                let job = c
                    .job
                    .as_ref()
                    .ok_or_else(|| error("repository_authority_revoked"))?;
                tx.commit().await.map_err(db_error)?;
                Ok(Domain {
                    kind: "plugin-job",
                    subject: proof.subject().into(),
                    session: Some(proof.session().into()),
                    id: Some(c.id.clone()),
                    generation: Some(i32::from(job.attempt)),
                    worker: None,
                    worker_generation: None,
                    worker_hash: None,
                    actor_authority: None,
                    archive_authority: Some(
                        proof
                            .snapshot()
                            .map_err(|_| error("repository_authority_revoked"))?,
                    ),
                    deadline: Some(c.exp),
                })
            }
            RepositoryContext::Session {
                principal,
                tx,
                authority,
            } => {
                if principal.in_read_only_preview() {
                    return Err(error("permission_denied"));
                }
                let access =
                    BuildService::authorize(tx, principal, &source.project_id, true, false).await?;
                if access["project"]["customer_id"] != source.customer_id {
                    return Err(error("permission_denied"));
                }
                Ok(Domain {
                    kind: "session",
                    subject: principal.user_id.clone(),
                    session: Some(principal.session_id.clone()),
                    id: None,
                    generation: None,
                    worker: None,
                    worker_generation: None,
                    worker_hash: None,
                    actor_authority: authority
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(|_| error("invalid_authority"))?,
                    archive_authority: None,
                    deadline: None,
                })
            }
            RepositoryContext::Build {
                worker,
                id,
                generation,
            } => {
                let tx = self.tx(true).await?;
                let b = BuildService::lease(&tx, worker, id, *generation).await?;
                if b["source_id"] != source.source_id
                    || b["project_id"] != source.project_id
                    || b["customer_id"] != source.customer_id
                    || b["source_revision"] != source.source_revision
                    || b["connection_generation"] != source.connection_generation
                {
                    return Err(error("repository_scope_changed"));
                }
                let subject = field(&b, "subject")?.into();
                tx.commit().await.map_err(db_error)?;
                Ok(Domain {
                    kind: "build",
                    subject,
                    session: None,
                    id: Some(id.clone()),
                    generation: Some(*generation),
                    worker: Some(worker.id.clone()),
                    worker_generation: Some(worker.generation),
                    worker_hash: Some(worker.hash.clone()),
                    actor_authority: None,
                    archive_authority: None,
                    deadline: None,
                })
            }
            RepositoryContext::Release {
                worker,
                id,
                generation,
            } => {
                let tx = self.tx(true).await?;
                let r = BuildService::release_lease(&tx, worker, id, *generation).await?;
                if r["source_id"] != source.source_id
                    || r["project_id"] != source.project_id
                    || r["customer_id"] != source.customer_id
                    || r["source_revision"] != source.source_revision
                    || r["connection_generation"] != source.connection_generation
                {
                    return Err(error("repository_scope_changed"));
                }
                let subject = field(&r, "approved_by")?.into();
                tx.commit().await.map_err(db_error)?;
                Ok(Domain {
                    kind: "release",
                    subject,
                    session: r["approved_session_id"]
                        .as_str()
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned),
                    id: Some(id.clone()),
                    generation: Some(*generation),
                    worker: Some(worker.id.clone()),
                    worker_generation: Some(worker.generation),
                    worker_hash: Some(worker.hash.clone()),
                    actor_authority: r["publication_target"]
                        .get("approvedAuthority")
                        .filter(|v| !v.is_null())
                        .cloned(),
                    archive_authority: None,
                    deadline: None,
                })
            }
        }
    }
    pub(crate) async fn policy(
        &self,
        plugin: &str,
        project: &str,
        action: Action,
        input: &Value,
        continuation: bool,
    ) -> Result<(RepositoryPolicySnapshot, RepositoryBinding)> {
        let snapshot = self
            .host
            .repository_policy(plugin, project, action.manifest_name(), input, continuation)
            .await
            .map_err(|_| error("repository_plugin_denied"))?;
        let binding = self
            .bindings
            .binding(plugin)
            .await?
            .ok_or_else(|| error("repository_provider_unavailable"))?;
        let descriptor = binding.descriptor()?;
        if descriptor.plugin_id != plugin
            || descriptor.version != snapshot.version
            || descriptor.manifest_sha256 != snapshot.manifest_sha256
            || descriptor.image != snapshot.image
            || descriptor.signer_key_id != snapshot.signer_key_id
            || binding.registry_url != snapshot.registry_url
        {
            return Err(error("repository_descriptor_changed"));
        }
        Ok((snapshot, binding))
    }
    async fn admit(
        &self,
        source: &RepositoryRef<'_>,
        action: Action,
        input: Value,
    ) -> Result<Admitted> {
        let plugin = source
            .plugin_id
            .as_deref()
            .ok_or_else(|| error("repository_provider_unavailable"))?;
        let domain = self.domain(source).await?;
        let (policy, binding) = self
            .policy(plugin, &source.project_id, action, &input, false)
            .await?;
        if policy.customer_id != source.customer_id {
            return Err(error("repository_scope_changed"));
        }
        let input = serde_json::to_vec(&input).map_err(|_| error("invalid_repository_request"))?;
        let configuration = serde_json::to_vec(&policy.configuration)
            .map_err(|_| error("invalid_repository_configuration"))?;
        if input.len() > protocol::FRAME_BYTES || configuration.len() > protocol::JSON_BYTES {
            return Err(error("repository_frame_limit"));
        }
        let now = chrono::Utc::now().timestamp();
        let claims = LeaseClaims {
            v: 1,
            purpose: "repository".into(),
            audience: "webdock.repository.runtime.v1".into(),
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: plugin.into(),
            generation: policy.generation,
            action,
            customer_id: source.customer_id.clone(),
            project_id: source.project_id.clone(),
            source_id: source.source_id.clone(),
            source_revision: source.source_revision as u64,
            connection_generation: source.connection_generation as u64,
            grant_revision: policy.grant_revision,
            configuration_revision: policy.configuration_revision,
            descriptor_sha256: protocol::sha256(binding.descriptor.as_bytes()),
            request_sha256: protocol::sha256(&input),
            configuration_sha256: protocol::sha256(&configuration),
            iat: now,
            exp: domain.deadline.unwrap_or(now + 60).min(now + 60),
        };
        let token = claims
            .sign(&self.issuer)
            .map_err(|_| error("repository_signing_unavailable"))?;
        let tx = self.tx(false).await?;
        execute(&tx,"INSERT INTO webdock_auth.repository_lease(id,plugin_id,customer_id,project_id,source_id,generation,source_revision,connection_generation,grant_revision,configuration_revision,action,descriptor_sha256,binding_sha256,request_sha256,configuration_sha256,input_json,configuration_json,authority_kind,actor_subject,actor_session,authority_id,authority_generation,worker_id,worker_generation,worker_credential_hash,issued_at,expires_at,actor_authority,archive_authority) VALUES($1::uuid,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,to_timestamp($26),to_timestamp($27),$28,$29)",vec![claims.id.clone().into(),plugin.into(),source.customer_id.clone().into(),source.project_id.clone().into(),source.source_id.clone().into(),(claims.generation as i64).into(),source.source_revision.into(),source.connection_generation.into(),(claims.grant_revision as i64).into(),(claims.configuration_revision as i64).into(),match action{Action::Resolve=>"resolve",Action::Archive=>"archive",Action::Check=>"check"}.into(),claims.descriptor_sha256.clone().into(),binding.fingerprint(&policy)?.into(),claims.request_sha256.clone().into(),claims.configuration_sha256.clone().into(),String::from_utf8(input.clone()).unwrap().into(),String::from_utf8(configuration.clone()).unwrap().into(),domain.kind.into(),domain.subject.into(),domain.session.into(),domain.id.into(),domain.generation.into(),domain.worker.into(),domain.worker_generation.into(),domain.worker_hash.into(),(now as f64).into(),(claims.exp as f64).into(),domain.actor_authority.into(),domain.archive_authority.into()]).await?;
        let active = rows(
            &tx,
            "SELECT coalesce(webdock_rls.repository_domain_live($1::uuid),'null'::jsonb) AS value",
            vec![claims.id.clone().into()],
        )
        .await?
        .pop()
        .unwrap_or(Value::Null);
        if active.is_null() {
            return Err(error("repository_authority_revoked"));
        }
        tx.commit().await.map_err(db_error)?;
        self.validate(&claims, None).await?;
        Ok(Admitted {
            claims,
            token,
            input,
            configuration,
            binding,
        })
    }
    async fn validate(&self, claims: &LeaseClaims, peer: Option<&PeerIdentity>) -> Result<()> {
        claims
            .validate(chrono::Utc::now().timestamp())
            .map_err(|_| error("repository_authority_revoked"))?;
        let tx = self.tx(false).await?;
        let row=rows(&tx,"SELECT to_jsonb(l)||jsonb_build_object('issued_epoch',extract(epoch from issued_at)::bigint,'expires_epoch',extract(epoch from expires_at)::bigint) AS value FROM webdock_auth.repository_lease l WHERE id=$1::uuid AND state='active' AND expires_at>now()",vec![claims.id.clone().into()]).await?.pop().ok_or_else(||error("repository_authority_revoked"))?;
        for (field_name, expected) in [
            ("action", serde_json::to_value(claims.action).unwrap()),
            ("issued_epoch", json!(claims.iat)),
            ("expires_epoch", json!(claims.exp)),
            ("plugin_id", json!(claims.plugin_id)),
            ("customer_id", json!(claims.customer_id)),
            ("project_id", json!(claims.project_id)),
            ("source_id", json!(claims.source_id)),
            ("generation", json!(claims.generation)),
            ("source_revision", json!(claims.source_revision)),
            ("connection_generation", json!(claims.connection_generation)),
            ("grant_revision", json!(claims.grant_revision)),
            (
                "configuration_revision",
                json!(claims.configuration_revision),
            ),
            ("descriptor_sha256", json!(claims.descriptor_sha256)),
            ("request_sha256", json!(claims.request_sha256)),
            ("configuration_sha256", json!(claims.configuration_sha256)),
        ] {
            if row[field_name] != expected {
                return Err(error("repository_authority_revoked"));
            }
        }
        let input: Value = serde_json::from_str(field(&row, "input_json")?)
            .map_err(|_| error("invalid_repository_record"))?;
        if protocol::sha256(field(&row, "input_json")?.as_bytes()) != claims.request_sha256
            || protocol::sha256(field(&row, "configuration_json")?.as_bytes())
                != claims.configuration_sha256
        {
            return Err(error("repository_authority_revoked"));
        }
        tx.commit().await.map_err(db_error)?;
        if row["authority_kind"] == "plugin-job" {
            self.host
                .validate_export_job(&row["archive_authority"])
                .await
                .map_err(|_| error("repository_authority_revoked"))?;
        }
        let authority_guard = if let Some(value) =
            row.get("actor_authority").filter(|v| !v.is_null())
        {
            let stored: webdock_browser_session::StoredAuthority =
                serde_json::from_value(value.clone())
                    .map_err(|_| error("repository_authority_revoked"))?;
            if row["authority_kind"] == "release" {
                let proof_tx = self.tx(true).await?;
                let current=rows(&proof_tx,"SELECT coalesce(publication_target->'approvedAuthority','null'::jsonb) AS value FROM webdock_auth.git_release WHERE id=$1",vec![field(&row,"authority_id")?.into()]).await?.pop().ok_or_else(||error("repository_authority_revoked"))?;
                if current != *value {
                    return Err(error("repository_authority_revoked"));
                }
                proof_tx.commit().await.map_err(db_error)?;
            }
            Some(
                self.authority
                    .as_ref()
                    .ok_or_else(|| error("repository_authority_unavailable"))?
                    .begin(
                        field(&row, "actor_subject")?,
                        field(&row, "actor_session")?,
                        &stored,
                        true,
                    )
                    .await?,
            )
        } else {
            None
        };
        let (policy, binding) = self
            .policy(
                &claims.plugin_id,
                &claims.project_id,
                claims.action,
                &input,
                true,
            )
            .await?;
        if policy.customer_id != claims.customer_id
            || policy.generation != claims.generation
            || policy.grant_revision != claims.grant_revision
            || policy.configuration_revision != claims.configuration_revision
            || protocol::sha256(binding.descriptor.as_bytes()) != claims.descriptor_sha256
            || protocol::sha256(
                &serde_json::to_vec(&policy.configuration)
                    .map_err(|_| error("invalid_repository_configuration"))?,
            ) != claims.configuration_sha256
            || binding.fingerprint(&policy)? != row["binding_sha256"].as_str().unwrap_or("")
            || peer.is_some_and(|peer| !policy.permits_peer(peer))
        {
            return Err(error("repository_authority_revoked"));
        }
        let tx = self.tx(false).await?;
        let active = rows(
            &tx,
            "SELECT coalesce(webdock_rls.repository_domain_live($1::uuid),'null'::jsonb) AS value",
            vec![claims.id.clone().into()],
        )
        .await?
        .pop()
        .unwrap_or(Value::Null);
        if active.is_null() {
            return Err(error("repository_authority_revoked"));
        }
        super::session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        super::session::commit(authority_guard).await?;
        Ok(())
    }
    async fn finish(&self, claims: &LeaseClaims) -> Result<()> {
        self.validate(claims, None).await?;
        let tx = self.tx(false).await?;
        let done=rows(&tx,"UPDATE webdock_auth.repository_lease SET state='complete' WHERE id=$1::uuid AND state='active' AND webdock_rls.repository_domain_live(id) IS NOT NULL RETURNING to_jsonb(repository_lease) AS value",vec![claims.id.clone().into()]).await?;
        if done.len() != 1 {
            return Err(error("repository_authority_revoked"));
        }
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }
    pub fn authority_service(
        self: Arc<Self>,
    ) -> wire::repository_authority_server::RepositoryAuthorityServer<RepositoryAuthorityRpc> {
        wire::repository_authority_server::RepositoryAuthorityServer::new(RepositoryAuthorityRpc {
            gate: self,
        })
        .max_decoding_message_size(32 * 1024)
        .max_encoding_message_size(1024)
    }
}
pub struct RepositoryAuthorityRpc {
    gate: Arc<RepositoryAdmission>,
}
#[tonic::async_trait]
impl wire::repository_authority_server::RepositoryAuthority for RepositoryAuthorityRpc {
    async fn authorize_peer(
        &self,
        request: tonic::Request<wire::PeerProof>,
    ) -> std::result::Result<tonic::Response<wire::AuthorityReply>, tonic::Status> {
        let peer = runtime::peer(&request)?;
        let input = request.into_inner();
        let binding = self
            .gate
            .bindings
            .binding(&input.plugin_id)
            .await
            .map_err(|_| tonic::Status::permission_denied("Repository peer denied"))?
            .ok_or_else(|| tonic::Status::permission_denied("Repository peer denied"))?;
        binding
            .descriptor()
            .map_err(|_| tonic::Status::permission_denied("Repository peer denied"))?;
        if protocol::sha256(binding.descriptor.as_bytes()) != input.descriptor_sha256 {
            return Err(tonic::Status::permission_denied("Repository peer denied"));
        }
        self.gate
            .host
            .runtime_authority(input.plugin_id, input.generation)
            .map_err(|_| tonic::Status::permission_denied("Repository peer denied"))?
            .authorize_peer(&peer)
            .await
            .map_err(|_| tonic::Status::permission_denied("Repository peer denied"))?;
        Ok(tonic::Response::new(wire::AuthorityReply { active: true }))
    }
    async fn authorize(
        &self,
        request: tonic::Request<wire::LeaseProof>,
    ) -> std::result::Result<tonic::Response<wire::AuthorityReply>, tonic::Status> {
        let peer = runtime::peer(&request)?;
        let claims = protocol::verify_lease_issuer(
            &request.get_ref().lease_token,
            &self.gate.issuer.verifying_key(),
            chrono::Utc::now().timestamp(),
        )
        .map_err(|_| tonic::Status::permission_denied("Repository lease denied"))?;
        self.gate
            .validate(&claims, Some(&peer))
            .await
            .map_err(|_| tonic::Status::permission_denied("Repository lease denied"))?;
        Ok(tonic::Response::new(wire::AuthorityReply { active: true }))
    }
}
pub struct PluginRepositoryProvider {
    gate: Arc<RepositoryAdmission>,
}
impl PluginRepositoryProvider {
    pub fn new(gate: Arc<RepositoryAdmission>) -> Self {
        Self { gate }
    }
}
#[async_trait]
impl RepositoryProvider for PluginRepositoryProvider {
    async fn resolve(&self, source: &RepositoryRef<'_>, reference: &str) -> Result<VerifiedCommit> {
        let admitted=self.gate.admit(source,Action::Resolve,json!({"installationID":source.installation_id,"repositoryID":source.repository_id,"reference":reference})).await?;
        let mut client = wire::repository_runtime_client::RepositoryRuntimeClient::new(
            admitted.binding.connect().await?,
        )
        .max_decoding_message_size(protocol::JSON_BYTES + 1024);
        let descriptor = client
            .describe(wire::DescribeRequest {})
            .await
            .map_err(|_| error("repository_transport_unavailable"))?
            .into_inner();
        if serde_json::from_slice::<String>(&descriptor.signed_descriptor_json)
            .ok()
            .as_deref()
            != Some(admitted.binding.descriptor.as_str())
        {
            return Err(error("repository_descriptor_changed"));
        }
        let reply = client
            .resolve(wire::LeaseRequest {
                lease_token: admitted.token,
                input_json: admitted.input,
                configuration_json: admitted.configuration,
            })
            .await
            .map_err(|_| error("repository_request_denied"))?
            .into_inner();
        if reply.output_json.len() > protocol::JSON_BYTES {
            return Err(error("repository_response_limit"));
        }
        let value: Value = serde_json::from_slice(&reply.output_json)
            .map_err(|_| error("invalid_repository_response"))?;
        if value["repositoryID"] != source.repository_id {
            return Err(error("repository_scope_changed"));
        }
        let commit = VerifiedCommit::new(string(&value, "sha")?.into())?;
        self.gate.finish(&admitted.claims).await?;
        Ok(commit)
    }
    async fn archive(&self, source: &RepositoryRef<'_>, commit: &str) -> Result<Vec<u8>> {
        crate::contracts::sha(commit)?;
        let admitted=self.gate.admit(source,Action::Archive,json!({"installationID":source.installation_id,"repositoryID":source.repository_id,"commit":commit})).await?;
        let mut client = wire::repository_runtime_client::RepositoryRuntimeClient::new(
            admitted.binding.connect().await?,
        )
        .max_decoding_message_size(protocol::FRAME_BYTES + 1024);
        let stream = client
            .archive(wire::LeaseRequest {
                lease_token: admitted.token,
                input_json: admitted.input,
                configuration_json: admitted.configuration,
            })
            .await
            .map_err(|_| error("repository_request_denied"))?
            .into_inner();
        let bytes = runtime::collect_archive(stream)
            .await
            .map_err(|_| error("repository_archive_denied"))?;
        crate::artifact::Archive::read(&bytes, crate::artifact::MAX_SOURCE)?;
        self.gate.finish(&admitted.claims).await?;
        Ok(bytes)
    }
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    field(value, key)
}
