//! Browser launch and confidential Mail gateway compatibility routes.
use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use webdock_core::features::OperationKind;
use webdock_mail::incoming::{MailboxCommand, NativeIncoming, ServiceCommand};
use webdock_mail::keys::{KeyChange, KeyCommand};
use webdock_mail::sending::{
    ProviderExecution, ProviderPolicy, ProviderSnapshot, SendCommand, Sending,
};
use webdock_mail::submissions::{QueueCommand, Submissions};
use webdock_mail::webmail::{FinishWrite, ReserveWrite, WebmailService};

/// Match the optional Core listener gate before declaring its required host service usable.
pub fn native_control_configured(
    configuration: Option<&crate::plugin_config::MailConfig>,
    environment: &std::collections::HashMap<String, String>,
) -> bool {
    configuration.is_some()
        && environment
            .get("WEBDOCK_NATIVE_MAIL_SENDING_ENABLED")
            .map(String::as_str)
            == Some("true")
        && webdock_core::features::FeatureSet::from_json(
            environment
                .get("WEBDOCK_FEATURES")
                .map(String::as_str)
                .unwrap_or("{}"),
        )
        .is_ok_and(|features| {
            features.state("mail") != webdock_core::features::FeatureState::Disabled
        })
}
/// Provider-neutral policy bridge. The domain keeps this transaction open until its state
/// transition commits; a separate momentary snapshot would lose the revocation fence.
pub struct MailProviderPolicy {
    host: Arc<webdock_plugin_host::HostService>,
}
impl MailProviderPolicy {
    pub fn new(host: Arc<webdock_plugin_host::HostService>) -> Self {
        Self { host }
    }
}
#[async_trait::async_trait]
impl ProviderPolicy for MailProviderPolicy {
    async fn action_source(
        &self,
        tx: &sea_orm::DatabaseTransaction,
        source: &Value,
    ) -> webdock_mail::Result<bool> {
        self.host
            .mail_source_in(tx, source)
            .await
            .map_err(|_| webdock_mail::Error::Unavailable)
    }
    async fn snapshot(
        &self,
        tx: &sea_orm::DatabaseTransaction,
        customer: &str,
        continuation: bool,
        peer: Option<&webdock_plugin_runtime::PeerIdentity>,
    ) -> webdock_mail::Result<ProviderSnapshot> {
        let policy = self
            .host
            .mail_policy_in(tx, customer, continuation)
            .await
            .map_err(|_| webdock_mail::Error::Unavailable)?;
        if peer.is_some_and(|peer| !policy.permits_peer(peer)) {
            return Err(webdock_mail::Error::Unavailable);
        }
        let execution = if policy.managed {
            let transport = &policy.transport;
            if transport["region"] != "eu"
                || !["managed", "byok"].contains(&transport["mode"].as_str().unwrap_or(""))
            {
                return Err(webdock_mail::Error::Unavailable);
            }
            ProviderExecution::Managed {
                credentials_sha256: transport["credentials_sha256"]
                    .as_str()
                    .filter(|v| {
                        v.len() == 64
                            && v.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                    .ok_or(webdock_mail::Error::Unavailable)?
                    .into(),
                transport: serde_json::json!({"customerID":policy.customer_id,"accountID":transport["provider_account_id"],"mode":transport["mode"],"region":"eu","transportRevision":policy.transport_revision,"bindingRevision":policy.grant_revision,"byokRevision":transport["byok_revision"]}),
                worker_credential_sha256: policy.binding.credential_sha256.clone(),
            }
        } else {
            let send_binding = policy
                .binding
                .customers
                .get(customer)
                .and_then(|bindings| bindings.get("send"))
                .ok_or(webdock_mail::Error::Unavailable)?
                .clone();
            let rule = send_binding
                .rules
                .iter()
                .find(|rule| rule.method == "POST")
                .ok_or(webdock_mail::Error::Unavailable)?;
            webdock_plugin_http::bind_target(&rule.method, &rule.path, send_binding.clone())
                .map_err(|_| webdock_mail::Error::Unavailable)?;
            ProviderExecution::LegacyHttp(send_binding)
        };
        Ok(ProviderSnapshot {
            customer_id: policy.customer_id,
            plugin_id: policy.plugin_id,
            generation: policy.generation,
            grant_revision: policy.grant_revision,
            transport_revision: policy.transport_revision,
            configuration_revision: policy.configuration_revision,
            descriptor_sha256: policy.manifest_sha256,
            binding_sha256: policy.binding_sha256,
            execution,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InitializationError {
    #[error("Missing Mail configuration: {0}")]
    Missing(&'static str),
    #[error("Invalid private Mail signing key")]
    SigningKey,
    #[error("Mail sender initialization failed")]
    Service,
    #[error("Incoming Mail administration must be explicitly enabled")]
    IncomingDisabled,
    #[error("Incoming Mail agent execution must be explicitly enabled")]
    IncomingAgentDisabled,
    #[error("Incoming Mail configuration requires explicitly enabled native mailboxes")]
    IncomingMailboxesDisabled,
    #[error("WEBDOCK_NATIVE_MAIL_CONFIGURATION_ENABLED must be true or false")]
    InvalidConfigurationFlag,
    #[error("Incoming Mail initialization failed")]
    IncomingService,
}
/// Configuration execution is independent of lifecycle execution and disabled by default.
/// Validate before opening pools so a requested but unavailable domain cannot be silently skipped.
pub fn incoming_configuration_enabled(
    environment: &std::collections::HashMap<String, String>,
) -> Result<bool, InitializationError> {
    match environment
        .get("WEBDOCK_NATIVE_MAIL_CONFIGURATION_ENABLED")
        .map(String::as_str)
    {
        None | Some("false") => return Ok(false),
        Some("true") => {}
        Some(_) => return Err(InitializationError::InvalidConfigurationFlag),
    }
    for (key, error) in [
        (
            "WEBDOCK_NATIVE_MAIL_ENABLED",
            InitializationError::IncomingDisabled,
        ),
        (
            "WEBDOCK_NATIVE_MAIL_AGENT_ENABLED",
            InitializationError::IncomingAgentDisabled,
        ),
        (
            "WEBDOCK_NATIVE_MAILBOXES_ENABLED",
            InitializationError::IncomingMailboxesDisabled,
        ),
    ] {
        if environment.get(key).map(String::as_str) != Some("true") {
            return Err(error);
        }
    }
    Ok(true)
}
fn incoming_value(
    environment: &std::collections::HashMap<String, String>,
    key: &'static str,
) -> Result<String, InitializationError> {
    environment
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or(InitializationError::Missing(key))
}
fn incoming_agent_configuration(
    environment: &std::collections::HashMap<String, String>,
) -> Result<webdock_mail::incoming::agent::MailAgentConfig, InitializationError> {
    if environment
        .get("WEBDOCK_NATIVE_MAIL_ENABLED")
        .map(String::as_str)
        != Some("true")
    {
        return Err(InitializationError::IncomingDisabled);
    }
    if environment
        .get("WEBDOCK_NATIVE_MAIL_AGENT_ENABLED")
        .map(String::as_str)
        != Some("true")
    {
        return Err(InitializationError::IncomingAgentDisabled);
    }
    Ok(webdock_mail::incoming::agent::MailAgentConfig {
        image: incoming_value(environment, "MAIL_STALWART_IMAGE")?,
        hostname_suffix: incoming_value(environment, "MAIL_HOSTNAME_SUFFIX")?,
        encryption_key: incoming_value(environment, "MAIL_ENCRYPTION_KEY")?,
    })
}
/// HTTP admission and the private agent listener must use the same reviewed tenant configuration.
pub async fn initialize_incoming(
    dependencies: webdock_mail::MailDependencies,
    environment: &std::collections::HashMap<String, String>,
) -> Result<Arc<NativeIncoming>, InitializationError> {
    if environment
        .get("WEBDOCK_NATIVE_MAIL_ENABLED")
        .map(String::as_str)
        != Some("true")
    {
        return Err(InitializationError::IncomingDisabled);
    }
    NativeIncoming::new(
        dependencies,
        webdock_mail::incoming::IncomingConfig {
            enabled: true,
            mailboxes_enabled: environment
                .get("WEBDOCK_NATIVE_MAILBOXES_ENABLED")
                .map(String::as_str)
                == Some("true"),
            sending_enabled: environment
                .get("WEBDOCK_NATIVE_MAIL_SENDING_ENABLED")
                .map(String::as_str)
                == Some("true"),
            managed_cluster: incoming_value(environment, "MAIL_MANAGED_CLUSTER_ID")?,
            rls_secret: incoming_value(environment, "DATABASE_RLS_SECRET")?,
        },
    )
    .await
    .map(Arc::new)
    .map_err(|_| InitializationError::IncomingService)
}
/// Core receives only provider-neutral desired state and retained encrypted Mail material.
pub async fn initialize_incoming_agent(
    dependencies: webdock_mail::MailDependencies,
    environment: &std::collections::HashMap<String, String>,
) -> Result<Arc<webdock_mail::incoming::agent::IncomingAgent>, InitializationError> {
    let configuration_enabled = incoming_configuration_enabled(environment)?;
    let configuration = incoming_agent_configuration(environment)?;
    let incoming = initialize_incoming(dependencies, environment).await?;
    webdock_mail::incoming::agent::IncomingAgent::new(incoming, configuration)
        .map(|agent| agent.with_configuration_enabled(configuration_enabled))
        .map(Arc::new)
        .map_err(|_| InitializationError::IncomingService)
}
/// The HTTP process and private callback listener use the same issuer/worker settings.
pub async fn initialize_sender(
    dependencies: webdock_mail::MailDependencies,
    host: Arc<webdock_plugin_host::HostService>,
    configuration: &crate::plugin_config::MailConfig,
    environment: &std::collections::HashMap<String, String>,
    webmail: Option<Arc<WebmailService>>,
) -> Result<Arc<Sending>, InitializationError> {
    let required = |key: &'static str| {
        environment
            .get(key)
            .cloned()
            .ok_or(InitializationError::Missing(key))
    };
    let worker_secret = required("MAIL_SEND_WORKER_SECRET")?;
    GatewayCredentials::hash(&worker_secret).map_err(|_| InitializationError::Service)?;
    let incoming = if environment
        .get("WEBDOCK_NATIVE_MAILBOXES_ENABLED")
        .is_some_and(|value| value == "true")
    {
        Some(webdock_mail::sending::IncomingConfig {
            managed_cluster: required("MAIL_MANAGED_CLUSTER_ID")?,
            hostname_suffix: required("MAIL_HOSTNAME_SUFFIX")?,
            encryption_key: required("MAIL_ENCRYPTION_KEY")?,
        })
    } else {
        None
    };
    let signing_key = crate::plugin_config::signing_key(&configuration.signing_key)
        .map_err(|_| InitializationError::SigningKey)?;
    let mut sender = Sending::new(
        dependencies,
        webdock_mail::sending::SendingConfig {
            enabled: true,
            rls_secret: required("DATABASE_RLS_SECRET")?,
            signing_key: signing_key.to_bytes(),
            worker_id: configuration.worker_id.clone(),
            worker_generation: configuration.worker_generation,
            worker_credential_sha256: hex::encode(Sha256::digest(worker_secret.as_bytes())),
            incoming,
        },
        Arc::new(MailProviderPolicy::new(host)),
    )
    .await
    .map_err(|_| InitializationError::Service)?;
    if let Some(webmail) = webmail {
        sender = sender.with_webmail(webmail);
    }
    Ok(Arc::new(sender))
}

#[cfg(test)]
mod tests {
    #[test]
    fn configured_transport_adoption_accepts_only_a_customer_selection() {
        assert!(
            serde_json::from_value::<super::Command>(
                serde_json::json!({"action":"adoptConfiguredNativeSending","customerID":"123"})
            )
            .is_ok()
        );
        for field in [
            "credentials",
            "provider",
            "pluginID",
            "region",
            "revision",
            "operator",
        ] {
            let mut body =
                serde_json::json!({"action":"adoptConfiguredNativeSending","customerID":"123"});
            body[field] = serde_json::json!("private-poison");
            assert!(serde_json::from_value::<super::Command>(body).is_err());
        }
    }

    use super::*;
    #[test]
    fn incoming_configuration_execution_is_a_separate_explicit_opt_in() {
        let mut environment = std::collections::HashMap::new();
        assert!(!incoming_configuration_enabled(&environment).unwrap());
        for key in [
            "WEBDOCK_NATIVE_MAIL_ENABLED",
            "WEBDOCK_NATIVE_MAIL_AGENT_ENABLED",
            "WEBDOCK_NATIVE_MAILBOXES_ENABLED",
        ] {
            environment.insert(key.to_owned(), "true".to_owned());
        }
        assert!(!incoming_configuration_enabled(&environment).unwrap());
        environment.insert(
            "WEBDOCK_NATIVE_MAIL_CONFIGURATION_ENABLED".to_owned(),
            "false".to_owned(),
        );
        assert!(!incoming_configuration_enabled(&environment).unwrap());
        environment.insert(
            "WEBDOCK_NATIVE_MAIL_CONFIGURATION_ENABLED".to_owned(),
            "true".to_owned(),
        );
        assert!(incoming_configuration_enabled(&environment).unwrap());
        for key in [
            "WEBDOCK_NATIVE_MAIL_ENABLED",
            "WEBDOCK_NATIVE_MAIL_AGENT_ENABLED",
            "WEBDOCK_NATIVE_MAILBOXES_ENABLED",
        ] {
            let mut incomplete = environment.clone();
            incomplete.remove(key);
            assert!(
                incoming_configuration_enabled(&incomplete).is_err(),
                "{key}"
            );
            incomplete.insert(key.to_owned(), "false".to_owned());
            assert!(
                incoming_configuration_enabled(&incomplete).is_err(),
                "{key}"
            );
        }
    }
    #[test]
    fn incoming_configuration_rejects_malformed_opt_in_without_echoing_values() {
        for value in ["TRUE", "1", "", "private-misconfigured-value"] {
            let environment = std::collections::HashMap::from([(
                "WEBDOCK_NATIVE_MAIL_CONFIGURATION_ENABLED".to_owned(),
                value.to_owned(),
            )]);
            let error = incoming_configuration_enabled(&environment).unwrap_err();
            assert!(!error.to_string().contains(value) || value.is_empty());
        }
    }
    #[test]
    fn incoming_executor_configuration_requires_both_gates_and_retained_names() {
        use std::collections::HashMap;
        let reviewed = HashMap::from([
            ("WEBDOCK_NATIVE_MAIL_ENABLED".into(), "true".into()),
            ("WEBDOCK_NATIVE_MAIL_AGENT_ENABLED".into(), "true".into()),
            (
                "MAIL_STALWART_IMAGE".into(),
                "registry.example.test/mail@sha256:reviewed-by-domain".into(),
            ),
            ("MAIL_HOSTNAME_SUFFIX".into(), "mail.example.test".into()),
            ("MAIL_ENCRYPTION_KEY".into(), "private-retained-key".into()),
        ]);
        assert!(incoming_agent_configuration(&reviewed).is_ok());
        for missing in [
            "WEBDOCK_NATIVE_MAIL_ENABLED",
            "WEBDOCK_NATIVE_MAIL_AGENT_ENABLED",
            "MAIL_STALWART_IMAGE",
            "MAIL_HOSTNAME_SUFFIX",
            "MAIL_ENCRYPTION_KEY",
        ] {
            let mut input = reviewed.clone();
            input.remove(missing);
            input.insert("MAIL_IMAGE".into(), "unreviewed-alias".into());
            let error = incoming_agent_configuration(&input)
                .err()
                .expect("Must fail closed");
            assert!(!error.to_string().contains("private-retained-key"));
        }
    }
    #[test]
    fn draining_keeps_revocation_and_suspension_available() {
        let features =
            webdock_core::features::FeatureSet::from_json(r#"{"mail":"draining"}"#).unwrap();
        for value in [
            json!({"action":"manageNativeSending","customerID":"123",
                "input":{"action":"revoke","keyID":"456"}}),
            json!({"action":"manageTenantMailService","customerID":"123",
                "input":{"action":"suspend","revision":"1"}}),
            json!({"action":"manageTenantMailboxes","customerID":"123",
                "input":{"action":"revoke","mailboxID":"456","subject":"user"}}),
        ] {
            let command: Command = serde_json::from_value(value.clone()).unwrap();
            assert!(
                features.require("mail", command.operation()).is_ok(),
                "{value}"
            );
        }
        for value in [
            json!({"action":"manageNativeSending","customerID":"123",
                "input":{"action":"create","label":"Notifications"}}),
            json!({"action":"manageTenantMailService","customerID":"123",
                "input":{"action":"activate","revision":"1"}}),
            json!({"action":"manageTenantMailboxes","customerID":"123",
                "input":{"action":"grant","mailboxID":"456","subject":"user"}}),
        ] {
            let command: Command = serde_json::from_value(value).unwrap();
            assert!(features.require("mail", command.operation()).is_err());
        }
    }
    #[test]
    fn incoming_commands_preserve_typed_fields_and_reject_overrides() {
        for value in [
            json!({"action":"getTenantMailService","customerID":"123"}),
            json!({"action":"getTenantMailboxes","customerID":"123"}),
            json!({"action":"manageTenantMailService","customerID":"123",
                "input":{"action":"activate","revision":"0"}}),
            json!({"action":"manageTenantMailboxes","customerID":"123",
                "input":{"action":"mailbox.create","domainID":"456",
                    "localPart":"team","name":"Team","quotaBytes":1048576}}),
        ] {
            assert!(
                serde_json::from_value::<Command>(value.clone()).is_ok(),
                "{value}"
            );
            let mut overridden = value;
            overridden["operator"] = json!(true);
            assert!(serde_json::from_value::<Command>(overridden).is_err());
        }
        for input in [
            json!({"action":"activate","revision":0}),
            json!({"action":"suspend","revision":"1","force":true}),
            json!({"action":"delete","revision":"1"}),
        ] {
            assert!(
                serde_json::from_value::<Command>(json!({
                    "action":"manageTenantMailService","customerID":"123","input":input
                }))
                .is_err()
            );
        }
    }
    #[test]
    fn queue_commands_require_worker_authority_and_preserve_strict_fields() {
        assert!(queue_command(Caller::WebmailGateway, &json!({"action":"send.claim"})).is_err());
        assert!(matches!(
            queue_command(Caller::SendingWorker, &json!({"action":"send.claim"})),
            Ok(Some(WorkerCommand::Queue(command))) if matches!(*command,QueueCommand::Claim{})
        ));
        assert!(
            queue_command(
                Caller::SendingWorker,
                &json!({"action":"send.claim","customerID":"123"})
            )
            .is_err()
        );
        assert!(queue_command(Caller::SendingWorker, &json!({"action":"send.unknown"})).is_err());
        assert!(matches!(
            queue_command(
                Caller::SendingWorker,
                &json!({"action":"send.authenticate","token":"x"})
            ),
            Ok(None)
        ));
    }
    #[test]
    fn provider_and_queued_webmail_commands_require_worker_credentials() {
        for value in [
            json!({"action":"send.preflight","token":"test","envelope":{"from":"a@example.test","to":["b@example.test"]}}),
            json!({"action":"send.policy.auth","customerID":"123","token":"test"}),
            json!({"action":"send.webmail.status","token":"test","id":"456"}),
            json!({"action":"send.webmail.lookup","token":"test","emailID":"draft"}),
            json!({"action":"send.webmail.filingAccess","token":"test","id":"456"}),
        ] {
            let denied = queue_command(Caller::WebmailGateway, &value)
                .err()
                .expect("Browser gateway credentials cannot perform worker commands");
            assert_eq!(denied.into_response().status(), StatusCode::FORBIDDEN);
            assert!(matches!(
                queue_command(Caller::SendingWorker, &value),
                Ok(Some(_))
            ));

            let mut altered = value;
            altered["authorityOverride"] = json!("operator");
            let denied = queue_command(Caller::SendingWorker, &altered)
                .err()
                .expect("Unknown authorization fields must not be ignored");
            assert_eq!(denied.into_response().status(), StatusCode::BAD_REQUEST);
        }
    }
    #[test]
    fn native_key_commands_are_typed_at_the_http_boundary() {
        let create = json!({
            "action":"manageNativeSending","customerID":"123",
            "input":{"action":"create","label":"Build notifications","domainID":"456"}
        });
        assert!(matches!(
            serde_json::from_value::<Command>(create.clone()).unwrap(),
            Command::ManageKeys {
                input: KeyCommand::Create { .. },
                ..
            }
        ));
        let mut invalid = create;
        invalid["input"]["domainID"] = Value::Null;
        assert!(serde_json::from_value::<Command>(invalid).is_err());
    }
    #[test]
    fn gateway_and_worker_credentials_are_distinct_and_reject_duplicates() {
        let browser = "b".repeat(32);
        let worker = "w".repeat(32);
        assert!(GatewayCredentials::new(Some(&browser), Some(&browser)).is_err());
        let credentials = GatewayCredentials::new(Some(&browser), Some(&worker)).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {browser}").parse().unwrap(),
        );
        assert_eq!(credentials.identify(&headers), Some(Caller::WebmailGateway));
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {worker}").parse().unwrap(),
        );
        assert_eq!(credentials.identify(&headers), Some(Caller::SendingWorker));
        headers.append(
            header::AUTHORIZATION,
            format!("Bearer {browser}").parse().unwrap(),
        );
        assert_eq!(credentials.identify(&headers), None);
        let credentials = GatewayCredentials::new(None, Some(&worker)).unwrap();
        headers.clear();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {browser}").parse().unwrap(),
        );
        assert_eq!(credentials.identify(&headers), None);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caller {
    WebmailGateway,
    SendingWorker,
}
struct GatewayCredentials {
    browser: Option<[u8; 32]>,
    worker: Option<[u8; 32]>,
}
impl GatewayCredentials {
    fn hash(secret: &str) -> Result<[u8; 32], &'static str> {
        if !(32..=512).contains(&secret.len())
            || !secret.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            return Err("Invalid Mail gateway secret");
        }
        Ok(Sha256::digest(secret.as_bytes()).into())
    }
    fn new(browser: Option<&str>, worker: Option<&str>) -> Result<Self, &'static str> {
        let browser = browser.map(Self::hash).transpose()?;
        let worker = worker.map(Self::hash).transpose()?;
        if browser.is_none() && worker.is_none() || browser.is_some() && browser == worker {
            return Err("Mail gateway and worker credentials must be separate");
        }
        Ok(Self { browser, worker })
    }
    fn identify(&self, headers: &HeaderMap) -> Option<Caller> {
        if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
            return None;
        }
        let value = headers
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        let hash = Self::hash(value).ok()?;
        if self
            .browser
            .as_ref()
            .is_some_and(|h| bool::from(h.ct_eq(&hash)))
        {
            return Some(Caller::WebmailGateway);
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|h| bool::from(h.ct_eq(&hash)))
        {
            return Some(Caller::SendingWorker);
        }
        None
    }
}
pub struct MailEndpoints {
    pub service: Option<Arc<WebmailService>>,
    pub incoming: Option<Arc<NativeIncoming>>,
    pub keys: Option<Arc<webdock_mail::keys::NativeKeys>>,
    pub queue: Option<Arc<Submissions>>,
    pub sender: Option<Arc<Sending>>,
    credentials: GatewayCredentials,
    operator_transports:
        std::collections::BTreeMap<String, webdock_mail::transport_adoption::ConfiguredTransport>,
}
impl MailEndpoints {
    pub fn new(service: Arc<WebmailService>, secret: &str) -> Result<Self, &'static str> {
        Ok(Self {
            service: Some(service),
            incoming: None,
            keys: None,
            queue: None,
            sender: None,
            operator_transports: Default::default(),
            credentials: GatewayCredentials::new(Some(secret), None)?,
        })
    }
    pub fn sending_only(
        keys: Arc<webdock_mail::keys::NativeKeys>,
        worker_secret: &str,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            service: None,
            incoming: None,
            keys: Some(keys),
            queue: None,
            sender: None,
            operator_transports: Default::default(),
            credentials: GatewayCredentials::new(None, Some(worker_secret))?,
        })
    }
    pub fn with_sending(
        mut self,
        keys: Arc<webdock_mail::keys::NativeKeys>,
        worker_secret: &str,
    ) -> Result<Self, &'static str> {
        let hash = GatewayCredentials::hash(worker_secret)?;
        if self.credentials.browser == Some(hash) {
            return Err("Mail gateway and worker credentials must be separate");
        }
        self.keys = Some(keys);
        self.credentials.worker = Some(hash);
        Ok(self)
    }
    pub fn with_queue(mut self, queue: Arc<Submissions>) -> Self {
        self.queue = Some(queue);
        self
    }
    pub fn with_operator_transports(
        mut self,
        accounts: Vec<webdock_mail::transport_adoption::ConfiguredTransport>,
    ) -> Result<Self, &'static str> {
        for account in accounts {
            if self
                .operator_transports
                .insert(account.customer().to_owned(), account)
                .is_some()
            {
                return Err("Duplicate operator transport");
            }
        }
        Ok(self)
    }
    pub fn with_sender(mut self, sender: Arc<Sending>) -> Self {
        self.sender = Some(sender);
        self
    }
    /// Incoming administration has no machine gateway until one is explicitly configured.
    pub fn incoming_only(incoming: Arc<NativeIncoming>) -> Self {
        Self {
            service: None,
            incoming: Some(incoming),
            keys: None,
            queue: None,
            sender: None,
            operator_transports: Default::default(),
            credentials: GatewayCredentials {
                browser: None,
                worker: None,
            },
        }
    }
    pub fn with_incoming(mut self, incoming: Arc<NativeIncoming>) -> Self {
        self.incoming = Some(incoming);
        self
    }
    fn incoming(&self) -> Result<&NativeIncoming, ApiFailure> {
        self.incoming.as_deref().ok_or_else(unavailable)
    }
    fn webmail(&self) -> Result<&WebmailService, ApiFailure> {
        self.service.as_deref().ok_or_else(unavailable)
    }
    pub(crate) fn gateway_origin(&self) -> Option<&str> {
        self.service
            .as_deref()
            .and_then(WebmailService::gateway_origin)
    }
    fn sending(&self) -> Result<&webdock_mail::keys::NativeKeys, ApiFailure> {
        self.keys.as_deref().ok_or_else(unavailable)
    }
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/console/mail", post(command))
        .route("/v1/mail", post(command))
        .route("/api/mail/gateway", post(gateway))
}
fn unavailable() -> ApiFailure {
    reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Mail service is unavailable.",
    )
}
fn invalid() -> ApiFailure {
    reject(
        StatusCode::BAD_REQUEST,
        "invalid_input",
        "Check the Mail request fields.",
    )
}
fn failure(error: webdock_mail::Error) -> ApiFailure {
    match error {
        webdock_mail::Error::Invalid => invalid(),
        _ => reject(
            StatusCode::FORBIDDEN,
            "mail_unavailable",
            "Mail access is unavailable.",
        ),
    }
}
fn incoming_failure(error: webdock_mail::incoming::Error) -> ApiFailure {
    match error {
        webdock_mail::incoming::Error::Invalid => invalid(),
        webdock_mail::incoming::Error::Unavailable => reject(
            StatusCode::FORBIDDEN,
            "mail_unavailable",
            "Mail access is unavailable.",
        ),
        webdock_mail::incoming::Error::Conflict => reject(
            StatusCode::CONFLICT,
            "conflict",
            "Mail changed. Reload its current state.",
        ),
    }
}
fn input<T: serde::de::DeserializeOwned>(
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<T, ApiFailure> {
    if body.len() > 65536 {
        return Err(reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_input",
            "Mail request is too large.",
        ));
    }
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            != Some("application/json")
    {
        return Err(reject(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_input",
            "JSON is required.",
        ));
    }
    serde_json::from_slice(body).map_err(|_| invalid())
}
#[derive(Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
enum Command {
    #[serde(rename = "adoptConfiguredNativeSending")]
    AdoptTransport {
        #[serde(rename = "customerID")]
        customer_id: String,
    },
    #[serde(rename = "getTenantMailService")]
    Service {
        #[serde(rename = "customerID")]
        customer_id: String,
    },
    #[serde(rename = "getTenantMailboxes")]
    Mailboxes {
        #[serde(rename = "customerID")]
        customer_id: String,
    },
    #[serde(rename = "manageTenantMailService")]
    ManageService {
        #[serde(rename = "customerID")]
        customer_id: String,
        input: ServiceCommand,
    },
    #[serde(rename = "manageTenantMailboxes")]
    ManageMailboxes {
        #[serde(rename = "customerID")]
        customer_id: String,
        input: MailboxCommand,
    },
    #[serde(rename = "manageNativeSending")]
    ManageKeys {
        #[serde(rename = "customerID")]
        customer_id: String,
        input: KeyCommand,
    },

