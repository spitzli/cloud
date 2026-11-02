//! A fixed, authenticated provider channel. Provider credentials never enter this worker.
use crate::{
    Error, Result,
    dispatch::DispatchTransport,
    message::{DispatchMessage, PreparedMessage, Sender, stamp_message},
    outbox::{Receipt, SendLease},
    sending::Preparation,
    smtp::{SmtpOptions, submit},
};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use tonic::transport::Channel;
use webdock_mail_protocol::{
    FRAME_BYTES, MAX_METADATA_BYTES, MAX_MIME_BYTES, sha256, valid_digest, verify_lease,
    wire::{SendFrame, SendHeader, mail_runtime_client::MailRuntimeClient, send_receipt::State},
};

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SendAuthorization {
    Direct {
        lease_token: String,
        #[serde(rename = "pluginID")]
        plugin_id: String,
        generation: u64,
        descriptor_sha256: String,
    },
    Stalwart {
        certificate_sha256: String,
        username: String,
        password: String,
        #[serde(rename = "submissionID")]
        submission_id: String,
        proof: String,
    },
}

#[derive(Serialize)]
struct Metadata<'a> {
    sender: &'a Sender,
    to: &'a [String],
    cc: &'a [String],
    bcc: &'a [String],
    subject: &'a str,
}
/// Struct field order is part of the signed metadata commitment.
pub fn metadata_bytes(message: &DispatchMessage) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(&Metadata {
        sender: &message.sender,
        to: &message.to,
        cc: &message.cc,
        bcc: &message.bcc,
        subject: &message.subject,
    })
    .map_err(|_| Error::Invalid)?;
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(Error::Invalid);
    }
    Ok(bytes)
}
pub fn preparation(message: &DispatchMessage, prepared: &PreparedMessage) -> Result<Preparation> {
    if prepared.raw.is_empty() || prepared.raw.len() as u64 > MAX_MIME_BYTES {
        return Err(Error::Invalid);
    }
    Ok(Preparation {
        metadata_sha256: sha256(&metadata_bytes(message)?),
        mime_sha256: sha256(&prepared.raw),
        mime_bytes: prepared.raw.len() as u64,
    })
}

