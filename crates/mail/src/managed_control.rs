//! Canonical managed Mail control. Host proves neutral action/runtime identity inside this ledger
//! transaction; private payload custody and transport remain in the external Mail runtime.
use crate::{
    Error, Result,
    authority::{self, Purpose},
    budget::{self, SendPath, Settlement},
    outbox::{self, Enqueue, PayloadFormat, SendLease, field, one, run},
    sending::{Envelope, Preparation, ProviderExecution, SendCommand, Sending},
    submissions::{QueueCommand, Submissions, completion_lock},
};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_mail_protocol::{
    LeaseClaims,
    native_control::{Control, ControlError},
    sha256, valid_digest,
};
use webdock_plugin_host::{HostService, MailRuntimeMode};
use webdock_plugin_runtime::PeerIdentity;
pub const EXECUTOR: &str = "managed-mail-v1";
pub const STORE_ID: &str = "mail:platform:v1";
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Command {
    Action(ActionCommand),
    Queue(Box<QueueCommand>),
    Send(Box<SendCommand>),
}
#[derive(Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum ActionCommand {
    #[serde(rename = "send.admitAction")]
    Admit { submission: ActionAdmission },
    #[serde(rename = "send.actionStatus")]
    Status {
        #[serde(rename = "invocationToken")]
        invocation_token: String,
        input: Value,
    },
    #[serde(rename = "send.checkTransition")]
    Transition {
        #[serde(rename = "invocationToken")]
        invocation_token: String,
        input: Value,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionAdmission {
    pub invocation_token: String,
    pub input: Value,
    pub payload_ref: String,
    pub private_policy_sha256: String,
}
pub struct ManagedControl {
    sender: Arc<Sending>,
    queue: Arc<Submissions>,
    host: Arc<HostService>,
}
impl ManagedControl {
    pub fn new(sender: Arc<Sending>, queue: Arc<Submissions>, host: Arc<HostService>) -> Self {
        Self {
            sender,
            queue,
            host,
        }
    }
    pub fn service(
        self: Arc<Self>,
    ) -> webdock_mail_protocol::wire::native_mail_control_server::NativeMailControlServer<
        webdock_mail_protocol::native_control::ControlServer,
    > {
        webdock_mail_protocol::native_control::ControlServer::service(self)
    }
    async fn execute(
        &self,
        peer: &PeerIdentity,
        generation: u64,
        value: Value,
    ) -> std::result::Result<Value, ControlError> {
        if !self.sender.config.enabled {
            return Err(ControlError::unavailable());
        }
        let command: Command =
            serde_json::from_value(value).map_err(|_| ControlError::invalid())?;
        let action_token = match &command {
            Command::Action(ActionCommand::Admit { submission }) => {
                Some(&submission.invocation_token)
            }
            Command::Action(
                ActionCommand::Status {
                    invocation_token, ..
                }
                | ActionCommand::Transition {
                    invocation_token, ..
                },
            ) => Some(invocation_token),
            _ => None,
        };
        // Canonical Browser/MCP auth comes first; discovery released its worker transaction.
        let action_guard = if let Some(token) = action_token {
            Some(
                self.host
                    .begin_mail_action(token)
                    .await
                    .map_err(host_error)?,
            )
        } else {
            None
        };
        let allocates = match &command {
            Command::Action(ActionCommand::Admit { .. }) => true,
            Command::Send(c) => matches!(&**c, SendCommand::Admit { .. }),
            _ => false,
        };
        let id = if allocates {
            Some(
                self.sender
                    .dependencies
                    .ids
                    .allocate()
                    .await
                    .map_err(|_| ControlError::unavailable())?
                    .to_string(),
            )
        } else {
            None
        };
        let mode = match &command {
            _ if allocates => MailRuntimeMode::Admission,
            Command::Queue(c)
                if matches!(
                    &**c,
                    QueueCommand::Finish { .. }
                        | QueueCommand::Cleanup {}
                        | QueueCommand::CleanupDone { .. }
                ) =>
            {
                MailRuntimeMode::Bookkeeping
            }
            _ => MailRuntimeMode::Dispatch,
        };
        let tx = authority::worker(
            &self.sender.dependencies,
            &self.sender.config.rls_secret,
            Purpose::Sending,
            None,
        )
        .await
        .map_err(error)?;
        self.host
            .mail_runtime_in(&tx, peer, generation, mode)
            .await
            .map_err(host_error)?;
        let result = match command {
            Command::Action(action) => {
                let guard = action_guard.as_ref().ok_or_else(ControlError::denied)?;
                if guard.claims.generation != generation {
                    return Err(ControlError::denied());
                }
                let input = match &action {
                    ActionCommand::Admit { submission } => &submission.input,
                    ActionCommand::Status { input, .. }
                    | ActionCommand::Transition { input, .. } => input,
                };
                let proof = self
                    .host
                    .mail_action_in(&tx, guard, input)
                    .await
                    .map_err(host_error)?;
                match action {
                    ActionCommand::Admit { submission } => {
                        if guard.claims.action != "mail.send" {
                            return Err(ControlError::denied());
                        }
                        self.admit_action(
                            &tx,
                            peer,
                            generation,
                            &proof,
                            submission,
                            id.ok_or_else(ControlError::unavailable)?,
                        )
                        .await
                        .map_err(error)?
                    }
                    ActionCommand::Status { input, .. } => self
                        .action_status(&tx, &guard.claims.action, &proof, &input)
                        .await
                        .map_err(error)?,
                    ActionCommand::Transition { input, .. } => self
                        .transition(&tx, &guard.claims.action, &proof, &input)
                        .await
                        .map_err(error)?,
                }
            }
            Command::Queue(command) => {
                if let QueueCommand::Status { token, id, .. } = &*command {
                    if !crate::proof::valid_id(id) {
                        return Err(ControlError::invalid());
                    }
                    let access = crate::keys::NativeKeys::authenticate_proof_in(&tx, token, None)
                        .await
                        .map_err(key_error)?;
                    one(&tx,"SELECT id FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2 AND source_id=$3",vec![id.clone().into(),access.customer_id.into(),format!("key:{}",access.key_id).into()]).await.map_err(error)?.ok_or_else(ControlError::denied)?;
                }
                self.queue
                    .execute_in(&tx, *command, Some(generation))
                    .await
                    .map_err(error)?
            }
            Command::Send(command) => match *command {
                SendCommand::Authenticate { token } => serde_json::to_value(
                    crate::keys::NativeKeys::authenticate_proof_in(&tx, &token, None)
                        .await
                        .map_err(key_error)?,
                )
                .map_err(|_| ControlError::unavailable())?,
                SendCommand::Preflight { token, envelope } => {
                    envelope.validate().map_err(error)?;
                    let access = crate::keys::NativeKeys::authenticate_proof_in(
                        &tx,
                        &token,
                        Some(&envelope.from),
                    )
                    .await
                    .map_err(key_error)?;
                    let policy = self
                        .sender
                        .policy
                        .snapshot(&tx, &access.customer_id, false, Some(peer))
                        .await
                        .map_err(error)?;
                    if !matches!(policy.execution, ProviderExecution::Managed { .. }) {
                        return Err(ControlError::denied());
                    }
                    crate::sending::sender_proof(&tx, &access.customer_id, &envelope.from)
                        .await
                        .map_err(key_error)?;
                    json!({"customerID":access.customer_id})
                }
                SendCommand::Admit { submission } => {
                    crate::keys::NativeKeys::authenticate_proof_in(
                        &tx,
                        &submission.token,
                        Some(&submission.envelope.from),
                    )
                    .await
                    .map_err(key_error)?;
                    self.sender
                        .admit_in(
                            &tx,
                            submission,
                            id.ok_or_else(ControlError::unavailable)?,
                            custody(generation),
                        )
                        .await
                        .map_err(key_error)?
                }
                SendCommand::Begin {
                    lease,
                    digest,
                    preparation,
                } => self
                    .begin(&tx, peer, generation, *lease, &digest, preparation)
                    .await
                    .map_err(error)?,
                _ => return Err(ControlError::invalid()),
            },
        };
        if let Some(guard) = &action_guard {
            self.host
                .verify_mail_action(guard)
                .await
                .map_err(host_error)?;
        }
        self.host
            .mail_runtime_in(&tx, peer, generation, mode)
            .await
            .map_err(host_error)?;
        tx.commit().await.map_err(|_| ControlError::unavailable())?;
        if let Some(guard) = action_guard {
            self.host
                .complete_mail_action(guard)
                .await
                .map_err(host_error)?;
        }
        Ok(result)
    }
    async fn admit_action(
        &self,
        tx: &DatabaseTransaction,
        peer: &PeerIdentity,
        generation: u64,
        proof: &Value,
        input: ActionAdmission,
        id: String,
    ) -> Result<Value> {
        if !uuid::Uuid::parse_str(&input.payload_ref).is_ok_and(|u| {
            u.to_string() == input.payload_ref
                && u.get_version_num() == 4
                && u.get_variant() == uuid::Variant::RFC4122
        }) || !valid_digest(&input.private_policy_sha256)
        {
            return Err(Error::Invalid);
        }
        let source = &proof["source"];
        let customer = text(source, "customerID")?;
        let compiled = webdock_mail_message::compile_action(input.input, text(source, "domain")?)
            .map_err(|_| Error::Invalid)?;
        let envelope: Envelope =
            serde_json::from_value(compiled.envelope).map_err(|_| Error::Invalid)?;
        let hash = sha256(&compiled.payload);
        let operation = text(&proof["invocation"], "operation_id")?;
        if !valid_digest(operation) {
            return Err(Error::Unavailable);
        }
        let source_id = format!("action:{operation}");
        let policy = self
            .sender
            .policy
            .snapshot(tx, customer, false, Some(peer))
            .await?;
        if !matches!(policy.execution, ProviderExecution::Managed { .. }) {
            return Err(Error::Unavailable);
        }
        one(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("mail-admission:{customer}").into()],
        )
        .await?;
        if let Some(row)=one(tx,"SELECT id,state,request_hash,source_context,payload_ref::text,CASE WHEN state='staging' AND lease_until>clock_timestamp() THEN lease_token::text END AS upload_token FROM webdock_mail.send_job WHERE customer_id=$1 AND source_id=$2 AND idempotency_key='mail.send'",vec![customer.into(),source_id.clone().into()]).await?{
            let ctx:Value=field(&row,"source_context")?;
            if field::<String>(&row,"request_hash")?!=hash||ctx["privatePolicySha256"]!=input.private_policy_sha256||!same_store(&ctx){return Err(Error::Invalid);}
            return Ok(json!({"id":field::<String>(&row,"id")?,"state":field::<String>(&row,"state")?,"created":false,"customerID":customer,"payloadRef":field::<String>(&row,"payload_ref")?,"uploadToken":field::<Option<String>>(&row,"upload_token")?}));
        }
        let reserved = budget::reserve(tx, customer, &id, envelope.recipients()).await?;
        if reserved.state != "reserved" || reserved.path != (SendPath::Direct {}) {
            return Err(Error::Unavailable);
        }
        deny_retained_incoming(tx, customer).await?;
        let mut context = custody(generation);
        context
            .as_object_mut()
            .ok_or(Error::Unavailable)?
            .extend(source.as_object().ok_or(Error::Unavailable)?.clone());
        context["transportSha256"] = json!(transport_sha256(&policy)?);
        context["operationID"] = json!(operation);
        context["privatePolicySha256"] = json!(input.private_policy_sha256);
        let request: Value =
            serde_json::from_slice(&compiled.payload).map_err(|_| Error::Invalid)?;
        context["subject"] = request["subject"].clone();
        let queued = outbox::enqueue(
            tx,
            &Enqueue {
                id,
                customer_id: customer.into(),
                source_id,
                idempotency_key: "mail.send".into(),
                request_hash: hash,
                payload_ref: input.payload_ref.clone(),
                payload_bytes: compiled
                    .payload
                    .len()
                    .try_into()
                    .map_err(|_| Error::Invalid)?,
                route: serde_json::to_value(policy.route(&reserved.path))
                    .map_err(|_| Error::Unavailable)?,
                envelope: serde_json::to_value(envelope).map_err(|_| Error::Invalid)?,
                payload_format: PayloadFormat::Request,
                source_context: context,
                staging: true,
                not_before: None,
            },
        )
        .await?;
        Ok(
            json!({"id":queued.id,"state":queued.state,"created":queued.created,"uploadToken":queued.upload_token,"customerID":customer,"payloadRef":input.payload_ref}),
        )
    }
    async fn action_status(
        &self,
        tx: &DatabaseTransaction,
        action: &str,
        proof: &Value,
        input: &Value,
    ) -> Result<Value> {
        let source = &proof["source"];
        let customer = text(source, "customerID")?;
        let resource = text(source, "resourceID")?;
        if action == "mail.history" {
            exact(input, &["resource_id"])?;
            let rows=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,state,envelope,source_context->>'subject' AS subject,route,created_at::text AS at FROM webdock_mail.send_job WHERE customer_id=$1 AND source_id LIKE 'action:%' AND source_context->>'resourceID'=$2 AND source_context->>'projectID'=$3 AND source_context->>'domain'=$4 ORDER BY created_at DESC,id DESC LIMIT 20",[customer.into(),resource.into(),text(source,"projectID")?.into(),text(source,"domain")?.into()])).await.map_err(|_|Error::Unavailable)?;
            let mut items = Vec::new();
            for row in rows {
                let envelope: Value = field(&row, "envelope")?;
                let route: Value = field(&row, "route")?;
                items.push(json!({"id":field::<String>(&row,"id")?,"state":field::<String>(&row,"state")?,"from":envelope["from"],"to":envelope["to"],"subject":field::<String>(&row,"subject")?,"transport":route["kind"],"at":field::<String>(&row,"at")?}));
            }
            return Ok(json!({"items":items}));
        }
        if !["mail.status", "mail.cancel"].contains(&action) {
            return Err(Error::Invalid);
        }
        exact(input, &["resource_id", "id"])?;
        let id = text(input, "id")?;
        if !crate::proof::valid_id(id) {
            return Err(Error::Invalid);
        }
        let row=one(tx,"SELECT state FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2 AND source_id LIKE 'action:%' AND source_context->>'resourceID'=$3 AND source_context->>'projectID'=$4 AND source_context->>'domain'=$5",vec![id.into(),customer.into(),resource.into(),text(source,"projectID")?.into(),text(source,"domain")?.into()]).await?.ok_or(Error::Unavailable)?;
        let mut state: String = field(&row, "state")?;
        if action == "mail.cancel" {
            completion_lock(tx, customer).await?;
            if outbox::cancel(tx, customer, id).await? {
                budget::settle(tx, customer, id, Settlement::Released).await?;
            }
            state = field(
                &one(
                    tx,
                    "SELECT state FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2",
                    vec![id.into(), customer.into()],
                )
                .await?
                .ok_or(Error::Unavailable)?,
                "state",
            )?;
        }
        Ok(json!({"id":id,"state":state}))
    }
    async fn transition(
        &self,
        tx: &DatabaseTransaction,
        action: &str,
        proof: &Value,
        input: &Value,
    ) -> Result<Value> {
        if !["incoming.activate", "mailboxes.create", "domains.delete"].contains(&action) {
            return Err(Error::Invalid);
        }
        let source = &proof["source"];
        let customer = text(source, "customerID")?;
        if (if action == "mailboxes.create" {
            &input["domain_id"]
        } else {
            &input["resource_id"]
        }) != &source["resourceID"]
        {
            return Err(Error::Invalid);
        }
        let row=one(tx,"SELECT EXISTS(SELECT 1 FROM webdock_mail.send_job WHERE customer_id=$1 AND state IN ('staging','queued','leased','preparation_review','handoff','unknown') AND route->>'kind'='direct' AND ($2::text IS NULL OR source_context->>'resourceID'=$2 OR envelope->>'from' LIKE '%@'||$3)) OR EXISTS(SELECT 1 FROM webdock_auth.mail_message WHERE customer_id=$1 AND NOT simulated AND dispatch_status IN ('scheduled','dispatching','outcome_unknown')) AS blocked",vec![customer.into(),(action=="domains.delete").then(||text(source,"resourceID")).transpose()?.into(),text(source,"domain")?.into()]).await?.ok_or(Error::Unavailable)?;
        if field::<bool>(&row, "blocked")? {
            return Err(Error::Unavailable);
        }
        Ok(json!({"allowed":true}))
    }
    async fn current(
        &self,
        tx: &DatabaseTransaction,
        job: &SendLease,
        snapshot: &crate::sending::ProviderSnapshot,
    ) -> Result<SendPath> {
        let envelope: Envelope =
            serde_json::from_value(job.envelope.clone()).map_err(|_| Error::Unavailable)?;
        envelope.validate()?;
        if job.source_id.starts_with("action:") {
            let operation = job
                .source_id
                .strip_prefix("action:")
                .ok_or(Error::Unavailable)?;
            if !valid_digest(operation)
                || job.source_context["operationID"] != operation
                || !self
                    .host
                    .mail_source_in(tx, &job.source_context)
                    .await
                    .map_err(|_| Error::Unavailable)?
                || !valid_digest(text(&job.source_context, "privatePolicySha256")?)
            {
                return Err(Error::Unavailable);
            }
            if envelope.from.split_once('@').map(|p| p.1)
                != Some(text(&job.source_context, "domain")?)
            {
                return Err(Error::Unavailable);
            }
        } else if let Some(key) = job.source_id.strip_prefix("key:") {
            crate::keys::NativeKeys::authorize_id_in(
                tx,
                &job.customer_id,
                key,
                Some(&envelope.from),
            )
            .await?;
            crate::sending::sender(tx, &job.customer_id, &envelope.from).await?;
        } else {
            return Err(Error::Unavailable);
        }
        deny_retained_incoming(tx, &job.customer_id).await?;
        let reserved =
            budget::reserve(tx, &job.customer_id, &job.id, envelope.recipients()).await?;
        if reserved.state != "reserved"
            || reserved.path != (SendPath::Direct {})
            || job.route
                != serde_json::to_value(snapshot.route(&reserved.path))
                    .map_err(|_| Error::Unavailable)?
        {
            return Err(Error::Unavailable);
        }
        Ok(reserved.path)
    }
    async fn begin(
        &self,
        tx: &DatabaseTransaction,
        peer: &PeerIdentity,
        generation: u64,
        submitted: SendLease,
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
        let policy = self
            .sender
            .policy
            .snapshot(tx, &submitted.customer_id, true, Some(peer))
            .await?;
        let Some(job) = crate::sending::read_job(
            tx,
            &submitted.id,
            &submitted.customer_id,
            &submitted.lease_token,
            "leased",
        )
        .await?
        else {
            return Ok(Value::Null);
        };
        if serde_json::to_value(&job).map_err(|_| Error::Unavailable)?
            != serde_json::to_value(&submitted).map_err(|_| Error::Unavailable)?
            || !dispatch_lane(&job.source_context, generation)
        {
            return Err(Error::Unavailable);
        }
        // Lookup failures roll back; only an authoritative negative source proof cancels.
        if job.source_id.starts_with("action:")
            && !self
                .host
                .mail_source_in(tx, &job.source_context)
                .await
                .map_err(|_| Error::Unavailable)?
        {
            completion_lock(tx, &job.customer_id).await?;
            if outbox::cancel(tx, &job.customer_id, &job.id).await? {
                budget::settle(tx, &job.customer_id, &job.id, Settlement::Released).await?;
            }
            return Ok(Value::Null);
        }
        if let Some(key) = job.source_id.strip_prefix("key:") {
            let envelope: Envelope =
                serde_json::from_value(job.envelope.clone()).map_err(|_| Error::Unavailable)?;
            let source = match crate::keys::NativeKeys::authorize_id_proof_in(
                tx,
                &job.customer_id,
                key,
                Some(&envelope.from),
            )
            .await
            {
                Ok(_) => crate::sending::sender_proof(tx, &job.customer_id, &envelope.from).await,
                Err(error) => Err(error),
            };
            match source {
                Ok(()) => {}
                Err(crate::keys::KeyFailure::Denied) => {
                    completion_lock(tx, &job.customer_id).await?;
                    if outbox::cancel(tx, &job.customer_id, &job.id).await? {
                        budget::settle(tx, &job.customer_id, &job.id, Settlement::Released).await?;
                    }
                    return Ok(Value::Null);
                }
                Err(error) => return Err(error.into()),
            }
        }
        if !transport_matches(&job.source_context, &policy)?
            || job.route
                != serde_json::to_value(policy.route(&SendPath::Direct {}))
                    .map_err(|_| Error::Unavailable)?
        {
            crate::sending::lock_job(tx, &job, "leased").await?;
            outbox::review_preparation(tx, &job).await?;
            return Ok(Value::Null);
        }
        self.current(tx, &job, &policy).await?;
        let ProviderExecution::Managed {
            worker_credential_sha256,
            ..
        } = &policy.execution
        else {
            return Err(Error::Unavailable);
        };
        crate::sending::lock_job(tx, &job, "leased").await?;
        if !outbox::begin_handoff(tx, &job).await? {
            return Ok(Value::Null);
        }
        let issued = chrono::Utc::now().timestamp();
        let claims = LeaseClaims {
            v: 1,
            purpose: "mail-send".into(),
            audience: "webdock.mail.runtime.v1".into(),
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: policy.plugin_id.clone(),
            generation: policy.generation,
            customer_id: job.customer_id.clone(),
            job_id: job.id.clone(),
            source_id: job.source_id.clone(),
            transport_revision: policy.transport_revision,
            grant_revision: policy.grant_revision,
            configuration_revision: policy.configuration_revision,
            descriptor_sha256: policy.descriptor_sha256.clone(),
            binding_sha256: policy.binding_sha256.clone(),
            worker_id: "mail".into(),
            worker_generation: generation,
            worker_credential_sha256: worker_credential_sha256.clone(),
            metadata_sha256: preparation.metadata_sha256,
            mime_sha256: preparation.mime_sha256,
            mime_bytes: preparation.mime_bytes,
            iat: issued,
            exp: issued + 60,
        };
        let token = claims
            .sign(&self.sender.signing)
            .map_err(|_| Error::Unavailable)?;
        run(tx,"INSERT INTO webdock_mail.provider_lease(id,job_id,customer_id,plugin_id,queue_lease_token,token_sha256,claims_json,issued_at,expires_at) VALUES($1::uuid,$2,$3,'mail',$4::uuid,$5,$6,to_timestamp($7::bigint),to_timestamp($8::bigint))",vec![claims.id.clone().into(),job.id.into(),job.customer_id.into(),job.lease_token.into(),sha256(token.as_bytes()).into(),serde_json::to_string(&claims).map_err(|_|Error::Unavailable)?.into(),issued.into(),(issued+60).into()]).await?;
        Ok(
            json!({"kind":"direct","leaseToken":token,"pluginID":"mail","generation":generation,"descriptorSha256":policy.descriptor_sha256}),
        )
    }
}
#[async_trait::async_trait]
impl Control for ManagedControl {
    async fn call(
        &self,
        peer: &PeerIdentity,
        generation: u64,
        command: Value,
    ) -> std::result::Result<Value, ControlError> {
        self.execute(peer, generation, command).await
    }
}
fn key_error(e: crate::keys::KeyFailure) -> ControlError {
    match e {
        crate::keys::KeyFailure::Invalid => ControlError::invalid(),
        crate::keys::KeyFailure::Denied => ControlError::denied(),
        crate::keys::KeyFailure::Unavailable => ControlError::unavailable(),
    }
}
fn error(e: Error) -> ControlError {
    match e {
        Error::Invalid => ControlError::invalid(),
        _ => ControlError::unavailable(),
    }
}
fn host_error(e: webdock_plugin_host::PluginError) -> ControlError {
    match e.code {
        webdock_plugin_host::ErrorCode::PermissionDenied => ControlError::denied(),
        webdock_plugin_host::ErrorCode::InvalidInput => ControlError::invalid(),
        _ => ControlError::unavailable(),
    }
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().ok_or(Error::Unavailable)
}
fn exact(v: &Value, keys: &[&str]) -> Result<()> {
    if v.as_object()
        .is_none_or(|o| o.len() != keys.len() || keys.iter().any(|k| !o.contains_key(*k)))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn custody(generation: u64) -> Value {
    json!({"executor":EXECUTOR,"payloadStoreID":STORE_ID,"payloadStoreVersion":1,"executorGeneration":generation})
}
pub(crate) fn transport_sha256(snapshot: &crate::sending::ProviderSnapshot) -> Result<String> {
    let ProviderExecution::Managed {
        transport,
        credentials_sha256,
        ..
    } = &snapshot.execution
    else {
        return Err(Error::Unavailable);
    };
    Ok(sha256(
        &serde_json::to_vec(&json!({"transport":transport,"credentialsSha256":credentials_sha256}))
            .map_err(|_| Error::Unavailable)?,
    ))
}
pub(crate) fn transport_matches(
    source: &Value,
    snapshot: &crate::sending::ProviderSnapshot,
) -> Result<bool> {
    Ok(source["transportSha256"].as_str() == Some(transport_sha256(snapshot)?.as_str()))
}
pub(crate) fn managed_source(source: &Value) -> bool {
    source["executor"] == EXECUTOR
}
fn same_store(source: &Value) -> bool {
    managed_source(source)
        && source["payloadStoreID"] == STORE_ID
        && source["payloadStoreVersion"] == 1
}
fn dispatch_lane(source: &Value, generation: u64) -> bool {
    same_store(source) && source["executorGeneration"] == generation
}
pub(crate) async fn deny_retained_incoming(tx: &DatabaseTransaction, customer: &str) -> Result<()> {
    let row=one(tx,"SELECT EXISTS(SELECT 1 FROM webdock_mail.service WHERE customer_id=$1) OR EXISTS(SELECT 1 FROM webdock_mail.instance WHERE customer_id=$1) AS blocked",vec![customer.into()]).await?.ok_or(Error::Unavailable)?;
    if field::<bool>(&row, "blocked")? {
        return Err(Error::Unavailable);
    }
    Ok(())
}
pub(crate) async fn validate_queue_lane(
    tx: &DatabaseTransaction,
    command: &QueueCommand,
    lane: Option<u64>,
) -> Result<()> {
    let (id, customer, token, submitted, terminal) = match command {
        QueueCommand::Status { id, .. } => (id.as_str(), None, None, None, false),
        QueueCommand::CommitPayload { lease } | QueueCommand::AbortPayload { lease } => (
            lease.id.as_str(),
            Some(lease.customer_id.as_str()),
            Some(lease.upload_token.as_str()),
            None,
            false,
        ),
        QueueCommand::ReviewPreparation { lease } => (
            lease.id.as_str(),
            Some(lease.customer_id.as_str()),
            Some(lease.lease_token.as_str()),
            Some(lease),
            false,
        ),
        QueueCommand::Finish { lease, .. } => (
            lease.id.as_str(),
            Some(lease.customer_id.as_str()),
            Some(lease.lease_token.as_str()),
            Some(lease),
            true,
        ),
        QueueCommand::CleanupDone { items } => {
            if items.len() > 50
                || items.iter().any(|item| {
                    !crate::proof::valid_id(&item.id)
                        || !crate::proof::valid_id(&item.customer_id)
                        || !uuid::Uuid::parse_str(&item.reference)
                            .is_ok_and(|u| u.to_string() == item.reference)
                })
            {
                return Err(Error::Invalid);
            }
            for item in items {
                let row=one(tx,"SELECT source_context FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2 AND payload_ref=$3::uuid AND state IN ('accepted','rejected','cancelled')",vec![item.id.clone().into(),item.customer_id.clone().into(),item.reference.clone().into()]).await?.ok_or(Error::Unavailable)?;
                check_lane(&field::<Value>(&row, "source_context")?, lane, true)?;
            }
            return Ok(());
        }
        QueueCommand::Claim {} | QueueCommand::Cleanup {} => return Ok(()),
    };
    let row=one(tx,"SELECT customer_id,state,source_context,lease_token::text AS token,route,envelope,payload_ref::text,payload_bytes,request_hash,payload_format,source_id FROM webdock_mail.send_job WHERE id=$1",vec![id.into()]).await?.ok_or(Error::Unavailable)?;
    let source: Value = field(&row, "source_context")?;
    let terminal = terminal
        || matches!(command, QueueCommand::Status { .. })
            && matches!(
                field::<String>(&row, "state")?.as_str(),
                "accepted" | "rejected" | "cancelled"
            );
    check_lane(&source, lane, terminal)?;
    if let Some(customer) = customer
        && field::<String>(&row, "customer_id")? != customer
    {
        return Err(Error::Unavailable);
    }
    if let Some(token) = token
        && field::<Option<String>>(&row, "token")?.as_deref() != Some(token)
    {
        return Err(Error::Unavailable);
    }
    if let Some(job) = submitted
        && (job.source_context != source
            || job.route != field::<Value>(&row, "route")?
            || job.envelope != field::<Value>(&row, "envelope")?
            || job.payload_ref != field::<String>(&row, "payload_ref")?
            || job.payload_bytes != field::<i32>(&row, "payload_bytes")?
            || job.request_hash != field::<String>(&row, "request_hash")?
            || job.source_id != field::<String>(&row, "source_id")?
            || job.payload_format.as_str() != field::<String>(&row, "payload_format")?)
    {
        return Err(Error::Unavailable);
    }
    Ok(())
}
fn check_lane(source: &Value, lane: Option<u64>, terminal: bool) -> Result<()> {
    let valid = match lane {
        Some(g) => same_store(source) && (terminal || dispatch_lane(source, g)),
        None => !managed_source(source),
    };
    if !valid {
        return Err(Error::Unavailable);
    }
    Ok(())
}
