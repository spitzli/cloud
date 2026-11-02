//! Co-located demo runtime over private Unix sockets. Use the generic chart for remote mTLS plugins.
use serde::Deserialize;
use std::{
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
    listen: PathBuf,
    core: PathBuf,
    core_peers: Vec<runtime::CorePeer>,
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
    webdock_echo_demo::validate_manifest(&manifest)?;
    let key: [u8; 32] = hex::decode(config.issuer_public_key_hex)?
        .try_into()
        .map_err(|_| "invalid key")?;
    let client = HostClient::connect_unix(config.core).await?;
    let authority = runtime::RemoteAuthority::new(
        client,
        manifest.id().into(),
        config.generation,
        config.core_peers,
    )?;
    let runtime = runtime::Runtime::new(
        manifest,
        config.generation,
        ed25519_dalek::VerifyingKey::from_bytes(&key)?,
        Arc::new(authority),
        Arc::new(webdock_echo_demo::Echo),
    )?;
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    runtime
        .serve_unix(runtime::UnixSocket::bind(config.listen)?, shutdown)
        .await?;
    Ok(())
}
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("echo_demo_start_failed: {error}");
            ExitCode::FAILURE
        }
    }
}
