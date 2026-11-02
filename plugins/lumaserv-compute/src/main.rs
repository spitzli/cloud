use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};
use webdock_plugin_runtime::{self as runtime, HostClient};
type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    manifest: PathBuf,
    issuer_public_key_hex: String,
    generation: u64,
    projects: BTreeMap<String, String>,
    listen: Listen,
    core: Core,
    core_peers: Vec<runtime::CorePeer>,
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
fn file(path: &Path, private: bool) -> Result<Vec<u8>, Error> {
    if !path.is_absolute() {
        return Err("absolute path required".into());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & if private { 0o077 } else { 0o022 } != 0
        || metadata.len() > 262144
    {
        return Err("untrusted configuration file".into());
    }
    let mut bytes = Vec::new();
    file.take(262145).read_to_end(&mut bytes)?;
    if bytes.len() > 262144 {
        return Err("configuration limit".into());
    }
    Ok(bytes)
}
async fn run() -> Result<(), Error> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("configuration path required")?);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let config: Configuration = serde_json::from_slice(&file(&path, true)?)?;
    let manifest = webdock_plugin_protocol::Manifest::parse(&file(&config.manifest, false)?)?;
    webdock_lumaserv_compute::validate_manifest(&manifest)?;
    let key: [u8; 32] = hex::decode(config.issuer_public_key_hex)?
        .try_into()
        .map_err(|_| "invalid key")?;
    let client = match config.core {
        Core::Unix { path } => HostClient::connect_unix(path).await?,
        Core::MutualTls {
            origin,
            domain,
            certificate,
            private_key,
            authority,
            server_certificate_sha256,
        } => HostClient::new(
            runtime::pinned_mtls_channel(
                origin,
                domain,
                runtime::PinnedMutualTls {
                    certificate: file(&certificate, false)?,
                    private_key: file(&private_key, true)?,
                    authority: file(&authority, false)?,
                    server_certificate_sha256,
                },
            )
            .await?,
        ),
    };
    let authority = runtime::RemoteAuthority::new(
        client.clone(),
        manifest.id().into(),
        config.generation,
        config.core_peers,
    )?;
    let provider = webdock_lumaserv_compute::Compute::new(client, config.projects)?;
    let runtime = runtime::Runtime::new(
        manifest,
        config.generation,
        ed25519_dalek::VerifyingKey::from_bytes(&key)?,
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
                    tokio::net::TcpListener::bind(address).await?,
                    runtime::MutualTls {
                        identity: tonic::transport::Identity::from_pem(
                            file(&certificate, false)?,
                            file(&private_key, true)?,
                        ),
                        peer_ca: tonic::transport::Certificate::from_pem(file(&authority, false)?),
                    },
                    shutdown,
                )
                .await?
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("compute_plugin_start_failed");
            ExitCode::FAILURE
        }
    }
}
