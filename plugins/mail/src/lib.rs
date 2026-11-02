//! Mail as a plugin: sending domains, delivery through the platform or a project's own
//! turboSMTP account, and a send action exposed to the Console, the public API and MCP.
//! The Core admits every call; this plugin checks again that records belong to the invoking project.
pub use webdock_plugin_kit::seal;
pub mod delivery;
pub mod engine;
pub mod executor;
pub mod listener;
mod mailboxes;
pub mod payload;
pub mod recovery;
pub mod store;
pub mod transport;

use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::ActionHandler;

type Result<T> = std::result::Result<T, PluginError>;
pub const PLUGIN_ID: &str = "mail";
const ACTIONS: &[(&str, &str)] = &[
    ("domains.create", "write"),
    ("domains.view", "read"),
    ("domains.verify", "write"),
    ("domains.delete", "write"),
    ("mail.send", "write"),
    ("mail.status", "read"),
    ("mail.cancel", "write"),
    ("mail.history", "read"),
    ("relay.connect", "write"),
    ("relay.view", "read"),
    ("relay.delete", "write"),
    ("mailboxes.create", "write"),
    ("mailboxes.view", "read"),
    ("mailboxes.credentials", "write"),
    ("mailboxes.rotate", "write"),
    ("mailboxes.delete", "write"),
    ("incoming.activate", "write"),
    ("incoming.deactivate", "write"),
];
fn invalid(message: &str) -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, message)
}
fn missing() -> PluginError {
    PluginError::new(ErrorCode::NotFound, "Not found")
}
fn unavailable(context: &str, error: impl std::fmt::Display) -> PluginError {
    let _ = error;
    eprintln!("mail_plugin_{context}_unavailable");
    PluginError::new(ErrorCode::Unavailable, "Mail service is unavailable")
}
fn parse<T: for<'a> Deserialize<'a>>(input: Value) -> Result<T> {
    serde_json::from_value(input).map_err(|_| invalid("Invalid mail request"))
}
fn store_error(error: store::Error) -> PluginError {
    unavailable("store", error)
}
/// A lowercase DNS name with at least two labels; no IP literals, no trailing dot.
pub fn domain_name(raw: &str) -> Result<String> {
    let name = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let labels = name.split('.').collect::<Vec<_>>();
    if name.len() > 253
        || labels.len() < 2
        || labels.iter().any(|l| {
            l.is_empty()
                || l.len() > 63
                || l.starts_with('-')
                || l.ends_with('-')
                || !l
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        || labels
            .last()
            .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid("Enter a domain such as example.com"));
    }
    Ok(name)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewDomain {
    domain: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    resource_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewRelay {
    consumer_key: String,
    consumer_secret: String,
}
fn uuid(raw: &str) -> Result<Uuid> {
    Uuid::parse_str(raw).map_err(|_| invalid("Invalid resource"))
}

/// Looks up `_webdock.<domain>` TXT records; tests substitute their own answers.
#[tonic::async_trait]
pub trait Dns: Send + Sync + 'static {
    async fn txt(&self, name: &str) -> Vec<String>;
}
pub struct SystemDns(hickory_resolver::TokioResolver);
impl SystemDns {
    pub fn new() -> std::result::Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self(hickory_resolver::Resolver::builder_tokio()?.build()))
    }
}
#[tonic::async_trait]
impl Dns for SystemDns {
    async fn txt(&self, name: &str) -> Vec<String> {
        self.0.txt_lookup(format!("{name}.")).await.map_or_else(
            |_| Vec::new(),
            |found| {
                found
                    .iter()
                    .map(|txt| {
                        txt.txt_data()
                            .iter()
                            .map(|part| String::from_utf8_lossy(part))
                            .collect()
                    })
                    .collect()
            },
        )
    }
}

