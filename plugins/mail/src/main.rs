//! Mail runtime over private Unix sockets or pinned mTLS, with its own metadata database.
use serde::Deserialize;
use std::{path::PathBuf, process::ExitCode, sync::Arc};
use webdock_plugin_kit::transport::{CoreTls, ListenTls, RuntimeTransport};
use webdock_plugin_runtime::{self as runtime};
type Error = Box<dyn std::error::Error + Send + Sync>;

mod startup;

enum StartupStage {
    Args,
    ConfigRead,
    ConfigDecode,
    Transport,
    ManifestRead,
    ManifestDecode,
    Platform,
    Store,
    CoreClient,
    Engines,
    Plugin,
    Sealer,
    Dns,
    Authority,
    DeliveryPeers,
    Payloads,
    DeliveryPolicy,
    NativeControl,
    ListenerCertificates,
    ProviderBinding,
    ManagedTransport,
    DeliveryWiring,
    Runtime,
    Listener,
}
impl StartupStage {
    fn emit(self) {
        if std::env::var_os("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            let stage = match self {
                Self::Args => "args",
                Self::ConfigRead => "config_read",
                Self::ConfigDecode => "config_decode",
                Self::Transport => "transport",
                Self::ManifestRead => "manifest_read",
                Self::ManifestDecode => "manifest_decode",
                Self::Platform => "platform",
                Self::Store => "store",
                Self::CoreClient => "core_client",
                Self::Engines => "engines",
                Self::Plugin => "plugin",
                Self::Sealer => "sealer",
                Self::Dns => "dns",
                Self::Authority => "authority",
                Self::DeliveryPeers => "delivery_peers",
                Self::Payloads => "payloads",
                Self::DeliveryPolicy => "delivery_policy",
                Self::NativeControl => "native_control",
                Self::ListenerCertificates => "listener_certificates",
                Self::ProviderBinding => "provider_binding",
                Self::ManagedTransport => "managed_transport",
                Self::DeliveryWiring => "delivery_wiring",
                Self::Runtime => "runtime",
                Self::Listener => "listener",
            };
            eprintln!("mail_startup_stage stage={stage}");
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    manifest: PathBuf,
    issuer_public_key_hex: String,
    generation: u64,
    listen: Option<PathBuf>,
    core: Option<PathBuf>,
    listen_tls: Option<ListenTls>,
    core_tls: Option<CoreTls>,
    core_peers: Vec<runtime::CorePeer>,
    storage_url: String,
    storage_ca: Option<PathBuf>,
    secret_key_hex: String,
    platform_transport: webdock_mail::transport::Platform,
    #[serde(default)]
    trust_unverified_domains: bool,
    mailboxes: Option<webdock_mail::engine::EngineConfig>,
    delivery: Option<webdock_mail::executor::DeliveryConfiguration>,
}
fn certificate_leaf(path: &std::path::Path) -> Result<Vec<u8>, Error> {
    rustls_pemfile::certs(&mut std::io::Cursor::new(webdock_plugin_kit::read_file(
        path, false,
    )?))
    .next()
    .transpose()?
    .map(|c| c.as_ref().to_vec())
    .ok_or_else(|| "runtime certificate required".into())
}
#[cfg(unix)]
unsafe extern "C" {
    fn geteuid() -> u32;
}
fn report_config_read_failure(path: &std::path::Path, error: &Error) {
    if std::env::var_os("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    // Diagnostics are secondary. The caller returns the same original error even if this fails.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        use std::io::Write;
        use std::os::unix::fs::MetadataExt;
        #[cfg(unix)]
        // SAFETY: POSIX geteuid has no arguments or caller preconditions; it only reads this process identity.
        let current_euid = Some(unsafe { geteuid() });
        #[cfg(not(unix))]
        let current_euid: Option<u32> = None;
        let metadata = std::fs::symlink_metadata(path);
        let value = metadata.as_ref().ok();
        let file_type = value.map_or("unavailable", |m| {
            if m.file_type().is_symlink() {
                "symlink"
            } else if m.is_file() {
                "regular"
            } else if m.is_dir() {
                "directory"
            } else {
                "other"
            }
        });
        let io_error = error.downcast_ref::<std::io::Error>();
        let record = serde_json::json!({"expected_fixed_path":path==std::path::Path::new("/run/webdock/runtime/runtime.json"),"absolute":path.is_absolute(),"file_type":file_type,"current_euid":current_euid,"file_uid":value.map(MetadataExt::uid),"file_gid":value.map(MetadataExt::gid),"mode":value.map(|m|m.mode()&0o7777),"nlink":value.map(MetadataExt::nlink),"len":value.map(std::fs::Metadata::len),"error_kind":if io_error.is_some(){"io"}else{"non_io"},"errno":io_error.and_then(std::io::Error::raw_os_error),"metadata_ok":metadata.is_ok(),"metadata_errno":metadata.as_ref().err().and_then(std::io::Error::raw_os_error)});
        if let Ok(encoded) = serde_json::to_vec(&record) {
            let mut line = b"mail_startup_read_metadata ".to_vec();
            line.extend_from_slice(&encoded);
            line.push(b'\n');
            let _ = std::io::stderr().lock().write_all(&line);
        }
    }));
}

