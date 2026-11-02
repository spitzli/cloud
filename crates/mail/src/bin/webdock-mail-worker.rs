//! Private payload consumer. Control HTTPS and the authenticated plugin channel are distinct.
use serde::Deserialize;
use serde_json::json;
use std::{
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use webdock_mail::{
    dispatch::{Dispatched, dispatch_one},
    outbox::{PayloadFormat, Receipt},
    payload::PrivatePayloadStore,
    provider_transport::{MAX_PROVIDERS, ProviderBinding, ProviderTransport, RoutingTransport},
    send_control::{HttpSendControl, SendControl},
};
use webdock_plugin_runtime::PinnedMutualTls;

type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    control_origin: String,
    control_secret: String,
    control_authority: Option<PathBuf>,
    payload_directory: PathBuf,
    payload_key: String,
    issuer_public_key_hex: String,
    providers: Vec<ProviderConfiguration>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderConfiguration {
    binding: ProviderBinding,
    channel: PluginChannel,
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum PluginChannel {
    Unix {
        path: PathBuf,
    },
    MutualTls {
        origin: String,
        domain: String,
        certificate: PathBuf,
        private_key: PathBuf,
        authority: PathBuf,
        certificate_sha256: String,
    },
}
fn file(path: &Path, private: bool) -> Result<Vec<u8>, Error> {
    if !path.is_absolute() {
        return Err("Absolute Mail worker configuration path required".into());
    }
    let file = std::fs::File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
        || metadata.mode() & if private { 0o077 } else { 0o022 } != 0
        || metadata.len() > 65536
    {
        return Err("Untrusted Mail worker configuration file".into());
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("Mail worker configuration limit".into());
    }
    Ok(bytes)
}
async fn channel(config: PluginChannel) -> Result<tonic::transport::Channel, Error> {
    match config {
        PluginChannel::Unix { path } => webdock_plugin_runtime::unix_channel(path).await,
        PluginChannel::MutualTls {
            origin,
            domain,
            certificate,
            private_key,
            authority,
            certificate_sha256,
        } => {
            webdock_plugin_runtime::pinned_mtls_channel(
                origin,
                domain,
                PinnedMutualTls {
                    certificate: file(&certificate, false)?,
                    private_key: file(&private_key, true)?,
                    authority: file(&authority, false)?,
                    server_certificate_sha256: certificate_sha256,
                },
            )
            .await
        }
    }
}
async fn file_accepted(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
    dispatched: &Dispatched,
) {
    if dispatched.job.payload_format != PayloadFormat::Mime
        || !matches!(dispatched.receipt, Some(Receipt::Accepted { .. }))
    {
        return;
    }
    if webdock_mail::webmail_send_api::file_queued(control, store, &dispatched.job)
        .await
        .is_err()
    {
        let _ = control
            .call(json!({
                "action":"send.webmail.filed",
                "id":dispatched.job.id,
                "customerID":dispatched.job.customer_id,
                "leaseToken":dispatched.job.lease_token,
                "filed":false
            }))
            .await;
        eprintln!("Accepted Mail needs Sent filing review");
    }
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        return Err("One private Mail worker configuration path required".into());
    }
    let config: Configuration = serde_json::from_slice(&file(Path::new(&args[0]), true)?)
        .map_err(|_| "Invalid Mail worker configuration")?;
    let trust = match config.control_authority {
        Some(path) => {
            let trust = reqwest::Certificate::from_pem_bundle(&file(&path, false)?)?;
            if trust.is_empty() || trust.len() > 32 {
                return Err("Invalid Mail control authority".into());
            }
            trust
        }
        None => vec![],
    };
    let control = HttpSendControl::new(&config.control_origin, &config.control_secret, &trust)?;
    let store = Arc::new(PrivatePayloadStore::open(
        &config.payload_directory,
        &config.payload_key,
    )?);
    let issuer: [u8; 32] = hex::decode(&config.issuer_public_key_hex)
        .map_err(|_| "Invalid Mail issuer key")?
        .try_into()
        .map_err(|_| "Invalid Mail issuer key")?;
    let issuer =
        ed25519_dalek::VerifyingKey::from_bytes(&issuer).map_err(|_| "Invalid Mail issuer key")?;
    if config.providers.len() > MAX_PROVIDERS {
        return Err("Too many Mail provider channels".into());
    }
    let mut keys = std::collections::HashSet::new();
    for provider in &config.providers {
        if !keys.insert((&provider.binding.plugin_id, provider.binding.generation)) {
            return Err("Duplicate Mail provider channel".into());
        }
    }
    let mut providers = Vec::new();
    for provider in config.providers {
        providers.push(ProviderTransport::new(
            channel(provider.channel).await?,
            provider.binding,
            issuer,
        )?);
    }
    let transport = RoutingTransport::new(providers)?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut cleaned_at = tokio::time::Instant::now();
    loop {
        let attempt = dispatch_one(&control, store.clone(), &transport);
        tokio::pin!(attempt);
        let (result, stopping) = tokio::select! {
            result = &mut attempt => (result, false),
            _ = terminate.recv() => (attempt.await, true),
            result = tokio::signal::ctrl_c() => { result?; (attempt.await, true) },
        };
        let idle = match result {
            Ok(Some(dispatched)) => {
                file_accepted(&control, store.clone(), &dispatched).await;
                false
            }
            Ok(None) => true,
            Err(_) => {
                eprintln!("Mail dispatch unavailable; any committed attempt is not retried");
                true
            }
        };
        if stopping {
            return Ok(());
        }
        if cleaned_at.elapsed() >= Duration::from_secs(60) {
            if webdock_mail::dispatch::cleanup_payloads(&control, store.clone())
                .await
                .is_err()
            {
                eprintln!("Mail payload cleanup unavailable");
            }
            cleaned_at = tokio::time::Instant::now();
        }
        if !idle {
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
            _ = terminate.recv() => return Ok(()),
            result = tokio::signal::ctrl_c() => { result?; return Ok(()); },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, sync::Mutex};
    use webdock_mail::{outbox::SendLease, send_control::ControlError};
    struct Control(Mutex<Vec<serde_json::Value>>);
    #[async_trait::async_trait]
    impl SendControl for Control {
        async fn call(&self, input: serde_json::Value) -> Result<serde_json::Value, ControlError> {
            self.0.lock().unwrap().push(input.clone());
            match input["action"].as_str().unwrap() {
                "send.webmail.filingAccess" => Err(ControlError::unavailable()),
                "send.webmail.filed" => Ok(json!({"recorded":true,"filed":false})),
                _ => panic!("Unexpected control action"),
            }
        }
    }
    #[tokio::test]
    async fn failed_filing_preserves_payload_and_only_accepted_mime_requests_filing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Arc::new(
            PrivatePayloadStore::open(dir.path(), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
                .unwrap(),
        );
        let payload = store.put("12", b"retained MIME", None).unwrap();
        let control = Control(Mutex::new(vec![]));
        let mut dispatched = Dispatched {
            job: SendLease {
                id: "789".into(),
                customer_id: "12".into(),
                source_id: "webmail:34".into(),
                payload_ref: payload.reference,
                payload_bytes: payload.bytes as i32,
                request_hash: "a".repeat(64),
                payload_format: PayloadFormat::Mime,
                source_context: json!({}),
                route: json!({"kind":"stalwart"}),
                envelope: json!({}),
                lease_token: uuid::Uuid::new_v4().to_string(),
            },
            receipt: Some(Receipt::Accepted {
                provider_id: "789".into(),
            }),
        };
        file_accepted(&control, store.clone(), &dispatched).await;
        {
            let calls = control.0.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[1]["action"], "send.webmail.filed");
            assert_eq!(calls[1]["id"], "789");
            assert_eq!(calls[1]["filed"], false);
        }
        assert!(store.get("12", &dispatched.job.payload_ref).is_ok());
        dispatched.receipt = Some(Receipt::Unknown {});
        file_accepted(&control, store.clone(), &dispatched).await;
        dispatched.receipt = Some(Receipt::Accepted {
            provider_id: "789".into(),
        });
        dispatched.job.payload_format = PayloadFormat::Request;
        file_accepted(&control, store, &dispatched).await;
        assert_eq!(control.0.lock().unwrap().len(), 2);
    }
}