/// Operator configuration, independent of message metadata and control response routing fields.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBinding {
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    pub generation: u64,
    pub descriptor_sha256: String,
    #[serde(rename = "workerID")]
    pub worker_id: String,
    pub worker_generation: u64,
    pub worker_credential_sha256: String,
}
pub struct ProviderTransport {
    client: MailRuntimeClient<Channel>,
    binding: ProviderBinding,
    issuer: VerifyingKey,
}
pub const MAX_PROVIDERS: usize = 32;
/// Installed channels are chosen from the persisted route, never from authorization metadata.
pub struct RoutingTransport {
    providers: HashMap<(String, u64), ProviderTransport>,
}
impl RoutingTransport {
    pub fn new(providers: Vec<ProviderTransport>) -> Result<Self> {
        if providers.len() > MAX_PROVIDERS {
            return Err(Error::Invalid);
        }
        let mut routes = HashMap::new();
        for provider in providers {
            let key = (
                provider.binding.plugin_id.clone(),
                provider.binding.generation,
            );
            if routes.insert(key, provider).is_some() {
                return Err(Error::Invalid);
            }
        }
        Ok(Self { providers: routes })
    }
    fn provider(&self, job: &SendLease) -> Option<&ProviderTransport> {
        let key = (
            job.route["pluginID"].as_str()?.to_owned(),
            job.route["generation"].as_u64()?,
        );
        self.providers
            .get(&key)
            .filter(|provider| provider.can_submit(job))
    }
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeRoute {
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

fn native_route(job: &SendLease, kind: &str) -> bool {
    job.route["kind"] == kind
        && serde_json::from_value::<NativeRoute>(job.route.clone()).is_ok()
        && ["generation", "transportRevision", "grantRevision"]
            .iter()
            .all(|field| {
                job.route[field]
                    .as_u64()
                    .is_some_and(|n| n > 0 && i64::try_from(n).is_ok())
            })
        && job.route["configurationRevision"]
            .as_u64()
            .is_some_and(|n| i64::try_from(n).is_ok())
        && ["descriptorSha256", "bindingSha256"]
            .iter()
            .all(|field| job.route[field].as_str().is_some_and(valid_digest))
        && (kind != "stalwart"
            || job.route["serviceRevision"]
                .as_str()
                .is_some_and(crate::proof::valid_id))
}
#[async_trait::async_trait]
impl DispatchTransport for RoutingTransport {
    fn can_submit(&self, job: &SendLease) -> bool {
        native_route(job, "stalwart") || self.provider(job).is_some()
    }
    async fn submit(
        &self,
        job: &SendLease,
        authorization: &Value,
        message: &DispatchMessage,
        prepared: &PreparedMessage,
    ) -> Receipt {
        if native_route(job, "stalwart") {
            let Ok(authorization) = serde_json::from_value(authorization.clone()) else {
                return Receipt::Unknown {};
            };
            return stalwart(job, authorization, prepared).await;
        }
        let Some(provider) = self.provider(job) else {
            return Receipt::Unknown {};
        };
        provider.submit(job, authorization, message, prepared).await
    }
}
impl ProviderTransport {
    pub fn new(channel: Channel, binding: ProviderBinding, issuer: VerifyingKey) -> Result<Self> {
        let slug = |value: &str| {
            !value.is_empty()
                && value.len() <= 64
                && value.as_bytes()[0].is_ascii_lowercase()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if !slug(&binding.plugin_id)
            || !slug(&binding.worker_id)
            || [binding.generation, binding.worker_generation]
                .iter()
                .any(|n| *n == 0 || *n > i64::MAX as u64)
            || !valid_digest(&binding.descriptor_sha256)
            || !valid_digest(&binding.worker_credential_sha256)
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            client: MailRuntimeClient::new(channel)
                .max_encoding_message_size(64 * 1024)
                .max_decoding_message_size(1024),
            binding,
            issuer,
        })
    }
    fn frames(
        &self,
        job: &SendLease,
        authorization: SendAuthorization,
        message: &DispatchMessage,
        prepared: &PreparedMessage,
    ) -> Result<Vec<SendFrame>> {
        let SendAuthorization::Direct {
            lease_token,
            plugin_id,
            generation,
            descriptor_sha256,
        } = authorization
        else {
            return Err(Error::Invalid);
        };
        if job.route["kind"] != "direct"
            || plugin_id != self.binding.plugin_id
            || generation != self.binding.generation
            || descriptor_sha256 != self.binding.descriptor_sha256
        {
            return Err(Error::Invalid);
        }
        let claims = verify_lease(
            &lease_token,
            &self.issuer,
            &self.binding.plugin_id,
            self.binding.generation,
            chrono::Utc::now().timestamp(),
        )
        .map_err(|_| Error::Invalid)?;
        let metadata = metadata_bytes(message)?;
        if claims.customer_id != job.customer_id
            || claims.job_id != job.id
            || claims.source_id != job.source_id
            || claims.descriptor_sha256 != self.binding.descriptor_sha256
            || claims.worker_id != self.binding.worker_id
            || claims.worker_generation != self.binding.worker_generation
            || claims.worker_credential_sha256 != self.binding.worker_credential_sha256
            || claims.metadata_sha256 != sha256(&metadata)
            || claims.mime_sha256 != sha256(&prepared.raw)
            || claims.mime_bytes != prepared.raw.len() as u64
            || job.route["pluginID"] != claims.plugin_id
            || job.route["generation"] != claims.generation
            || job.route["transportRevision"] != claims.transport_revision
            || job.route["grantRevision"] != claims.grant_revision
            || job.route["configurationRevision"] != claims.configuration_revision
            || job.route["descriptorSha256"] != claims.descriptor_sha256
            || job.route["bindingSha256"] != claims.binding_sha256
        {
            return Err(Error::Invalid);
        }
        let mut frames = vec![SendFrame {
            offset: 0,
            data: vec![],
            header: Some(SendHeader {
                lease_token,
                metadata_json: metadata,
            }),
        }];
        for (index, chunk) in prepared.raw.chunks(FRAME_BYTES).enumerate() {
            frames.push(SendFrame {
                offset: (index * FRAME_BYTES) as u64,
                data: chunk.to_vec(),
                header: None,
            });
        }
        Ok(frames)
    }
    async fn direct(
        &self,
        job: &SendLease,
        authorization: SendAuthorization,
        message: &DispatchMessage,
        prepared: &PreparedMessage,
    ) -> Receipt {
        let Ok(frames) = self.frames(job, authorization, message, prepared) else {
            return Receipt::Unknown {};
        };
        let mut request = tonic::Request::new(tokio_stream::iter(frames));
        request.set_timeout(Duration::from_secs(110));
        let response = tokio::time::timeout(
            Duration::from_secs(110),
            self.client.clone().submit(request),
        )
        .await;
        let Ok(Ok(response)) = response else {
            return Receipt::Unknown {};
        };
        let receipt = response.into_inner();
        match State::try_from(receipt.state) {
            Ok(State::Accepted)
                if !receipt.provider_id.is_empty()
                    && receipt.provider_id.len() <= 255
                    && receipt
                        .provider_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)) =>
            {
                Receipt::Accepted {
                    provider_id: receipt.provider_id,
                }
            }
            Ok(State::Rejected) if receipt.provider_id.is_empty() => Receipt::Rejected {},
            _ => Receipt::Unknown {},
        }
    }
}