pub struct Settings {
    pub platform: transport::Platform,
    /// Local dev only: treat every domain as verified, because `.test` names cannot carry DNS.
    pub trust_unverified_domains: bool,
}
pub struct Plugin {
    store: Arc<store::Store>,
    sealer: seal::Sealer,
    delivery: Option<Arc<delivery::Delivery>>,
    dns: Box<dyn Dns>,
    settings: Settings,
    engines: Option<engine::Engines>,
    /// ponytail: one mailbox operation at a time across projects; per-project locks if it matters.
    provisioning: tokio::sync::Mutex<()>,
}
impl Plugin {
    pub fn new(
        store: store::Store,
        sealer: seal::Sealer,
        _sender: transport::Sender,
        dns: Box<dyn Dns>,
        settings: Settings,
        engines: Option<engine::Engines>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: Arc::new(store),
            sealer,
            delivery: None,
            dns,
            settings,
            engines,
            provisioning: tokio::sync::Mutex::new(()),
        })
    }
    pub fn with_delivery(self: Arc<Self>, delivery: Arc<delivery::Delivery>) -> Result<Arc<Self>> {
        let mut plugin =
            Arc::try_unwrap(self).map_err(|_| invalid("Mail configuration already shared"))?;
        plugin.delivery = Some(delivery);
        Ok(Arc::new(plugin))
    }
    pub fn store(&self) -> Arc<store::Store> {
        self.store.clone()
    }
    async fn domain(&self, claims: &InvocationClaims, id: &str) -> Result<store::Domain> {
        self.store
            .domain(&claims.project_id, &uuid(id)?)
            .await
            .map_err(store_error)?
            .ok_or_else(missing)
    }
    async fn create_domain(&self, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let input: NewDomain = parse(input)?;
        let domain = store::Domain {
            id: Uuid::new_v4(),
            domain: domain_name(&input.domain)?,
            token: hex::encode(seal::random::<16>()),
            verified: false,
        };
        if !self
            .store
            .add_domain(&claims.project_id, &domain)
            .await
            .map_err(store_error)?
        {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "This project already has this domain",
            ));
        }
        Ok(json!({"resource":{"id":domain.id.to_string(),"name":domain.domain}}))
    }
    async fn view_domain(&self, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let target: Target = parse(input)?;
        let domain = self.domain(claims, &target.resource_id).await?;
        let relay = self
            .store
            .relay(&claims.project_id)
            .await
            .map_err(store_error)?;
        let recent = self.store.recent(&domain.id).await.map_err(store_error)?;
        let mailboxes = self
            .store
            .domain_mailboxes(&domain.id)
            .await
            .map_err(store_error)?;
        let dkim = if mailboxes.is_empty() {
            None
        } else {
            self.dkim(&claims.customer_id, &domain.domain).await
        };
        let spf = match (&relay, &self.settings.platform) {
            (Some(_), _) | (None, transport::Platform::TurboSmtp { .. }) => {
                "v=spf1 include:_spf.turbo-smtp.com ~all"
            }
            (None, transport::Platform::Smtp { .. }) => "v=spf1 ~all",
        };
        let mut records = vec![
            json!({"type": "TXT", "name": format!("_webdock.{}", domain.domain), "value": format!("webdock-verify={}", domain.token), "purpose": "Proves you own the domain"}),
            json!({"type": "TXT", "name": domain.domain, "value": spf, "purpose": "Allows the sending service (SPF)"}),
            json!({"type": "TXT", "name": format!("_dmarc.{}", domain.domain), "value": "v=DMARC1; p=none", "purpose": "Reports on mail that claims your domain (DMARC)"}),
        ];
        if let Some((selector, key)) = dkim {
            records.push(json!({"type": "TXT", "name": format!("{selector}._domainkey.{}", domain.domain), "value": format!("v=DKIM1; k=rsa; p={key}"), "purpose": "Signs mail from your mailboxes (DKIM)"}));
        }
        Ok(json!({
            "domain": domain.domain,
            "verified": domain.verified || self.settings.trust_unverified_domains,
            "verificationSkipped": !domain.verified && self.settings.trust_unverified_domains,
            "transport": if relay.is_some() { "turbosmtp" } else { "platform" },
            "records": records,
            "incoming": self.incoming_state(&claims.customer_id).await?,
            "mailboxes": mailboxes.iter().map(|local| format!("{local}@{}", domain.domain)).collect::<Vec<_>>(),
            "recent": recent.iter().map(|s| json!({"from": s.sender, "to": s.recipients, "subject": s.subject, "transport": s.transport, "status": s.status, "at": s.created_at})).collect::<Vec<_>>(),
        }))
    }
    async fn verify_domain(&self, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let target: Target = parse(input)?;
        let domain = self.domain(claims, &target.resource_id).await?;
        let expected = format!("webdock-verify={}", domain.token);
        let found = domain.verified
            || self
                .dns
                .txt(&format!("_webdock.{}", domain.domain))
                .await
                .iter()
                .any(|value| value.trim() == expected);
        if found && !self.store.verified(&domain.id).await.map_err(store_error)? {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Another project has already verified this domain",
            ));
        }
        Ok(json!({"verified": found}))
    }
    async fn connect_relay(&self, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let input: NewRelay = parse(input)?;
        let (key, secret) = (input.consumer_key.trim(), input.consumer_secret.trim());
        for value in [key, secret] {
            if value.is_empty()
                || value.len() > 512
                || !value.bytes().all(|b| (0x21..=0x7e).contains(&b))
            {
                return Err(invalid(
                    "Enter the consumer key and secret from your turboSMTP account",
                ));
            }
        }
        let id = Uuid::new_v4();
        let hint = format!("…{}", &key[key.len().saturating_sub(4)..]);
        if !self
            .store
            .add_relay(
                &claims.project_id,
                &id,
                &self.sealer.seal(&id, key),
                &self.sealer.seal(&id, secret),
                &hint,
            )
            .await
            .map_err(store_error)?
        {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "This project already has a turboSMTP account",
            ));
        }
        Ok(json!({"resource":{"id":id.to_string(),"name":"turboSMTP account"}}))
    }
    async fn relay(&self, claims: &InvocationClaims, input: Value) -> Result<store::Relay> {
        let target: Target = parse(input)?;
        let relay = self
            .store
            .relay(&claims.project_id)
            .await
            .map_err(store_error)?
            .ok_or_else(missing)?;
        if relay.id != uuid(&target.resource_id)? {
            return Err(missing());
        }
        Ok(relay)
    }
}

