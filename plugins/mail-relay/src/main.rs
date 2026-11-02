//! Explicitly installed provider process. Configuration cannot enroll or approve itself.
use serde::Deserialize;
use std::{
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use webdock_mail_protocol::runtime::{MutualTls, RemoteAuthority, Runtime, UnixSocket};
use webdock_plugin_runtime::{CorePeer, PinnedMutualTls};
type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    #[serde(rename = "pluginID")]
    plugin_id: String,
    version: String,
    generation: u64,
    descriptor_sha256: String,
    issuer_public_key_hex: String,
    worker: CorePeer,
    core_peer: CorePeer,
    listen: Listen,
    core: Core,
    #[cfg(feature = "local-provider-test")]
    #[serde(default)]
    test_origin: Option<String>,
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Listen {
    Unix {
        path: PathBuf,
    },
    MutualTls {
        address: std::net::SocketAddr,
        certificate: PathBuf,
        private_key: PathBuf,
        authority: PathBuf,
    },
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Core {
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
        return Err("Absolute provider configuration path required".into());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        // SAFETY: geteuid has no preconditions or pointer arguments.
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & if private { 0o077 } else { 0o022 } != 0
        || metadata.len() > 65536
    {
        return Err("Untrusted provider configuration file".into());
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("Provider configuration limit".into());
    }
    Ok(bytes)
}
fn tls(certificate: &Path, private_key: &Path, authority: &Path) -> Result<MutualTls, Error> {
    Ok(MutualTls {
        identity: tonic::transport::Identity::from_pem(
            file(certificate, false)?,
            file(private_key, true)?,
        ),
        peer_ca: tonic::transport::Certificate::from_pem(file(authority, false)?),
    })
}
async fn shutdown() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("termination signal");
    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        return Err("One private provider configuration path required".into());
    }
    let config: Configuration = serde_json::from_slice(&file(Path::new(&args[0]), true)?)
        .map_err(|_| "Invalid provider configuration")?;
    let issuer: [u8; 32] = hex::decode(&config.issuer_public_key_hex)
        .map_err(|_| "Invalid issuer key")?
        .try_into()
        .map_err(|_| "Invalid issuer key")?;
    let issuer =
        ed25519_dalek::VerifyingKey::from_bytes(&issuer).map_err(|_| "Invalid issuer key")?;
    let channel = match config.core {
        Core::Unix { path } => webdock_plugin_runtime::unix_channel(path).await?,
        Core::MutualTls {
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
            .await?
        }
    };
    #[cfg(not(feature = "local-provider-test"))]
    let relay = webdock_mail_relay::Relay::new();
    #[cfg(feature = "local-provider-test")]
    let relay = if let Some(origin) = config.test_origin {
        webdock_mail_relay::Relay::local_fixture(&origin)?
    } else {
        webdock_mail_relay::Relay::new()
    };
    let runtime = Runtime::new(
        config.plugin_id,
        config.version,
        config.generation,
        config.descriptor_sha256,
        issuer,
        Arc::new(RemoteAuthority::new(
            config.worker,
            config.core_peer,
            channel,
        )),
        Arc::new(relay),
    )?;
    match config.listen {
        Listen::Unix { path } => {
            runtime
                .serve_unix(UnixSocket::bind(path)?, shutdown())
                .await?
        }
        Listen::MutualTls {
            address,
            certificate,
            private_key,
            authority,
        } => {
            runtime
                .serve_mtls(
                    tokio::net::TcpListener::bind(address).await?,
                    tls(&certificate, &private_key, &authority)?,
                    shutdown(),
                )
                .await?
        }
    };
    Ok(())
}
