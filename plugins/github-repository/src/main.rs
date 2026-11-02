//! Standalone provider process; private configuration and authenticated Core callback are required.
use serde::Deserialize;
use std::{
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use webdock_repository_protocol::{runtime, verify_descriptor};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    descriptor: String,
    publisher_public_key_hex: String,
    issuer_public_key_hex: String,
    generation: u64,
    app_id: u64,
    app_private_key: PathBuf,
    listen: Listen,
    core: Core,
    core_peers: Vec<runtime::CorePeer>,
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
        server_certificate_sha256: String,
    },
}
type Error = Box<dyn std::error::Error + Send + Sync>;
fn file(path: &Path, private: bool) -> Result<Vec<u8>, Error> {
    if !path.is_absolute() {
        return Err("absolute configuration path required".into());
    }
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let m = f.metadata()?;
    if !m.is_file()
        || m.nlink() != 1
        // SAFETY: geteuid has no preconditions or pointer arguments.
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & if private { 0o077 } else { 0o022 } != 0
        || m.len() > 65536
    {
        return Err("untrusted configuration file".into());
    }
    let mut bytes = Vec::new();
    f.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("configuration limit".into());
    }
    Ok(bytes)
}
fn key(hex: &str) -> Result<ed25519_dalek::VerifyingKey, Error> {
    let bytes: [u8; 32] = hex::decode(hex)?
        .try_into()
        .map_err(|_| "invalid verification key")?;
    Ok(ed25519_dalek::VerifyingKey::from_bytes(&bytes)?)
}
fn tls(
    certificate: &Path,
    private_key: &Path,
    authority: &Path,
) -> Result<runtime::MutualTls, Error> {
    Ok(runtime::MutualTls {
        identity: tonic::transport::Identity::from_pem(
            file(certificate, false)?,
            file(private_key, true)?,
        ),
        peer_ca: tonic::transport::Certificate::from_pem(file(authority, false)?),
    })
}
async fn run() -> Result<(), Error> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let mut args = std::env::args_os();
    args.next();
    let path = PathBuf::from(args.next().ok_or("configuration path required")?);
    if args.next().is_some() {
        return Err("unexpected arguments".into());
    }
    let config: Configuration = serde_json::from_slice(&file(&path, true)?)?;
    let publisher = key(&config.publisher_public_key_hex)?;
    let descriptor = verify_descriptor(&config.descriptor, &publisher)?;
    let channel = match &config.core {
        Core::Unix { path } => runtime::connect_unix(path).await?,
        Core::MutualTls {
            origin,
            domain,
            certificate,
            private_key,
            authority,
            server_certificate_sha256,
        } => {
            webdock_plugin_runtime::pinned_mtls_channel(
                origin.clone(),
                domain.clone(),
                webdock_plugin_runtime::PinnedMutualTls {
                    certificate: file(certificate, false)?,
                    private_key: file(private_key, true)?,
                    authority: file(authority, false)?,
                    server_certificate_sha256: server_certificate_sha256.clone(),
                },
            )
            .await?
        }
    };
    let authority = runtime::RemoteAuthority::new(
        channel,
        descriptor.plugin_id,
        config.generation,
        webdock_repository_protocol::sha256(config.descriptor.as_bytes()),
        config.core_peers,
    )?;
    let pem = file(&config.app_private_key, true)?;
    let provider = webdock_github_repository::GithubProvider::new(config.app_id, &pem)?;
    #[cfg(feature = "local-provider-test")]
    let provider = if let Some(origin) = config.test_origin {
        webdock_github_repository::GithubProvider::loopback_test(config.app_id, &pem, &origin)?
    } else {
        provider
    };
    let runtime = runtime::Runtime::new(
        config.descriptor,
        &publisher,
        config.generation,
        key(&config.issuer_public_key_hex)?,
        Arc::new(authority),
        Arc::new(provider),
    )?;
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    match config.listen {
        Listen::Unix { path } => {
            runtime
                .serve_unix(runtime::UnixSocket::bind(path)?, shutdown)
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
                    address,
                    tls(&certificate, &private_key, &authority)?,
                    shutdown,
                )
                .await?
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("repository_plugin_start_failed");
        std::process::exit(1);
    }
}
