//! Native admission and durable provider authority. Provider credentials leave Core only through
//! an authenticated, one-use callback after the queue handoff has committed.
pub use crate::incoming_send::IncomingConfig;
use crate::{
    Error, MailDependencies, Result,
    authority::{self, Purpose},
    budget::{self, SendPath, Settlement},
    keys::NativeKeys,
    message::envelope_address,
    outbox::{self, Enqueue, PayloadFormat, SendLease, field, one, run},
    submissions::{completion_lock, settle_journal},
};
use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use sea_orm::DatabaseTransaction;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};
use webdock_mail_protocol::{
    LeaseClaims, sha256, valid_digest, verify_lease,
    wire::{
        self,
        mail_authority_server::{MailAuthority, MailAuthorityServer},
    },
};
use webdock_plugin_runtime::PeerIdentity;

pub struct ProviderSnapshot {
    pub customer_id: String,
    pub plugin_id: String,
    pub generation: u64,
    pub grant_revision: u64,
    pub transport_revision: u64,
    pub configuration_revision: u64,
    pub descriptor_sha256: String,
    pub binding_sha256: String,
    pub execution: ProviderExecution,
}
pub enum ProviderExecution {
    LegacyHttp(webdock_plugin_http::HttpBinding),
    Managed {
        transport: Value,
        credentials_sha256: String,
        worker_credential_sha256: String,
    },
}
#[async_trait::async_trait]
pub trait ProviderPolicy: Send + Sync {
    async fn action_source(&self, _tx: &DatabaseTransaction, _source: &Value) -> Result<bool> {
        Err(Error::Unavailable)
    }
    /// The adapter holds policy locks in this transaction and validates the actual plugin peer.
    async fn snapshot(
        &self,
        tx: &DatabaseTransaction,
        customer: &str,
        continuation: bool,
        peer: Option<&PeerIdentity>,
    ) -> Result<ProviderSnapshot>;
}
pub struct SendingConfig {
    pub enabled: bool,
    pub rls_secret: String,
    pub signing_key: [u8; 32],
    pub worker_id: String,
    pub worker_generation: u64,
    pub worker_credential_sha256: String,
    pub incoming: Option<IncomingConfig>,
}
pub struct Sending {
    pub(crate) dependencies: MailDependencies,
    pub(crate) config: SendingConfig,
    pub(crate) signing: SigningKey,
    pub(crate) webmail: Option<Arc<crate::webmail::WebmailService>>,
    pub(crate) policy: Arc<dyn ProviderPolicy>,
}
pub use webdock_mail_protocol::outbox::Preparation;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub from: String,
    pub to: Vec<String>,
}
impl Envelope {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.to.is_empty() || self.to.len() > 100 || envelope_address(&self.from)? != self.from {
            return Err(Error::Invalid);
        }
        for recipient in &self.to {
            if envelope_address(recipient)? != *recipient {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    pub(crate) fn recipients(&self) -> i32 {
        self.to.iter().collect::<HashSet<_>>().len() as i32
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Admission {
    pub token: String,
    pub idempotency_key: String,
    pub request_hash: String,
    pub payload_ref: String,
    pub payload_bytes: i32,
    pub envelope: Envelope,
    pub not_before: Option<DateTime<Utc>>,
}
#[derive(Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum SendCommand {
    #[serde(rename = "send.preflight")]
    Preflight { token: String, envelope: Envelope },
    #[serde(rename = "send.policy.auth")]
    AuthenticatePolicy {
        #[serde(rename = "customerID")]
        customer_id: String,
        token: String,
    },
    #[serde(rename = "send.policy")]
    Policy {
        submission: Box<crate::smtp_policy::PolicySubmission>,
    },
    #[serde(rename = "send.authenticate")]
    Authenticate { token: String },
    #[serde(rename = "send.admit")]
    Admit { submission: Admission },
    #[serde(rename = "send.begin")]
    Begin {
        lease: Box<SendLease>,
        digest: String,
        preparation: Preparation,
    },
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Route {
    kind: RouteKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_revision: Option<String>,
    #[serde(rename = "pluginID")]
    plugin_id: String,
    generation: u64,
    transport_revision: u64,
    grant_revision: u64,
    configuration_revision: u64,
    descriptor_sha256: String,
    binding_sha256: String,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RouteKind {
    Direct,
    Stalwart,
}
impl ProviderSnapshot {
    pub(crate) fn route(&self, path: &SendPath) -> Route {
        Route {
            kind: match path {
                SendPath::Direct {} => RouteKind::Direct,
                SendPath::Stalwart { .. } => RouteKind::Stalwart,
            },
            service_revision: match path {
                SendPath::Direct {} => None,
                SendPath::Stalwart { service_revision } => Some(service_revision.clone()),
            },
            plugin_id: self.plugin_id.clone(),
            generation: self.generation,
            transport_revision: self.transport_revision,
            grant_revision: self.grant_revision,
            configuration_revision: self.configuration_revision,
            descriptor_sha256: self.descriptor_sha256.clone(),
            binding_sha256: self.binding_sha256.clone(),
        }
    }
    fn matches(&self, claims: &LeaseClaims) -> bool {
        self.customer_id == claims.customer_id
            && self.plugin_id == claims.plugin_id
            && self.generation == claims.generation
            && self.transport_revision == claims.transport_revision
            && self.grant_revision == claims.grant_revision
            && self.configuration_revision == claims.configuration_revision
            && self.descriptor_sha256 == claims.descriptor_sha256
            && self.binding_sha256 == claims.binding_sha256
    }
}
impl Sending {
    pub async fn new(
        dependencies: MailDependencies,
        config: SendingConfig,
        policy: Arc<dyn ProviderPolicy>,
    ) -> Result<Self> {
        authority::validate_pools(&dependencies).await?;
        if !(32..=4096).contains(&config.rls_secret.len())
            || config.worker_generation == 0
            || config.worker_generation > i64::MAX as u64
            || !valid_digest(&config.worker_credential_sha256)
            || config.worker_id.is_empty()
            || config.worker_id.len() > 64
            || !config.worker_id.as_bytes()[0].is_ascii_lowercase()
            || !config
                .worker_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(Error::Invalid);
        }
        if let Some(incoming) = &config.incoming {
            incoming.validate()?;
        }
        Ok(Self {
            signing: SigningKey::from_bytes(&config.signing_key),
            webmail: None,
            dependencies,
            config,
            policy,
        })
    }
    pub fn service(self: Arc<Self>) -> MailAuthorityServer<Self> {
        MailAuthorityServer::from_arc(self)
            .max_decoding_message_size(17000)
            .max_encoding_message_size(66560)
    }
    pub async fn execute(&self, command: SendCommand) -> Result<Value> {
        if !self.config.enabled {
            return Err(Error::Unavailable);
        }
        // Allocate before opening the transaction, using the coordinated durable allocator.
        let id = if matches!(command, SendCommand::Admit { .. }) {
            Some(
                self.dependencies
                    .ids
                    .allocate()
                    .await
                    .map_err(|_| Error::Unavailable)?
                    .to_string(),
            )
        } else {
            None
        };
        let module_write = !matches!(
            command,
            SendCommand::Authenticate { .. }
                | SendCommand::AuthenticatePolicy { .. }
                | SendCommand::Policy { .. }
                | SendCommand::Begin { .. }
        );
        let tx = authority::worker(
            &self.dependencies,
            &self.config.rls_secret,
            Purpose::Sending,
            Some(module_write),
        )
        .await?;
        let result = match command {
            SendCommand::Preflight { token, envelope } => {
                envelope.validate()?;
                let access = NativeKeys::authenticate_in(&tx, &token, Some(&envelope.from)).await?;
                self.policy
                    .snapshot(&tx, &access.customer_id, false, None)
                    .await?;
                sender(&tx, &access.customer_id, &envelope.from).await?;
                json!({"customerID":access.customer_id})
            }
            SendCommand::AuthenticatePolicy { customer_id, token } => {
                crate::smtp_policy::authenticate(&tx, &customer_id, &token).await?;
                json!({"ok":true})
            }
            SendCommand::Policy { submission } => match self.policy(&tx, *submission).await {
                Ok(value) => value,
                Err(Error::Invalid) => return Err(Error::Invalid),
                Err(_) => json!({"accept":false}),
            },
            SendCommand::Authenticate { token } => {
                serde_json::to_value(NativeKeys::authenticate_in(&tx, &token, None).await?)
                    .map_err(|_| Error::Unavailable)?
            }
            SendCommand::Admit { submission } => {
                self.admit(&tx, submission, id.ok_or(Error::Unavailable)?)
                    .await?
            }
            SendCommand::Begin {
                lease,
                digest,
                preparation,
            } => {
                if crate::managed_control::managed_source(&lease.source_context) {
                    return Err(Error::Unavailable);
                }
                self.begin(&tx, *lease, &digest, preparation).await?
            }
        };
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        Ok(result)
    }
    async fn admit(&self, tx: &DatabaseTransaction, input: Admission, id: String) -> Result<Value> {
        self.admit_in(tx, input, id, json!({}))
            .await
            .map_err(Error::from)
    }
    pub(crate) async fn admit_in(
        &self,
        tx: &DatabaseTransaction,
        input: Admission,
        id: String,
        mut source_context: Value,
    ) -> std::result::Result<Value, crate::keys::KeyFailure> {
        input.envelope.validate()?;
        let access =
            NativeKeys::authenticate_proof_in(tx, &input.token, Some(&input.envelope.from)).await?;
        let snapshot = self
            .policy
            .snapshot(tx, &access.customer_id, false, None)
            .await?;
        if matches!(snapshot.execution, ProviderExecution::Managed { .. })
            != crate::managed_control::managed_source(&source_context)
        {
            return Err(crate::keys::KeyFailure::Unavailable);
        }
        if crate::managed_control::managed_source(&source_context) {
            source_context["transportSha256"] =
                json!(crate::managed_control::transport_sha256(&snapshot)?);
        }
        sender_proof(tx, &access.customer_id, &input.envelope.from).await?;
        one(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("mail-admission:{}", access.customer_id).into()],
        )
        .await?;
        let source = format!("key:{}", access.key_id);
        if let Some(row)=one(tx,"SELECT id,state,request_hash,source_context,payload_ref::text,CASE WHEN state='staging' AND lease_until>clock_timestamp() THEN lease_token::text END AS upload_token FROM webdock_mail.send_job WHERE customer_id=$1 AND source_id=$2 AND idempotency_key=$3",vec![access.customer_id.clone().into(),source.clone().into(),input.idempotency_key.clone().into()]).await? {
            if field::<String>(&row,"request_hash")?!=input.request_hash {return Err(crate::keys::KeyFailure::Invalid);}
            if crate::managed_control::managed_source(&source_context) {
                let existing:Value=field(&row,"source_context")?;
                if !crate::managed_control::managed_source(&existing)||existing["payloadStoreID"]!=crate::managed_control::STORE_ID{return Err(crate::keys::KeyFailure::Unavailable);}

            }
            return Ok(json!({"id":field::<String>(&row,"id")?,"state":field::<String>(&row,"state")?,"created":false}));
        }
        let budget =
            budget::reserve(tx, &access.customer_id, &id, input.envelope.recipients()).await?;
        if budget.state != "reserved" {
            return Err(crate::keys::KeyFailure::Unavailable);
        }
        if let SendPath::Stalwart { service_revision } = &budget.path {
            self.config
                .incoming
                .as_ref()
                .ok_or(Error::Unavailable)?
                .connection(
                    tx,
                    &access.customer_id,
                    service_revision,
                    snapshot.transport_revision,
                )
                .await?;
        }
        let result = outbox::enqueue(
            tx,
            &Enqueue {
                id,
                customer_id: access.customer_id.clone(),
                source_id: source,
                idempotency_key: input.idempotency_key,
                request_hash: input.request_hash,
                payload_ref: input.payload_ref.clone(),
                payload_bytes: input.payload_bytes,
                route: serde_json::to_value(snapshot.route(&budget.path))
                    .map_err(|_| Error::Unavailable)?,
                envelope: serde_json::to_value(input.envelope).map_err(|_| Error::Invalid)?,
                payload_format: PayloadFormat::Request,
                source_context,
                staging: true,
                not_before: input.not_before,
            },
        )
        .await?;
        Ok(
            json!({"id":result.id,"state":result.state,"created":result.created,"uploadToken":result.upload_token,"customerID":access.customer_id,"payloadRef":input.payload_ref}),
        )
    }
    pub(crate) async fn current(
        &self,
        tx: &DatabaseTransaction,
        job: &SendLease,
        snapshot: &ProviderSnapshot,
    ) -> Result<SendPath> {
        let envelope: Envelope =
            serde_json::from_value(job.envelope.clone()).map_err(|_| Error::Unavailable)?;
        envelope.validate()?;
        if let Some(key) = job.source_id.strip_prefix("key:") {
            NativeKeys::authorize_id_in(tx, &job.customer_id, key, Some(&envelope.from)).await?;
        } else if job.source_id.starts_with("action:")
            && crate::managed_control::managed_source(&job.source_context)
        {
            if !self.policy.action_source(tx, &job.source_context).await? {
                return Err(Error::Unavailable);
            }
        } else {
            let access = self.queued_access(tx, job).await?;
            if access.sender_address != envelope.from {
                return Err(Error::Unavailable);
            }
        }
        if !job.source_id.starts_with("action:") {
            sender(tx, &job.customer_id, &envelope.from).await?;
        }
        if crate::managed_control::managed_source(&job.source_context) {
            if !crate::managed_control::transport_matches(&job.source_context, snapshot)? {
                return Err(Error::Unavailable);
            }
            crate::managed_control::deny_retained_incoming(tx, &job.customer_id).await?;
        }
        let route: Route =
            serde_json::from_value(job.route.clone()).map_err(|_| Error::Unavailable)?;
        let budget = budget::reserve(tx, &job.customer_id, &job.id, envelope.recipients()).await?;
        if budget.state != "reserved"
            || route != snapshot.route(&budget.path)
            || snapshot.customer_id != job.customer_id
        {
            return Err(Error::Unavailable);
        }
        Ok(budget.path)
    }
    pub(crate) async fn begin(
        &self,
        tx: &DatabaseTransaction,
        lease: SendLease,
        digest: &str,
        preparation: Preparation,
    ) -> Result<Value> {
        if !valid_digest(digest)
            || !valid_digest(&preparation.metadata_sha256)
            || !valid_digest(&preparation.mime_sha256)
            || !(1..=webdock_mail_protocol::MAX_MIME_BYTES).contains(&preparation.mime_bytes)
        {
            return Err(Error::Invalid);
        }
        // Policy locks precede queue row locks, matching revocation's lock order.
        let snapshot = self
            .policy
            .snapshot(tx, &lease.customer_id, true, None)
            .await;
        let Some(job) = read_job(
            tx,
            &lease.id,
            &lease.customer_id,
            &lease.lease_token,
            "leased",
        )
        .await?
        else {
            return Ok(Value::Null);
        };
        if !same_lease(&job, &lease)? {
            return Err(Error::Unavailable);
        }
        // Legacy route credentials cannot be promoted to a native provider grant. Preserve the
        // unsent payload and reservation for reviewed migration or explicit cancellation.
        if serde_json::from_value::<Route>(job.route.clone()).is_err() {
            outbox::review_preparation(tx, &job).await?;
            return Ok(Value::Null);
        }
        let allowed = match snapshot {
            Ok(snapshot) => self
                .current(tx, &job, &snapshot)
                .await
                .map(|path| (snapshot, path)),
            Err(error) => Err(error),
        };
        let Ok((snapshot, path)) = allowed else {
            completion_lock(tx, &job.customer_id).await?;
            if outbox::cancel(tx, &job.customer_id, &job.id).await? {
                budget::settle(tx, &job.customer_id, &job.id, Settlement::Released).await?;
                settle_journal(tx, &job.id, "cancelled").await?;
            }
            return Ok(Value::Null);
        };
        let smtp = if let SendPath::Stalwart { service_revision } = &path {
            Some(
                self.config
                    .incoming
                    .as_ref()
                    .ok_or(Error::Unavailable)?
                    .connection(
                        tx,
                        &job.customer_id,
                        service_revision,
                        snapshot.transport_revision,
                    )
                    .await?,
            )
        } else {
            None
        };
        lock_job(tx, &job, "leased").await?;
        if !outbox::begin_handoff(tx, &job).await? {
            return Ok(Value::Null);
        }
        if let (Some(smtp), SendPath::Stalwart { service_revision }) = (smtp, &path) {
            run(
                tx,
                "UPDATE webdock_mail.send_job SET message_digest=$2 WHERE id=$1",
                vec![job.id.clone().into(), digest.into()],
            )
            .await?;
            let envelope: Envelope =
                serde_json::from_value(job.envelope).map_err(|_| Error::Unavailable)?;
            let proof = crate::proof::sign_submission(
                &crate::proof::SubmissionProof {
                    customer_id: job.customer_id,
                    id: job.id.clone(),
                    digest: digest.into(),
                    from: envelope.from,
                    to: envelope.to,
                    transport_revision: snapshot.transport_revision.to_string(),
                    service_revision: service_revision.clone(),
                },
                &self
                    .config
                    .incoming
                    .as_ref()
                    .ok_or(Error::Unavailable)?
                    .encryption_key,
            )?;
            return Ok(
                json!({"kind":"stalwart","certificateSha256":smtp.certificate_sha256,"username":smtp.username,"password":smtp.password,"submissionID":job.id,"proof":proof}),
            );
        }
        let issued = Utc::now().timestamp();
        let claims = LeaseClaims {
            v: 1,
            purpose: "mail-send".into(),
            audience: "webdock.mail.runtime.v1".into(),
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: snapshot.plugin_id.clone(),
            generation: snapshot.generation,
            customer_id: job.customer_id.clone(),
            job_id: job.id.clone(),
            source_id: job.source_id.clone(),
            transport_revision: snapshot.transport_revision,
            grant_revision: snapshot.grant_revision,
            configuration_revision: snapshot.configuration_revision,
            descriptor_sha256: snapshot.descriptor_sha256.clone(),
            binding_sha256: snapshot.binding_sha256,
            worker_id: self.config.worker_id.clone(),
            worker_generation: self.config.worker_generation,
            worker_credential_sha256: self.config.worker_credential_sha256.clone(),
            metadata_sha256: preparation.metadata_sha256,
            mime_sha256: preparation.mime_sha256,
            mime_bytes: preparation.mime_bytes,
            iat: issued,
            exp: issued + 60,
        };
        let token = claims.sign(&self.signing).map_err(|_| Error::Unavailable)?;
        run(tx,"INSERT INTO webdock_mail.provider_lease(id,job_id,customer_id,plugin_id,queue_lease_token,token_sha256,claims_json,issued_at,expires_at) VALUES($1::uuid,$2,$3,$4,$5::uuid,$6,$7,to_timestamp($8::bigint),to_timestamp($9::bigint))",vec![claims.id.clone().into(),job.id.into(),job.customer_id.into(),claims.plugin_id.clone().into(),job.lease_token.into(),sha256(token.as_bytes()).into(),serde_json::to_string(&claims).map_err(|_|Error::Unavailable)?.into(),issued.into(),(issued+60).into()]).await?;
        Ok(
            json!({"kind":"direct","leaseToken":token,"pluginID":snapshot.plugin_id,"generation":snapshot.generation,"descriptorSha256":snapshot.descriptor_sha256}),
        )
    }
    async fn callback(
        &self,
        peer: &PeerIdentity,
        token: &str,
        attempt: bool,
    ) -> Result<Option<Value>> {
        if !self.config.enabled || token.len() > 16384 {
            return Err(Error::Unavailable);
        }
        let tx = authority::worker(
            &self.dependencies,
            &self.config.rls_secret,
            Purpose::Sending,
            Some(false),
        )
        .await?;
        let row=one(&tx,"SELECT claims_json,queue_lease_token::text FROM webdock_mail.provider_lease WHERE token_sha256=$1 AND attempted_at IS NULL AND revoked_at IS NULL AND expires_at>clock_timestamp()",vec![sha256(token.as_bytes()).into()]).await?.ok_or(Error::Unavailable)?;
        let persisted: LeaseClaims = serde_json::from_str(&field::<String>(&row, "claims_json")?)
            .map_err(|_| Error::Unavailable)?;
        let claims = verify_lease(
            token,
            &self.signing.verifying_key(),
            &persisted.plugin_id,
            persisted.generation,
            Utc::now().timestamp(),
        )
        .map_err(|_| Error::Unavailable)?;
        if claims != persisted {
            return Err(Error::Unavailable);
        }
        let snapshot = self
            .policy
            .snapshot(&tx, &claims.customer_id, true, Some(peer))
            .await?;
        let worker_matches = match &snapshot.execution {
            ProviderExecution::LegacyHttp(_) => {
                claims.worker_id == self.config.worker_id
                    && claims.worker_generation == self.config.worker_generation
                    && claims.worker_credential_sha256 == self.config.worker_credential_sha256
            }
            ProviderExecution::Managed {
                worker_credential_sha256,
                ..
            } => {
                claims.worker_id == "mail"
                    && claims.worker_generation == snapshot.generation
                    && claims.worker_credential_sha256 == *worker_credential_sha256
            }
        };
        if !worker_matches || !snapshot.matches(&claims) {
            return Err(Error::Unavailable);
        }
        let job = read_job(
            &tx,
            &claims.job_id,
            &claims.customer_id,
            &field::<String>(&row, "queue_lease_token")?,
            "handoff",
        )
        .await?
        .ok_or(Error::Unavailable)?;
        if job.source_id != claims.source_id {
            return Err(Error::Unavailable);
        }
        if self.current(&tx, &job, &snapshot).await? != (SendPath::Direct {}) {
            return Err(Error::Unavailable);
        }
        lock_job(&tx, &job, "handoff").await?;
        let result = if attempt {
            if !run(&tx,"UPDATE webdock_mail.provider_lease SET attempted_at=clock_timestamp() WHERE id=$1::uuid AND token_sha256=$2 AND attempted_at IS NULL AND revoked_at IS NULL AND expires_at>clock_timestamp()",vec![claims.id.into(),sha256(token.as_bytes()).into()]).await? {return Err(Error::Unavailable);}
            Some(match snapshot.execution {
                ProviderExecution::LegacyHttp(binding) => json!({"binding":binding}),
                ProviderExecution::Managed { mut transport, .. } => {
                    if !crate::managed_control::managed_source(&job.source_context) {
                        return Err(Error::Unavailable);
                    }
                    transport["selection"] = json!(if job.source_id.starts_with("action:") {
                        "project"
                    } else {
                        "native"
                    });
                    if job.source_id.starts_with("action:") {
                        transport["projectID"] = job.source_context["projectID"].clone();
                        transport["privatePolicySha256"] =
                            job.source_context["privatePolicySha256"].clone();
                    }
                    json!({"kind":"managed","sourceContext":job.source_context,"transport":transport})
                }
            })
        } else {
            None
        };
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        Ok(result)
    }
}
fn same_lease(a: &SendLease, b: &SendLease) -> Result<bool> {
    Ok(serde_json::to_value(a).map_err(|_| Error::Unavailable)?
        == serde_json::to_value(b).map_err(|_| Error::Unavailable)?)
}
pub(crate) async fn lock_job(tx: &DatabaseTransaction, job: &SendLease, state: &str) -> Result<()> {
    one(
        tx,
        "SELECT id FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2 FOR UPDATE",
        vec![job.id.clone().into(), job.customer_id.clone().into()],
    )
    .await?
    .ok_or(Error::Unavailable)?;
    let current = read_job(tx, &job.id, &job.customer_id, &job.lease_token, state)
        .await?
        .ok_or(Error::Unavailable)?;
    if !same_lease(job, &current)? {
        return Err(Error::Unavailable);
    }
    Ok(())
}
pub(crate) async fn read_job(
    tx: &DatabaseTransaction,
    id: &str,
    customer: &str,
    token: &str,
    state: &str,
) -> Result<Option<SendLease>> {
    if !crate::proof::valid_id(id)
        || !crate::proof::valid_id(customer)
        || !uuid::Uuid::parse_str(token).is_ok_and(|v| v.to_string() == token)
    {
        return Err(Error::Invalid);
    }
    let Some(row)=one(tx,"SELECT id,customer_id,source_id,payload_ref::text,payload_bytes,request_hash,payload_format,source_context,route,envelope,lease_token::text FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2 AND lease_token=$3::uuid AND state=$4 AND (lease_until>clock_timestamp() OR state='unknown' OR (state='handoff' AND route->>'kind'='stalwart'))",vec![id.into(),customer.into(),token.into(),state.into()]).await? else {return Ok(None);};
    Ok(Some(SendLease {
        id: field(&row, "id")?,
        customer_id: field(&row, "customer_id")?,
        source_id: field(&row, "source_id")?,
        payload_ref: field(&row, "payload_ref")?,
        payload_bytes: field(&row, "payload_bytes")?,
        request_hash: field(&row, "request_hash")?,
        payload_format: match field::<String>(&row, "payload_format")?.as_str() {
            "request" => PayloadFormat::Request,
            "mime" => PayloadFormat::Mime,
            _ => return Err(Error::Unavailable),
        },
        source_context: field(&row, "source_context")?,
        route: field(&row, "route")?,
        envelope: field(&row, "envelope")?,
        lease_token: field(&row, "lease_token")?,
    }))
}
pub(crate) async fn sender(tx: &DatabaseTransaction, customer: &str, from: &str) -> Result<()> {
    sender_proof(tx, customer, from).await.map_err(Error::from)
}
pub(crate) async fn sender_proof(
    tx: &DatabaseTransaction,
    customer: &str,
    from: &str,
) -> std::result::Result<(), crate::keys::KeyFailure> {
    let (_, domain) = from
        .split_once('@')
        .ok_or(crate::keys::KeyFailure::Invalid)?;
    let row=one(tx,"SELECT provider_id,provider_snapshot FROM webdock_auth.mail_sender_domain WHERE deleted_at IS NULL AND customer_id=$1 AND domain=$2 AND status='ownership_verified' AND provider_operation IS NULL FOR SHARE",vec![customer.into(),domain.into()]).await?.ok_or(crate::keys::KeyFailure::Denied)?;
    // Retained verified domain evidence is required even when a provider was explicitly adopted.
    let snapshot: Value = field(&row, "provider_snapshot")?;
    if snapshot["domain"] != domain
        || snapshot["id"]
            != field::<Option<String>>(&row, "provider_id")?
                .map(Value::String)
                .unwrap_or(Value::Null)
        || snapshot["spf_verified"] != true
        || snapshot["dkim_verified"] != true
        || snapshot["dmarc_verified"] != true
    {
        return Err(crate::keys::KeyFailure::Denied);
    }
    Ok(())
}
fn denied() -> tonic::Status {
    tonic::Status::permission_denied("Mail provider authority denied")
}
#[tonic::async_trait]
impl MailAuthority for Sending {
    async fn guard(
        &self,
        request: tonic::Request<wire::LeaseProof>,
    ) -> std::result::Result<tonic::Response<wire::GuardReply>, tonic::Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        self.callback(&peer, &request.into_inner().lease_token, false)
            .await
            .map_err(|_| denied())?;
        Ok(tonic::Response::new(wire::GuardReply { active: true }))
    }
    async fn begin_attempt(
        &self,
        request: tonic::Request<wire::LeaseProof>,
    ) -> std::result::Result<tonic::Response<wire::AttemptReply>, tonic::Status> {
        let peer = webdock_plugin_runtime::authenticated_peer(&request)?;
        let configuration = self
            .callback(&peer, &request.into_inner().lease_token, true)
            .await
            .map_err(|_| denied())?
            .ok_or_else(denied)?;
        let raw = serde_json::to_vec(&configuration).map_err(|_| denied())?;
        if raw.len() > 65536 {
            return Err(denied());
        }
        Ok(tonic::Response::new(wire::AttemptReply {
            admitted: true,
            private_configuration_json: raw,
        }))
    }
}