#[derive(Clone, Copy)]
enum MailActionBoundary {
    Handler,
    Dispatch,
    Result,
    RecoveryFinish,
}
impl MailActionBoundary {
    fn token(self) -> &'static str {
        match self {
            Self::Handler => "handler_enter",
            Self::Dispatch => "dispatch",
            Self::Result => "result",
            Self::RecoveryFinish => "recovery_finish",
        }
    }
    fn line(self, action: &str, code: Option<ErrorCode>) -> Option<String> {
        let action = match action {
            "domains.create" => "domains.create",
            "mail.send" => "mail.send",
            "incoming.activate" => "incoming.activate",
            "mailboxes.create" => "mailboxes.create",
            "mailboxes.view" => "mailboxes.view",
            "mailboxes.credentials" => "mailboxes.credentials",
            _ => return None,
        };
        let code = match code {
            None => "none",
            Some(ErrorCode::InvalidInput) => "invalid_input",
            Some(ErrorCode::NotSupported) => "not_supported",
            Some(ErrorCode::PermissionDenied) => "permission_denied",
            Some(ErrorCode::CapabilityUnavailable) => "capability_unavailable",
            Some(ErrorCode::Conflict) => "conflict",
            Some(ErrorCode::Unavailable) => "unavailable",
            Some(ErrorCode::DeadlineExceeded) => "deadline_exceeded",
            Some(ErrorCode::NotFound) => "not_found",
        };
        Some(format!(
            "mail_action action={action} stage={} publicCode={code}",
            self.token()
        ))
    }
    fn enabled(value: Option<&std::ffi::OsStr>) -> bool {
        value == Some(std::ffi::OsStr::new("1"))
    }
    fn emit(self, action: &str, code: Option<ErrorCode>) {
        if Self::enabled(std::env::var_os("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref())
            && let Some(line) = self.line(action, code)
        {
            use std::io::Write;
            let _ = writeln!(std::io::stderr().lock(), "{line}");
        }
    }
}
pub struct Handler(pub Arc<Plugin>);
#[tonic::async_trait]
impl ActionHandler for Handler {
    async fn invoke(
        &self,
        claims: &InvocationClaims,
        token: &str,
        input: Value,
        configuration: Value,
    ) -> Result<Value> {
        MailActionBoundary::Handler.emit(&claims.action, None);
        if configuration != json!({}) {
            MailActionBoundary::Result.emit(&claims.action, Some(ErrorCode::InvalidInput));
            return Err(invalid("Invalid mail request"));
        }
        let plugin = &self.0;
        let recovery = plugin.store.recovery_effect().await.map_err(|_| {
            MailActionBoundary::Result.emit(&claims.action, Some(ErrorCode::Unavailable));
            PluginError::new(ErrorCode::Unavailable, "Mail recovery is fenced")
        })?;
        let result = async {
        if !plugin
            .store
            .bind_project(&claims.project_id, &claims.customer_id)
            .await
            .map_err(store_error)?
        {
            return Err(PluginError::denied());
        }
        let transition = matches!(
            claims.action.as_str(),
            "incoming.activate" | "mailboxes.create" | "domains.delete"
        );
        let _fence =
            if transition || matches!(claims.action.as_str(), "relay.connect" | "relay.delete") {
                Some(
                    plugin
                        .store
                        .customer_fence(&claims.customer_id)
                        .await
                        .map_err(|_| {
                            PluginError::new(ErrorCode::Unavailable, "Mail transition unavailable")
                        })?,
                )
            } else {
                None
            };
        if transition {
            plugin.transition(token, &input).await?;
        }
        MailActionBoundary::Dispatch.emit(&claims.action,None);
        match claims.action.as_str() {
            "domains.create" => plugin.create_domain(claims, input).await,
            "domains.view" => plugin.view_domain(claims, input).await,
            "domains.verify" => plugin.verify_domain(claims, input).await,
            "domains.delete" => {
                let target: Target = parse(input)?;
                // The Core proved ownership; a domain already gone here is simply done.
                if !plugin
                    .store
                    .delete_domain(&claims.project_id, &uuid(&target.resource_id)?)
                    .await
                    .map_err(store_error)?
                {
                    return Err(PluginError::new(
                        ErrorCode::Conflict,
                        "Remove the domain's mailboxes first",
                    ));
                }
                Ok(json!({}))
            }
            "incoming.activate" => plugin.incoming(claims, input, true).await,
            "incoming.deactivate" => plugin.incoming(claims, input, false).await,
            "mailboxes.create" => plugin.create_mailbox(claims, input).await,
            "mailboxes.view" => plugin.view_mailbox(claims, input).await,
            "mailboxes.credentials" => plugin.mailbox_password(claims, input).await,
            "mailboxes.rotate" => plugin.rotate_mailbox(claims, input).await,
            "mailboxes.delete" => plugin.delete_mailbox(claims, input).await,
            "mail.send" => plugin.queued_send(claims, token, input).await,
            "mail.status" | "mail.cancel" => plugin.outbox_status(token, input, false).await,
            "mail.history" => plugin.outbox_status(token, input, true).await,
            "relay.connect" => plugin.connect_relay(claims, input).await,
            "relay.view" => {
                let relay = plugin.relay(claims, input).await?;
                Ok(
                    json!({"provider": "turboSMTP", "region": "EU", "keyHint": relay.key_hint, "connectedAt": relay.created_at}),
                )
            }
            "relay.delete" => {
                let target: Target = parse(input)?;
                // Idempotent: deleting an account that is already gone succeeds.
                plugin
                    .store
                    .delete_relay(&claims.project_id, &uuid(&target.resource_id)?)
                    .await
                    .map_err(store_error)?;
                Ok(json!({}))
            }
            _ => Err(PluginError::new(
                ErrorCode::NotSupported,
                "Mail action unavailable",
            )),
        }
        }.await;
        MailActionBoundary::Result.emit(
            &claims.action,
            result.as_ref().err().map(|error| error.code),
        );
        let finished = recovery.finish().await;
        MailActionBoundary::RecoveryFinish.emit(
            &claims.action,
            finished.as_ref().err().map(|_| ErrorCode::Unavailable),
        );
        finished.map_err(|_| {
            PluginError::new(ErrorCode::Unavailable, "Mail recovery effect is uncertain")
        })?;
        result
    }
}

/// The runtime refuses to start with any manifest other than the reviewed contract.
pub fn validate_manifest(manifest: &Manifest) -> Result<()> {
    let value = manifest.as_json();
    if manifest.id() != PLUGIN_ID
        || value["capabilities"] != json!(["plugin.workloads"])
        || value["httpBindings"] != json!([])
        || value["services"]
            != json!({"required":[{"name":"native-mail-control","version":1}],"optional":[]})
        || value["version"] != "0.3.0"
        || value["schemaVersion"] != 2
        || value["readableSchema"] != json!({"min":1,"max":2})
        || value["actions"]
            .as_array()
            .is_none_or(|a| a.len() != ACTIONS.len())
        || !ACTIONS.iter().all(|(name, intent)| {
            manifest.action(name).is_some_and(|action| {
                action["intent"] == *intent && action["capabilities"] == json!([])
            })
        })
    {
        return Err(invalid("Invalid mail manifest"));
    }
    Ok(())
}

#[cfg(test)]
mod system_dns_probe {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;

    fn os_inputs() {
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        // SAFETY: the POSIX getter takes no pointers and has no preconditions.
        let euid = unsafe { geteuid() };
        for (name, path, file) in [
            ("root", "/", false),
            ("etc", "/etc", false),
            ("resolv_conf", "/etc/resolv.conf", true),
            ("hosts", "/etc/hosts", true),
            ("hostname", "/etc/hostname", true),
        ] {
            let metadata = std::fs::symlink_metadata(path);
            let value = metadata.as_ref().ok();
            let mut readable = None;
            let mut read_errno = None;
            let mut read_bytes = None;
            let mut read_sha256 = None;
            if file {
                let read = || -> std::io::Result<Vec<u8>> {
                    let source = std::fs::File::open(path)?;
                    let mut bytes = Vec::new();
                    source.take(65537).read_to_end(&mut bytes)?;
                    Ok(bytes)
                }();
                readable = Some(read.is_ok());
                match read {
                    Ok(bytes) if bytes.len() <= 65536 => {
                        read_bytes = Some(bytes.len());
                        read_sha256 = Some(hex::encode(Sha256::digest(&bytes)));
                    }
                    Ok(_) => panic!("fixed OS input exceeds probe bound"),
                    Err(error) => read_errno = error.raw_os_error(),
                }
            }
            let file_type = value.map_or("unavailable", |m| {
                if m.file_type().is_symlink() {
                    "symlink"
                } else if m.is_dir() {
                    "directory"
                } else if m.is_file() {
                    "regular"
                } else {
                    "other"
                }
            });
            println!(
                "mail_dns_os_input {}",
                serde_json::json!({
                    "name":name,"currentEuid":euid,"metadataOk":metadata.is_ok(),
                    "metadataErrno":metadata.as_ref().err().and_then(std::io::Error::raw_os_error),
                    "fileType":file_type,"uid":value.map(MetadataExt::uid),
                    "gid":value.map(MetadataExt::gid),"mode":value.map(|m|m.mode()&0o7777),
                    "nlink":value.map(MetadataExt::nlink),"len":value.map(std::fs::Metadata::len),
                    "readable":readable,"readErrno":read_errno,"readBytes":read_bytes,"readSHA256":read_sha256,
                })
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "owned constructor-only probe; no DNS lookup"]
    async fn constructor_only() {
        assert_eq!(
            std::env::var("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref(),
            Ok("1")
        );
        let metadata = std::env::var("WEBDOCK_DNS_OS_METADATA").as_deref() == Ok("1");
        let expected = match std::env::var("WEBDOCK_DNS_CONSTRUCTOR_EXPECT").as_deref() {
            Ok("ok") => Some(true),
            Ok("deny") => Some(false),
            Ok("observe") if metadata => None,
            _ => panic!("fixed constructor expectation required"),
        };
        if metadata {
            os_inputs();
        }
        let readable = std::fs::read("/etc/resolv.conf");
        let result = super::SystemDns::new();
        println!(
            "mail_dns_constructor {{\"constructorOk\":{},\"readable\":{},\"readErrno\":{}}}",
            result.is_ok(),
            readable.is_ok(),
            readable
                .as_ref()
                .err()
                .and_then(std::io::Error::raw_os_error)
                .map_or_else(|| "null".to_owned(), |number| number.to_string())
        );
        if metadata {
            let (class, errno) = match result.as_ref().err() {
                None => ("ok", None),
                Some(error) => match error
                    .downcast_ref::<hickory_resolver::ResolveError>()
                    .and_then(hickory_resolver::ResolveError::proto)
                    .map(hickory_resolver::proto::ProtoError::kind)
                {
                    Some(hickory_resolver::proto::ProtoErrorKind::Io(error)) => {
                        ("io", error.raw_os_error())
                    }
                    Some(_) => ("proto_other", None),
                    None => ("resolver_other", None),
                },
            };
            println!(
                "mail_dns_constructor_class {}",
                serde_json::json!({"class":class,"errno":errno})
            );
        }
        if let Some(expected) = expected {
            assert_eq!(result.is_ok(), expected, "constructor outcome mismatch");
        }
    }
}

#[cfg(test)]
mod action_diagnostic_tests {
    use super::*;
    #[test]
    fn fixed_handler_markers_keep_body_and_finish_outcomes_separate_without_private_error_text() {
        for value in [None, Some("0"), Some("true"), Some("01"), Some("1")] {
            assert_eq!(
                MailActionBoundary::enabled(value.map(std::ffi::OsStr::new)),
                value == Some("1")
            );
        }
        for action in [
            "domains.create",
            "mail.send",
            "incoming.activate",
            "mailboxes.create",
            "mailboxes.view",
            "mailboxes.credentials",
        ] {
            for stage in [
                MailActionBoundary::Handler,
                MailActionBoundary::Dispatch,
                MailActionBoundary::Result,
                MailActionBoundary::RecoveryFinish,
            ] {
                let line = stage.line(action, None).unwrap();
                assert!(line.starts_with("mail_action action="));
                assert!(!line.contains("private"));
            }
        }
        let error = PluginError::new(ErrorCode::NotFound, "private-body-poison");
        let body = MailActionBoundary::Result
            .line("mail.send", Some(error.code))
            .unwrap();
        let finish = MailActionBoundary::RecoveryFinish
            .line("mail.send", Some(ErrorCode::Unavailable))
            .unwrap();
        assert!(body.contains("publicCode=not_found") && finish.contains("publicCode=unavailable"));
        assert!(!format!("{body}{finish}").contains("poison"));
        assert!(
            MailActionBoundary::Handler
                .line("private-token-poison", None)
                .is_none()
        );
    }
}
