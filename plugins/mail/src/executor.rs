//! Managed route policy and exactly one externally authorized transport attempt.
use crate::{
    delivery::private_policy_digest,
    seal::Sealer,
    store::Store,
    transport::{Platform, Sender},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use webdock_mail_protocol::{
    sha256, valid_digest, verify_lease,
    wire::{LeaseProof, mail_authority_client::MailAuthorityClient},
};
use webdock_mail_runtime::{
    ControlError, SendControl,
    dispatch::DispatchTransport,
    message::{DispatchMessage, PreparedMessage},
    outbox::{Receipt, SendLease},
    provider_transport::{ProviderBinding, SendAuthorization, metadata_bytes},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryConfiguration {
    pub native_lease_issuer_public_key_hex: String,
    pub provider_binding: Option<ProviderBinding>,
    pub gateway_peers: Vec<webdock_plugin_runtime::CorePeer>,
    pub native_accounts: Vec<NativeAccount>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeAccount {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "accountID", deserialize_with = "Option::deserialize")]
    pub account_id: Option<String>,
    pub mode: Mode,
    pub transport_revision: u64,
    pub binding_revision: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub byok_revision: Option<u64>,
    pub transport: Platform,
}
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Managed,
    Byok,
}
impl NativeAccount {
    fn matches(&self, t: &TransportMarker) -> bool {
        self.customer_id == t.customer_id
            && self.account_id == t.account_id
            && self.mode == t.mode
            && self.transport_revision == t.transport_revision
            && self.binding_revision == t.binding_revision
            && self.byok_revision == t.byok_revision
    }
}
pub struct Policy {
    pub store: Arc<Store>,
    platform: Platform,
    sealer: Sealer,
    accounts: Vec<NativeAccount>,
}
impl Policy {
    pub fn new(
        store: Arc<Store>,
        platform: Platform,
        sealer: Sealer,
        accounts: Vec<NativeAccount>,
    ) -> Result<Arc<Self>, &'static str> {
        platform.validate()?;
        if accounts.len() > 4096 {
            return Err("native account map limit");
        }
        let mut customers = std::collections::HashSet::new();
        for a in &accounts {
            a.transport.validate()?;
            if !customers.insert(a.customer_id.clone())
                || !webdock_mail_runtime::proof::valid_id(&a.customer_id)
                || a.account_id
                    .as_ref()
                    .is_some_and(|id| !webdock_mail_runtime::proof::valid_id(id))
                || [
                    Some(a.transport_revision),
                    Some(a.binding_revision),
                    a.byok_revision,
                ]
                .iter()
                .flatten()
                .any(|n| *n == 0 || *n > i64::MAX as u64)
            {
                return Err("invalid native account map");
            }
        }
        Ok(Arc::new(Self {
            store,
            platform,
            sealer,
            accounts,
        }))
    }
    async fn project_digest(&self, project: &str) -> Result<String, ControlError> {
        let relay = self
            .store
            .relay(project)
            .await
            .map_err(|_| ControlError::unavailable())?;
        Ok(private_policy_digest(
            project,
            relay.as_ref().map(|r| r.id),
            &self.platform,
        ))
    }
    pub async fn preflight(&self, job: &SendLease) -> Result<bool, ControlError> {
        if !self
            .store
            .direct_allowed(&job.customer_id)
            .await
            .map_err(|_| ControlError::unavailable())?
        {
            return Ok(false);
        }
        if job.source_context["executor"] != "managed-mail-v1"
            || job.source_context["payloadStoreID"] != "mail:platform:v1"
            || job.source_context["payloadStoreVersion"] != 1
            || job.route["kind"] != "direct"
        {
            return Ok(false);
        }
        if let Some(source) = job.source_id.strip_prefix("action:") {
            let Some(project) = job.source_context["projectID"]
                .as_str()
                .filter(|p| webdock_mail_runtime::proof::valid_id(p))
            else {
                return Ok(false);
            };
            if !valid_digest(source)
                || job.source_context["privatePolicySha256"] != self.project_digest(project).await?
            {
                return Ok(false);
            }
            let Some(resource) = job.source_context["resourceID"]
                .as_str()
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
            else {
                return Ok(false);
            };
            return Ok(self
                .store
                .domain(project, &resource)
                .await
                .map_err(|_| ControlError::unavailable())?
                .is_some_and(|d| job.source_context["domain"] == d.domain));
        }
        Ok(job
            .source_id
            .strip_prefix("key:")
            .is_some_and(webdock_mail_runtime::proof::valid_id)
            && self.accounts.iter().any(|a| {
                a.customer_id == job.customer_id
                    && job.route["transportRevision"] == a.transport_revision
            }))
    }
    async fn selected(&self, job: &SendLease, marker: &Marker) -> Result<Selected, ControlError> {
        let t = &marker.transport;
        if marker.kind != "managed"
            || marker.source_context != job.source_context
            || t.customer_id != job.customer_id
            || t.region != "eu"
            || job.route["transportRevision"] != t.transport_revision
            || t.binding_revision == 0
        {
            return Err(ControlError::unavailable());
        }
        if job.source_id.starts_with("action:") {
            let Some(project) = t
                .project_id
                .as_deref()
                .filter(|p| job.source_context["projectID"] == *p)
            else {
                return Err(ControlError::unavailable());
            };
            if t.selection != "project"
                || t.private_policy_sha256.as_deref()
                    != Some(self.project_digest(project).await?.as_str())
                || job.source_context["privatePolicySha256"].as_str()
                    != t.private_policy_sha256.as_deref()
            {
                return Err(ControlError::unavailable());
            }
            return match self
                .store
                .relay(project)
                .await
                .map_err(|_| ControlError::unavailable())?
            {
                Some(relay) => Ok(Selected::Own {
                    key: self
                        .sealer
                        .open(&relay.id, &relay.key_sealed)
                        .ok_or_else(ControlError::unavailable)?,
                    secret: self
                        .sealer
                        .open(&relay.id, &relay.secret_sealed)
                        .ok_or_else(ControlError::unavailable)?,
                }),
                None => Ok(Selected::Platform(self.platform.clone())),
            };
        }
        if t.selection != "native" || t.project_id.is_some() || t.private_policy_sha256.is_some() {
            return Err(ControlError::unavailable());
        }
        self.accounts
            .iter()
            .find(|a| a.matches(t))
            .map(|a| Selected::Platform(a.transport.clone()))
            .ok_or_else(ControlError::unavailable)
    }
}
/// Serializes ordinary Core admission/handoff with private incoming reservations across processes.
pub struct FencedControl {
    inner: Arc<dyn SendControl>,
    policy: Arc<Policy>,
}
impl FencedControl {
    pub fn new(inner: Arc<dyn SendControl>, policy: Arc<Policy>) -> Arc<Self> {
        Arc::new(Self { inner, policy })
    }
}
#[async_trait::async_trait]
impl SendControl for FencedControl {
    async fn call(&self, input: Value) -> Result<Value, ControlError> {
        let _recovery = if matches!(input["action"].as_str(), Some("send.admit" | "send.begin")) {
            Some(
                self.policy
                    .store
                    .recovery_guard()
                    .await
                    .map_err(|_| ControlError::unavailable())?,
            )
        } else {
            None
        };
        if input["action"] == "send.admit" {
            let customer = self
                .inner
                .call(json!({"action":"send.authenticate","token":input["submission"]["token"]}))
                .await?;
            let customer = customer["customerID"]
                .as_str()
                .filter(|s| webdock_mail_runtime::proof::valid_id(s))
                .ok_or_else(ControlError::unavailable)?;
            let _guard = self
                .policy
                .store
                .customer_fence(customer)
                .await
                .map_err(|_| ControlError::unavailable())?;
            if !self
                .policy
                .store
                .direct_allowed(customer)
                .await
                .map_err(|_| ControlError::unavailable())?
            {
                return Err(ControlError::unavailable());
            }
            return self.inner.call(input).await;
        }
        if input["action"] == "send.begin" {
            let job: SendLease = serde_json::from_value(input["lease"].clone())
                .map_err(|_| ControlError::invalid())?;
            let _guard = self
                .policy
                .store
                .customer_fence(&job.customer_id)
                .await
                .map_err(|_| ControlError::unavailable())?;
            if !self.policy.preflight(&job).await? {
                if self
                    .inner
                    .call(json!({"action":"send.reviewPreparation","lease":job}))
                    .await?
                    != true
                {
                    return Err(ControlError::unavailable());
                }
                return Ok(Value::Null);
            }
            return self.inner.call(input).await;
        }
        self.inner.call(input).await
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    kind: String,
    source_context: Value,
    transport: TransportMarker,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransportMarker {
    selection: String,
    #[serde(rename = "customerID")]
    customer_id: String,
    #[serde(rename = "accountID", deserialize_with = "Option::deserialize")]
    account_id: Option<String>,
    mode: Mode,
    region: String,
    transport_revision: u64,
    binding_revision: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    byok_revision: Option<u64>,
    #[serde(rename = "projectID")]
    project_id: Option<String>,
    private_policy_sha256: Option<String>,
}
enum Selected {
    Platform(Platform),
    Own { key: String, secret: String },
}
pub struct ManagedTransport {
    policy: Arc<Policy>,
    authority: MailAuthorityClient<tonic::transport::Channel>,
    binding: ProviderBinding,
    issuer: ed25519_dalek::VerifyingKey,
    sender: Sender,
}
impl ManagedTransport {
    pub fn new(
        policy: Arc<Policy>,
        channel: tonic::transport::Channel,
        binding: ProviderBinding,
        issuer: &str,
        sender: Sender,
        generation: u64,
    ) -> Result<Self, &'static str> {
        if binding.plugin_id != "mail"
            || binding.worker_id != "mail"
            || binding.generation != generation
            || binding.worker_generation != generation
            || !valid_digest(&binding.descriptor_sha256)
            || !valid_digest(&binding.worker_credential_sha256)
        {
            return Err("invalid managed lease binding");
        }
        let bytes: [u8; 32] = hex::decode(issuer)
            .map_err(|_| "invalid native lease issuer")?
            .try_into()
            .map_err(|_| "invalid native lease issuer")?;
        let issuer = ed25519_dalek::VerifyingKey::from_bytes(&bytes)
            .map_err(|_| "invalid native lease issuer")?;
        Ok(Self {
            policy,
            authority: MailAuthorityClient::new(channel)
                .max_decoding_message_size(66560)
                .max_encoding_message_size(17408),
            binding,
            issuer,
            sender,
        })
    }
    fn verify(
        &self,
        job: &SendLease,
        authorization: &Value,
        message: &DispatchMessage,
        prepared: &PreparedMessage,
    ) -> Result<String, ()> {
        let auth: SendAuthorization =
            serde_json::from_value(authorization.clone()).map_err(|_| ())?;
        let SendAuthorization::Direct {
            lease_token,
            plugin_id,
            generation,
            descriptor_sha256,
        } = auth
        else {
            return Err(());
        };
        if plugin_id != self.binding.plugin_id
            || generation != self.binding.generation
            || descriptor_sha256 != self.binding.descriptor_sha256
        {
            return Err(());
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ())?
            .as_secs() as i64;
        let c = verify_lease(&lease_token, &self.issuer, &plugin_id, generation, now)
            .map_err(|_| ())?;
        if c.customer_id != job.customer_id
            || c.job_id != job.id
            || c.source_id != job.source_id
            || c.worker_id != self.binding.worker_id
            || c.worker_generation != self.binding.worker_generation
            || c.worker_credential_sha256 != self.binding.worker_credential_sha256
            || c.descriptor_sha256 != self.binding.descriptor_sha256
            || c.metadata_sha256 != sha256(&metadata_bytes(message).map_err(|_| ())?)
            || c.mime_sha256 != sha256(&prepared.raw)
            || c.mime_bytes != prepared.raw.len() as u64
            || job.route["pluginID"] != c.plugin_id
            || job.route["generation"] != c.generation
            || job.route["transportRevision"] != c.transport_revision
            || job.route["grantRevision"] != c.grant_revision
            || job.route["configurationRevision"] != c.configuration_revision
            || job.route["descriptorSha256"] != c.descriptor_sha256
            || job.route["bindingSha256"] != c.binding_sha256
            || job.source_context["executorGeneration"] != self.binding.generation
        {
            return Err(());
        }
        Ok(lease_token)
    }
}
#[async_trait::async_trait]
impl DispatchTransport for ManagedTransport {
    fn can_submit(&self, job: &SendLease) -> bool {
        job.route["kind"] == "direct"
            && job.route["pluginID"] == "mail"
            && job.route["generation"] == self.binding.generation
            && job.route["descriptorSha256"] == self.binding.descriptor_sha256
            && job.source_context["executor"] == "managed-mail-v1"
    }
    async fn submit(
        &self,
        job: &SendLease,
        authorization: &Value,
        message: &DispatchMessage,
        prepared: &PreparedMessage,
    ) -> Receipt {
        let Ok(recovery) = self.policy.store.recovery_effect().await else {
            return Receipt::Unknown {};
        };
        let receipt = async {
            let Ok(token) = self.verify(job, authorization, message, prepared) else {
                return Receipt::Unknown {};
            };
            // Core handoff already committed. Recheck private reservation/policy before one-use authority.
            let Ok(_fence) = self.policy.store.customer_fence(&job.customer_id).await else {
                return Receipt::Unknown {};
            };
            if !matches!(self.policy.preflight(job).await, Ok(true)) {
                return Receipt::Unknown {};
            }
            let response = tokio::time::timeout(
                Duration::from_secs(15),
                self.authority
                    .clone()
                    .begin_attempt(LeaseProof { lease_token: token }),
            )
            .await;
            let Ok(Ok(response)) = response else {
                return Receipt::Unknown {};
            };
            let response = response.into_inner();
            if !response.admitted || response.private_configuration_json.len() > 65536 {
                return Receipt::Unknown {};
            }
            let Ok(marker) = serde_json::from_slice::<Marker>(&response.private_configuration_json)
            else {
                return Receipt::Unknown {};
            };
            let Ok(selected) = self.policy.selected(job, &marker).await else {
                return Receipt::Unknown {};
            };
            // The canonical handoff now blocks incoming transitions; release private DB connection before I/O.
            drop(_fence);
            let route = match &selected {
                Selected::Platform(platform) => crate::transport::Route::Platform(platform),
                Selected::Own { key, secret } => crate::transport::Route::Own {
                    key: key.clone(),
                    secret: secret.clone(),
                },
            };
            self.sender
                .submit_prepared(route, job, message, prepared)
                .await
        }
        .await;
        if recovery.finish().await.is_err() {
            return Receipt::Unknown {};
        }
        receipt
    }
}

/// Generic installer normal profile uses the same enrolled leaf for listener and callback.
/// Split certificates/manual Unix enrollment must supply the exact enrolled credential explicitly.
pub fn resolve_provider_binding(
    explicit: Option<ProviderBinding>,
    generation: u64,
    manifest: &[u8],
    listener_leaf: Option<&[u8]>,
    callback_leaf: Option<&[u8]>,
) -> Result<ProviderBinding, &'static str> {
    let descriptor = sha256(manifest);
    let binding = match explicit {
        Some(binding) => binding,
        None => {
            let leaf = listener_leaf
                .filter(|leaf| !leaf.is_empty() && Some(*leaf) == callback_leaf)
                .ok_or("explicit enrolled provider binding required")?;
            ProviderBinding {
                plugin_id: "mail".into(),
                generation,
                descriptor_sha256: descriptor.clone(),
                worker_id: "mail".into(),
                worker_generation: generation,
                worker_credential_sha256: sha256(leaf),
            }
        }
    };
    if generation == 0
        || generation > i64::MAX as u64
        || binding.plugin_id != "mail"
        || binding.worker_id != "mail"
        || binding.generation != generation
        || binding.worker_generation != generation
        || binding.descriptor_sha256 != descriptor
        || !valid_digest(&binding.worker_credential_sha256)
    {
        return Err("invalid enrolled provider binding");
    }
    Ok(binding)
}