async fn run() -> Result<(), Error> {
    StartupStage::Args.emit();
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("configuration path required")?);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    StartupStage::ConfigRead.emit();
    let configuration_bytes = match webdock_plugin_kit::read_file(&path, true) {
        Ok(bytes) => bytes,
        Err(error) => {
            report_config_read_failure(&path, &error);
            return Err(error);
        }
    };
    StartupStage::ConfigDecode.emit();
    let config: Configuration = serde_json::from_slice(&configuration_bytes)?;
    drop(configuration_bytes);
    let transport = RuntimeTransport {
        listen: config.listen,
        core: config.core,
        listen_tls: config.listen_tls,
        core_tls: config.core_tls,
    };
    StartupStage::Transport.emit();
    transport.validate()?;
    StartupStage::ManifestRead.emit();
    let manifest_bytes = webdock_plugin_kit::read_file(&config.manifest, false)?;
    StartupStage::ManifestDecode.emit();
    let manifest = webdock_plugin_protocol::Manifest::parse(&manifest_bytes)?;
    webdock_mail::validate_manifest(&manifest)?;
    StartupStage::Platform.emit();
    config.platform_transport.validate()?;
    if config.trust_unverified_domains
        && !matches!(
            config.platform_transport,
            webdock_mail::transport::Platform::Smtp { .. }
        )
    {
        return Err("unverified domains are trusted only with a loopback catcher".into());
    }
    StartupStage::Store.emit();
    let store = match config.storage_ca {
        Some(path) => {
            webdock_mail::store::Store::connect_tls(
                &config.storage_url,
                &webdock_plugin_kit::read_file(&path, false)?,
            )
            .await?
        }
        None => webdock_mail::store::Store::connect(&config.storage_url).await?,
    };
    StartupStage::CoreClient.emit();
    let client = transport.core_client_lazy().await?;
    StartupStage::Engines.emit();
    let engines = match config.mailboxes {
        Some(mailboxes) => Some(webdock_mail::engine::Engines::new(
            Box::new(webdock_mail::engine::HostWorkloads {
                client: client.clone(),
                plugin: manifest.id().into(),
                generation: config.generation,
            }),
            mailboxes,
        )?),
        None => None,
    };
    StartupStage::Plugin.emit();
    let plugin = webdock_mail::Plugin::new(
        store,
        {
            StartupStage::Sealer.emit();
            webdock_mail::seal::Sealer::new(&config.secret_key_hex)?
        },
        webdock_mail::transport::Sender::new(webdock_mail::transport::TURBOSMTP_ORIGIN),
        Box::new({
            StartupStage::Dns.emit();
            webdock_mail::SystemDns::new()?
        }),
        webdock_mail::Settings {
            platform: config.platform_transport.clone(),
            trust_unverified_domains: config.trust_unverified_domains,
        },
        engines,
    );
    StartupStage::Authority.emit();
    let key: [u8; 32] = hex::decode(config.issuer_public_key_hex)?
        .try_into()
        .map_err(|_| "invalid key")?;
    let authority = runtime::RemoteAuthority::new(
        client.clone(),
        manifest.id().into(),
        config.generation,
        config.core_peers.clone(),
    )?;
    let mut native_router = axum::Router::new();
    let mut dispatch = None;
    let plugin =
        if let Some(delivery) = config.delivery {
            StartupStage::DeliveryPeers.emit();
            if delivery.gateway_peers.len() > 16
                || delivery.gateway_peers.iter().any(|peer| match peer {
                    runtime::CorePeer::MutualTls { certificate_sha256 } => {
                        !webdock_mail_protocol::valid_digest(certificate_sha256)
                    }
                    runtime::CorePeer::Unix { .. } => false,
                })
            {
                return Err("invalid gateway peers".into());
            }
            for gateway in &delivery.gateway_peers {
                if config.core_peers.iter().any(|core| {
                    serde_json::to_value(core).ok() == serde_json::to_value(gateway).ok()
                }) {
                    return Err("gateway and Core peers must be distinct".into());
                }
            }
            StartupStage::Payloads.emit();
            let payloads: Arc<dyn webdock_mail_runtime::PrivatePayloadStore> = Arc::new(
                webdock_mail::payload::PgPrivatePayloadStore::new(
                    plugin.store(),
                    &config.secret_key_hex,
                )
                .await?,
            );
            StartupStage::DeliveryPolicy.emit();
            let policy = webdock_mail::executor::Policy::new(
                plugin.store(),
                config.platform_transport.clone(),
                {
                    StartupStage::Sealer.emit();
                    webdock_mail::seal::Sealer::new(&config.secret_key_hex)?
                },
                delivery.native_accounts,
            )?;
            StartupStage::NativeControl.emit();
            let control: Arc<dyn webdock_mail_runtime::SendControl> =
                webdock_mail::executor::FencedControl::new(
                    Arc::new(
                        webdock_mail_protocol::native_control::NativeControlClient::new(
                            client.channel(),
                            config.generation,
                        )?,
                    ),
                    policy.clone(),
                );
            StartupStage::ListenerCertificates.emit();
            let listener_leaf = transport
                .listen_tls
                .as_ref()
                .map(|tls| certificate_leaf(&tls.certificate))
                .transpose()?;
            let callback_leaf = transport
                .core_tls
                .as_ref()
                .map(|tls| certificate_leaf(&tls.certificate))
                .transpose()?;
            StartupStage::ProviderBinding.emit();
            let binding = webdock_mail::executor::resolve_provider_binding(
                delivery.provider_binding,
                config.generation,
                &manifest_bytes,
                listener_leaf.as_deref(),
                callback_leaf.as_deref(),
            )?;
            StartupStage::ManagedTransport.emit();
            let executor = webdock_mail::executor::ManagedTransport::new(
                policy,
                client.channel(),
                binding,
                &delivery.native_lease_issuer_public_key_hex,
                webdock_mail::transport::Sender::new(webdock_mail::transport::TURBOSMTP_ORIGIN),
                config.generation,
            )?;
            native_router = webdock_mail::listener::native_routes(
                webdock_mail_runtime::send_api::SendApi::new(control.clone(), payloads.clone())
                    .router(),
                delivery.gateway_peers,
            );
            dispatch = Some(startup::Dispatcher {
                control: control.clone(),
                payloads: payloads.clone(),
                #[cfg(not(test))]
                transport: executor,
                #[cfg(test)]
                transport: Box::new(executor),
            });
            StartupStage::DeliveryWiring.emit();
            plugin.with_delivery(webdock_mail::delivery::Delivery::new(control, payloads))?
        } else {
            plugin
        };
    StartupStage::Runtime.emit();
    let runtime = runtime::Runtime::new(
        manifest,
        config.generation,
        ed25519_dalek::VerifyingKey::from_bytes(&key)?,
        Arc::new(authority),
        Arc::new(webdock_mail::Handler(plugin.clone())),
    )?;
    let shutdown = async {
        #[cfg(unix)]
        {
            if let Ok(mut term) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {
                    result=tokio::signal::ctrl_c()=>{if result.is_ok(){startup::LifetimeEvent::Sigint}else{startup::LifetimeEvent::SignalApiError}.emit();},
                    result=term.recv()=>{if result.is_some(){startup::LifetimeEvent::Sigterm}else{startup::LifetimeEvent::SignalStreamClosed}.emit();}
                }
            } else {
                if tokio::signal::ctrl_c().await.is_ok() {
                    startup::LifetimeEvent::Sigint
                } else {
                    startup::LifetimeEvent::SignalApiError
                }
                .emit();
            }
        }
        #[cfg(not(unix))]
        {
            if tokio::signal::ctrl_c().await.is_ok() {
                startup::LifetimeEvent::Sigint
            } else {
                startup::LifetimeEvent::SignalApiError
            }
            .emit();
        }
    };
    let resume = || plugin.resume();
    StartupStage::Listener.emit();
    match (&transport.listen, &transport.listen_tls) {
        (Some(path), None) => {
            let socket = runtime::UnixSocket::bind(path)?;
            startup::serve_delivery(
                |stopped| async {
                    Ok(
                        webdock_mail::listener::serve_unix(runtime, native_router, socket, async {
                            let _ = stopped.await;
                        })
                        .await?,
                    )
                },
                shutdown,
                client,
                webdock_mail::PLUGIN_ID,
                config.generation,
                resume,
                dispatch,
            )
            .await?;
        }
        (None, Some(tls)) => {
            let material = tls.material()?;
            let listener = tokio::net::TcpListener::bind(tls.address).await?;
            startup::serve_delivery(
                |stopped| async {
                    Ok(webdock_mail::listener::serve_mtls(
                        runtime,
                        native_router,
                        listener,
                        material,
                        async {
                            let _ = stopped.await;
                        },
                    )
                    .await?)
                },
                shutdown,
                client,
                webdock_mail::PLUGIN_ID,
                config.generation,
                resume,
                dispatch,
            )
            .await?;
        }
        _ => return Err("listener required".into()),
    }
    Ok(())
}
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("mail_plugin_start_failed");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration() -> serde_json::Value {
        serde_json::json!({
            "manifest":"/run/mail/manifest.json", "issuerPublicKeyHex":"a".repeat(64),
            "generation":2, "corePeers":[{"kind":"mutualTls","certificateSha256":"b".repeat(64)}],
            "storageUrl":"postgresql://metadata.invalid/mail", "secretKeyHex":"c".repeat(64),
            "platformTransport":{"kind":"smtp","host":"127.0.0.1","port":9}
        })
    }

    #[test]
    fn managed_tls_configuration_needs_no_unix_paths() {
        let mut value = configuration();
        value["listenTls"] = serde_json::json!({"address":"0.0.0.0:8443","certificate":"/run/mail/runtime.crt","privateKey":"/run/mail/runtime.key","authority":"/run/mail/core-ca.crt"});
        value["coreTls"] = serde_json::json!({"origin":"https://core.test:8443","domain":"core.test","certificate":"/run/mail/runtime.crt","privateKey":"/run/mail/runtime.key","authority":"/run/mail/core-ca.crt","serverCertificateSha256":"b".repeat(64)});
        value["storageCa"] = "/run/mail/storage-ca.crt".into();
        assert!(serde_json::from_value::<Configuration>(value).is_ok());
    }

    #[test]
    fn legacy_private_unix_configuration_still_parses() {
        let mut value = configuration();
        value["listen"] = "/run/mail/plugin.sock".into();
        value["core"] = "/run/mail/core.sock".into();
        value["storageUrl"] = "postgresql://localhost/mail".into();
        value["corePeers"] = serde_json::json!([{"kind":"unix","uid":1000}]);
        assert!(serde_json::from_value::<Configuration>(value).is_ok());
    }

    #[test]
    fn runtime_configuration_rejects_missing_ambiguous_and_untrusted_transports() {
        let mut unix = configuration();
        unix["listen"] = "/run/mail/plugin.sock".into();
        unix["core"] = "/run/mail/core.sock".into();
        let tls = serde_json::json!({"address":"0.0.0.0:8443","certificate":"/run/mail/runtime.crt","privateKey":"/run/mail/runtime.key","authority":"/run/mail/core-ca.crt"});
        let mut ambiguous = unix.clone();
        ambiguous["listenTls"] = tls;
        let mut relative = unix.clone();
        relative["core"] = "relative.sock".into();
        for value in [configuration(), ambiguous, relative] {
            let config = serde_json::from_value::<Configuration>(value).unwrap();
            assert!(
                RuntimeTransport {
                    listen: config.listen,
                    core: config.core,
                    listen_tls: config.listen_tls,
                    core_tls: config.core_tls
                }
                .validate()
                .is_err()
            );
        }
        unix["unknownPrivateField"] = true.into();
        assert!(serde_json::from_value::<Configuration>(unix).is_err());
        let mut strict_tls = configuration();
        strict_tls["listenTls"] = serde_json::json!({"address":"0.0.0.0:8443","certificate":"/run/mail/runtime.crt","privateKey":"/run/mail/runtime.key","authority":"/run/mail/core-ca.crt","insecure":true});
        assert!(serde_json::from_value::<Configuration>(strict_tls).is_err());
    }
}