    #[serde(rename = "launchTenantWebmail")]
    Launch {
        #[serde(rename = "customerID")]
        customer_id: String,
        #[serde(rename = "mailboxID")]
        mailbox_id: String,
    },
}
impl Command {
    fn operation(&self) -> OperationKind {
        match self {
            Self::AdoptTransport { .. } => OperationKind::Provision,
            Self::Service { .. } | Self::Mailboxes { .. } | Self::Launch { .. } => {
                OperationKind::Read
            }
            Self::ManageService {
                input: ServiceCommand::Suspend { .. } | ServiceCommand::Reconcile { .. },
                ..
            }
            | Self::ManageMailboxes {
                input: MailboxCommand::Revoke { .. },
                ..
            }
            | Self::ManageKeys {
                input: KeyCommand::Revoke { .. },
                ..
            } => OperationKind::ContinueAccepted,
            Self::ManageKeys { .. } | Self::ManageService { .. } | Self::ManageMailboxes { .. } => {
                OperationKind::Provision
            }
        }
    }
}
#[derive(Serialize)]
struct Data<T> {
    data: T,
}
fn data<T: Serialize>(value: T) -> Response {
    Json(Data { data: value }).into_response()
}
#[derive(Serialize)]
struct KeyReply {
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}
async fn command(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let command: Command = input(&headers, &body)?;
        state
            .config
            .features
            .require("mail", command.operation())
            .map_err(|_| unavailable())?;
        let actor = auth::resolve_actor(&state, &headers)
            .await?
            .ok_or_else(|| {
                reject(
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    "Sign in to continue.",
                )
            })?;
        let endpoint = state.mail.as_ref().ok_or_else(unavailable)?;
        let response = match command {
            Command::AdoptTransport { customer_id } => {
                if !actor.principal.is_operator() {
                    return Err(reject(
                        StatusCode::FORBIDDEN,
                        "mail_unavailable",
                        "Mail access is unavailable.",
                    ));
                }
                let configured = endpoint
                    .operator_transports
                    .get(&customer_id)
                    .ok_or_else(unavailable)?;
                endpoint
                    .sending()?
                    .adopt_configured_transport(&actor.principal, &actor.authority, configured)
                    .await
                    .map_err(failure)?;
                data(serde_json::json!({"ready":true}))
            }

            Command::Service { customer_id } => data(
                endpoint
                    .incoming()?
                    .service(&actor.principal, &actor.authority, &customer_id)
                    .await
                    .map_err(incoming_failure)?,
            ),
            Command::Mailboxes { customer_id } => data(
                endpoint
                    .incoming()?
                    .directory(&actor.principal, &actor.authority, &customer_id)
                    .await
                    .map_err(incoming_failure)?,
            ),
            Command::ManageService { customer_id, input } => data(
                endpoint
                    .incoming()?
                    .manage_service(&actor.principal, &actor.authority, &customer_id, input)
                    .await
                    .map_err(incoming_failure)?,
            ),
            Command::ManageMailboxes { customer_id, input } => data(
                endpoint
                    .incoming()?
                    .manage_mailboxes(&actor.principal, &actor.authority, &customer_id, input)
                    .await
                    .map_err(incoming_failure)?,
            ),
            Command::ManageKeys { customer_id, input } => {
                let change = endpoint
                    .sending()?
                    .manage(&actor.principal, &actor.authority, &customer_id, input)
                    .await
                    .map_err(failure)?;
                data(match change {
                    KeyChange::Created { token, .. } => KeyReply {
                        message: "Save this Spitzli Cloud API key now. It is shown only once.",
                        token: Some(token),
                    },
                    KeyChange::Revoked { .. } => KeyReply {
                        message: "Spitzli Cloud API key revoked.",
                        token: None,
                    },
                })
            }

            Command::Launch {
                customer_id,
                mailbox_id,
            } => data(
                endpoint
                    .webmail()?
                    .launch(
                        &actor.principal,
                        &actor.authority,
                        &customer_id,
                        &mailbox_id,
                    )
                    .await
                    .map_err(failure)?,
            ),
        };
        Ok(response)
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
enum GatewayCommand {
    #[serde(rename = "send.authenticate")]
    Authenticate {
        token: String,
    },

    Exchange {
        code: String,
    },
    Inspect {
        token: String,
    },
    Logout {
        token: String,
    },
    Reserve {
        write: ReserveWrite,
    },
    Finish {
        write: FinishWrite,
    },
}
#[derive(Deserialize)]
#[serde(untagged)]
enum WorkerCommand {
    Queue(Box<QueueCommand>),
    Send(Box<SendCommand>),
    Webmail(Box<webdock_mail::queued_webmail::WebmailCommand>),
}
fn queue_command(caller: Caller, value: &Value) -> Result<Option<WorkerCommand>, ApiFailure> {
    if value["action"]
        .as_str()
        .is_some_and(|action| action.starts_with("send.") && action != "send.authenticate")
    {
        if caller != Caller::SendingWorker {
            return Err(reject(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Mail control scope is unavailable.",
            ));
        }
        return serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| invalid());
    }
    Ok(None)
}
async fn gateway(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let endpoint = state.mail.as_ref().ok_or_else(unavailable)?;
        let caller = endpoint.credentials.identify(&headers).ok_or_else(|| {
            reject(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Gateway authentication required.",
            )
        })?;
        let value: Value = input(&headers, &body)?;
        if let Some(command) = queue_command(caller, &value)? {
            state
                .config
                .features
                .require("mail", OperationKind::ContinueAccepted)
                .map_err(|_| unavailable())?;
            let data = match command {
                WorkerCommand::Queue(command) => {
                    endpoint
                        .queue
                        .as_deref()
                        .ok_or_else(unavailable)?
                        .execute(*command)
                        .await
                }
                WorkerCommand::Send(command) => {
                    endpoint
                        .sender
                        .as_deref()
                        .ok_or_else(unavailable)?
                        .execute(*command)
                        .await
                }
                WorkerCommand::Webmail(command) => {
                    endpoint
                        .sender
                        .as_deref()
                        .ok_or_else(unavailable)?
                        .execute_webmail(*command)
                        .await
                }
            }
            .map_err(failure)?;
            return Ok(Json(json!({"data":data})).into_response());
        }
        let command: GatewayCommand = serde_json::from_value(value).map_err(|_| invalid())?;
        let permitted = match command {
            GatewayCommand::Authenticate { .. } => caller == Caller::SendingWorker,
            GatewayCommand::Inspect { .. } => true,
            _ => caller == Caller::WebmailGateway,
        };
        if !permitted {
            return Err(reject(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Mail control scope is unavailable.",
            ));
        }
        if !matches!(command, GatewayCommand::Logout { .. }) {
            state
                .config
                .features
                .require(
                    "mail",
                    if matches!(command, GatewayCommand::Reserve { .. }) {
                        OperationKind::Provision
                    } else {
                        OperationKind::ContinueAccepted
                    },
                )
                .map_err(|_| unavailable())?;
        }
        let data: Value = match command {
            GatewayCommand::Authenticate { token } => json!(
                endpoint
                    .sending()?
                    .authenticate(&token, None)
                    .await
                    .map_err(failure)?
            ),
            GatewayCommand::Exchange { code } => {
                json!(endpoint.webmail()?.exchange(&code).await.map_err(failure)?)
            }
            GatewayCommand::Inspect { token } => {
                json!(endpoint.webmail()?.inspect(&token).await.map_err(failure)?)
            }
            GatewayCommand::Logout { token } => {
                endpoint.webmail()?.revoke(&token).await.map_err(failure)?;
                json!({"ok":true})
            }
            GatewayCommand::Reserve { write } => json!(
                endpoint
                    .webmail()?
                    .reserve_write(write)
                    .await
                    .map_err(failure)?
            ),
            GatewayCommand::Finish { write } => {
                endpoint
                    .webmail()?
                    .finish_write(write)
                    .await
                    .map_err(failure)?;
                json!({"ok":true})
            }
        };
        Ok(Json(json!({"data":data})).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