async fn stalwart(
    job: &SendLease,
    authorization: SendAuthorization,
    prepared: &PreparedMessage,
) -> Receipt {
    let SendAuthorization::Stalwart {
        certificate_sha256,
        username,
        password,
        submission_id,
        proof,
    } = authorization
    else {
        return Receipt::Unknown {};
    };
    if job.route["kind"] != "stalwart" || submission_id != job.id {
        return Receipt::Unknown {};
    }
    let Ok(raw) = stamp_message(&prepared.raw, &submission_id, &proof) else {
        return Receipt::Unknown {};
    };
    let Ok(options) = SmtpOptions::assigned(
        &job.customer_id,
        certificate_sha256,
        username,
        password,
        Duration::from_secs(60),
    ) else {
        return Receipt::Unknown {};
    };
    match submit(&options, &prepared.from, &prepared.to, &raw).await {
        Ok(()) => Receipt::Accepted {
            provider_id: submission_id,
        },
        Err(error) if !error.uncertain => Receipt::Rejected {},
        Err(_) => Receipt::Unknown {},
    }
}

#[async_trait::async_trait]
impl DispatchTransport for ProviderTransport {
    fn can_submit(&self, job: &SendLease) -> bool {
        native_route(job, "direct")
            && job.route["pluginID"] == self.binding.plugin_id
            && job.route["generation"] == self.binding.generation
            && job.route["descriptorSha256"] == self.binding.descriptor_sha256
    }
    async fn submit(
        &self,
        job: &SendLease,
        authorization: &Value,
        message: &DispatchMessage,
        prepared: &PreparedMessage,
    ) -> Receipt {
        if !self.can_submit(job) {
            return Receipt::Unknown {};
        }
        let Ok(authorization) = serde_json::from_value(authorization.clone()) else {
            return Receipt::Unknown {};
        };
        match authorization {
            SendAuthorization::Direct { .. } => {
                self.direct(job, authorization, message, prepared).await
            }
            SendAuthorization::Stalwart { .. } => Receipt::Unknown {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{SendRequest, prepare_message};
    use crate::outbox::PayloadFormat;
    use ed25519_dalek::SigningKey;
    use serde_json::json;
    use std::{
        collections::HashSet,
        os::unix::fs::PermissionsExt,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use webdock_mail_protocol::{
        Denied, LeaseClaims,
        runtime::{Authority, Handler, PeerIdentity, Runtime, UnixSocket},
        wire::SendReceipt,
    };
    fn fixture() -> (
        SigningKey,
        ProviderBinding,
        SendLease,
        DispatchMessage,
        PreparedMessage,
    ) {
        let key = SigningKey::from_bytes(&[72; 32]);
        let binding = ProviderBinding {
            plugin_id: "relay-fixture".into(),
            generation: 1,
            descriptor_sha256: "a".repeat(64),
            worker_id: "mail-worker".into(),
            worker_generation: 1,
            worker_credential_sha256: "b".repeat(64),
        };
        let message = SendRequest::parse(
            json!({"from":"sender@example.test","to":"target@example.test","bcc":"hidden@example.test","subject":"Fixture","text":"body".repeat(FRAME_BYTES / 2)}),
            chrono::Utc::now(),
        )
        .unwrap()
        .compile()
        .unwrap();
        let prepared = prepare_message(
            &message.raw,
            &message.sender,
            &["target@example.test".into(), "hidden@example.test".into()],
        )
        .unwrap();
        let job = SendLease {
            id: "789".into(),
            customer_id: "12".into(),
            source_id: "key:34".into(),
            payload_ref: "unused".into(),
            payload_bytes: prepared.raw.len() as i32,
            request_hash: sha256(&prepared.raw),
            payload_format: PayloadFormat::Request,
            source_context: json!({}),
            route: json!({
                "kind":"direct","pluginID":binding.plugin_id,"generation":binding.generation,
                "transportRevision":1,"grantRevision":1,"configurationRevision":0,
                "descriptorSha256":binding.descriptor_sha256,"bindingSha256":"c".repeat(64)
            }),
            envelope: json!({"from":prepared.from,"to":prepared.to}),
            lease_token: uuid::Uuid::new_v4().to_string(),
        };
        (key, binding, job, message, prepared)
    }
    fn claims(
        binding: &ProviderBinding,
        job: &SendLease,
        message: &DispatchMessage,
        prepared: &PreparedMessage,
    ) -> LeaseClaims {
        let commitment = preparation(message, prepared).unwrap();
        LeaseClaims {
            v: 1,
            purpose: "mail-send".into(),
            audience: "webdock.mail.runtime.v1".into(),
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: binding.plugin_id.clone(),
            generation: binding.generation,
            customer_id: job.customer_id.clone(),
            job_id: job.id.clone(),
            source_id: job.source_id.clone(),
            transport_revision: 1,
            grant_revision: 1,
            configuration_revision: 0,
            descriptor_sha256: binding.descriptor_sha256.clone(),
            binding_sha256: "c".repeat(64),
            worker_id: binding.worker_id.clone(),
            worker_generation: binding.worker_generation,
            worker_credential_sha256: binding.worker_credential_sha256.clone(),
            metadata_sha256: commitment.metadata_sha256,
            mime_sha256: commitment.mime_sha256,
            mime_bytes: commitment.mime_bytes,
            iat: chrono::Utc::now().timestamp(),
            exp: chrono::Utc::now().timestamp() + 60,
        }
    }
    fn authorization(key: &SigningKey, claims: &LeaseClaims) -> SendAuthorization {
        SendAuthorization::Direct {
            lease_token: claims.sign(key).unwrap(),
            plugin_id: claims.plugin_id.clone(),
            generation: claims.generation,
            descriptor_sha256: claims.descriptor_sha256.clone(),
        }
    }
    #[tokio::test]
    async fn signed_binding_and_payload_changes_are_denied_before_streaming() {
        let (key, binding, job, message, prepared) = fixture();
        assert_eq!(
            metadata_bytes(&message).unwrap(),
            br#"{"sender":{"email":"sender@example.test"},"to":["target@example.test"],"cc":[],"bcc":["hidden@example.test"],"subject":"Fixture"}"#
        );
        let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        let transport =
            ProviderTransport::new(channel, binding.clone(), key.verifying_key()).unwrap();
        let original = claims(&binding, &job, &message, &prepared);
        for field in [
            "valid",
            "plugin",
            "generation",
            "customer",
            "job",
            "source",
            "descriptor",
            "worker",
            "worker-generation",
            "worker-credential",
            "transport-revision",
            "grant-revision",
            "configuration-revision",
            "binding",
            "metadata",
            "mime",
            "size",
        ] {
            let mut changed = original.clone();
            match field {
                "valid" => {}
                "plugin" => changed.plugin_id = "another-plugin".into(),
                "generation" => changed.generation += 1,
                "customer" => changed.customer_id = "13".into(),
                "job" => changed.job_id = "790".into(),
                "source" => changed.source_id = "key:35".into(),
                "descriptor" => changed.descriptor_sha256 = "d".repeat(64),
                "worker" => changed.worker_id = "another-worker".into(),
                "worker-generation" => changed.worker_generation += 1,
                "worker-credential" => changed.worker_credential_sha256 = "e".repeat(64),
                "transport-revision" => changed.transport_revision += 1,
                "grant-revision" => changed.grant_revision += 1,
                "configuration-revision" => changed.configuration_revision += 1,
                "binding" => changed.binding_sha256 = "f".repeat(64),
                "metadata" => changed.metadata_sha256 = "f".repeat(64),
                "mime" => changed.mime_sha256 = "f".repeat(64),
                "size" => changed.mime_bytes += 1,
                _ => unreachable!(),
            }
            let frames = transport.frames(&job, authorization(&key, &changed), &message, &prepared);
            assert_eq!(frames.is_ok(), field == "valid", "{field}");
        }
    }
    struct Live {
        attempts: Mutex<HashSet<String>>,
    }
    #[async_trait::async_trait]
    impl Authority for Live {
        async fn guard(
            &self,
            peer: &PeerIdentity,
            _claims: &LeaseClaims,
            _token: &str,
        ) -> webdock_mail_protocol::Result<()> {
            if matches!(peer, PeerIdentity::Unix { .. }) {
                Ok(())
            } else {
                Err(Denied)
            }
        }
        async fn begin_attempt(
            &self,
            _peer: &PeerIdentity,
            claims: &LeaseClaims,
            _token: &str,
        ) -> webdock_mail_protocol::Result<Value> {
            if self.attempts.lock().unwrap().insert(claims.id.clone()) {
                Ok(json!({"secret":"provider-only"}))
            } else {
                Err(Denied)
            }
        }
    }
    struct Sink(AtomicUsize);
    #[async_trait::async_trait]
    impl Handler for Sink {
        async fn submit(
            &self,
            _claims: &LeaseClaims,
            metadata: Value,
            mime: Vec<u8>,
            private: Value,
        ) -> SendReceipt {
            assert_eq!(metadata["bcc"][0], "hidden@example.test");
            assert!(mime.len() > FRAME_BYTES);
            assert!(!String::from_utf8_lossy(&mime).contains("hidden@example.test"));
            assert_eq!(private["secret"], "provider-only");
            self.0.fetch_add(1, Ordering::SeqCst);
            SendReceipt {
                state: State::Accepted as i32,
                provider_id: "receipt_1".into(),
            }
        }
    }
    #[tokio::test]
    async fn authenticated_stream_reaches_provider_once_and_denied_replay_is_unknown() {
        let (key, binding, job, message, prepared) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("mail.sock");
        let socket = UnixSocket::bind(&path).unwrap();
        let sink = Arc::new(Sink(AtomicUsize::new(0)));
        let runtime = Runtime::new(
            binding.plugin_id.clone(),
            "1.0.0".into(),
            binding.generation,
            binding.descriptor_sha256.clone(),
            key.verifying_key(),
            Arc::new(Live {
                attempts: Mutex::new(HashSet::new()),
            }),
            sink.clone(),
        )
        .unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(runtime.serve_unix(socket, async {
            let _ = stopped.await;
        }));
        let channel = webdock_plugin_runtime::unix_channel(&path).await.unwrap();
        let grant = claims(&binding, &job, &message, &prepared);
        let transport = ProviderTransport::new(channel, binding, key.verifying_key()).unwrap();
        let value = serde_json::to_value(authorization(&key, &grant)).unwrap();
        assert!(matches!(
            transport.submit(&job, &value, &message, &prepared).await,
            Receipt::Accepted { .. }
        ));
        assert!(matches!(
            transport.submit(&job, &value, &message, &prepared).await,
            Receipt::Unknown {}
        ));
        assert_eq!(sink.0.load(Ordering::SeqCst), 1);
        drop(transport);
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn routing_uses_independent_installed_channels_and_native_smtp_needs_no_plugin() {
        let (key, binding, job, message, prepared) = fixture();
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut providers = Vec::new();
        let mut jobs = Vec::new();
        let mut sinks = Vec::new();
        let mut servers = Vec::new();
        for (plugin, generation) in [("relay-fixture", 1), ("another-relay", 2)] {
            let mut binding = binding.clone();
            binding.plugin_id = plugin.into();
            binding.generation = generation;
            let mut job = job.clone();
            job.route["pluginID"] = serde_json::json!(plugin);
            job.route["generation"] = serde_json::json!(generation);
            let grant = claims(&binding, &job, &message, &prepared);
            let path = dir.path().join(format!("{plugin}.sock"));
            let socket = UnixSocket::bind(&path).unwrap();
            let sink = Arc::new(Sink(AtomicUsize::new(0)));
            let runtime = Runtime::new(
                plugin.into(),
                "1.0.0".into(),
                generation,
                binding.descriptor_sha256.clone(),
                key.verifying_key(),
                Arc::new(Live {
                    attempts: Mutex::new(HashSet::new()),
                }),
                sink.clone(),
            )
            .unwrap();
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(runtime.serve_unix(socket, async {
                let _ = stopped.await;
            }));
            let channel = webdock_plugin_runtime::unix_channel(&path).await.unwrap();
            providers.push(ProviderTransport::new(channel, binding, key.verifying_key()).unwrap());
            jobs.push((
                job,
                serde_json::to_value(authorization(&key, &grant)).unwrap(),
            ));
            sinks.push(sink);
            servers.push((stop, server));
        }
        let transport = RoutingTransport::new(providers).unwrap();
        for (index, (job, authorization)) in jobs.iter().enumerate() {
            assert!(transport.can_submit(job));
            assert!(matches!(
                transport
                    .submit(job, authorization, &message, &prepared)
                    .await,
                Receipt::Accepted { .. }
            ));
            assert_eq!(sinks[index].0.load(Ordering::SeqCst), 1);
        }
        let mut forged = jobs[0].1.clone();
        forged["pluginID"] = json!("another-relay");
        forged["generation"] = json!(2);
        assert!(matches!(
            transport
                .submit(&jobs[0].0, &forged, &message, &prepared)
                .await,
            Receipt::Unknown {}
        ));
        assert!(sinks.iter().all(|sink| sink.0.load(Ordering::SeqCst) == 1));
        drop(transport);
        for (stop, server) in servers {
            stop.send(()).unwrap();
            server.await.unwrap().unwrap();
        }
        let empty = RoutingTransport::new(vec![]).unwrap();
        let mut incoming = job;
        assert!(!empty.can_submit(&incoming));
        incoming.route["kind"] = json!("stalwart");
        incoming.route["serviceRevision"] = json!("1");
        assert!(empty.can_submit(&incoming));
        incoming.route.as_object_mut().unwrap().remove("generation");
        assert!(!empty.can_submit(&incoming));
    }

    #[tokio::test]
    async fn duplicate_and_excessive_operator_routes_are_rejected() {
        let (key, binding, _, _, _) = fixture();
        let provider = |binding: ProviderBinding| {
            ProviderTransport::new(
                tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy(),
                binding,
                key.verifying_key(),
            )
            .unwrap()
        };
        assert!(
            RoutingTransport::new(vec![provider(binding.clone()), provider(binding.clone())])
                .is_err()
        );
        let mut providers = Vec::new();
        for generation in 1..=(MAX_PROVIDERS as u64 + 1) {
            let mut binding = binding.clone();
            binding.generation = generation;
            providers.push(provider(binding));
        }
        assert!(RoutingTransport::new(providers).is_err());
    }
}
