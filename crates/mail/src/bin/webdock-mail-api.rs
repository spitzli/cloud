//! Native Mail ingress. Bind locally behind the ingress proxy; internal routes authenticate
//! independently and must remain excluded from its public route table.
use serde::Deserialize;
use std::{
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use webdock_mail::{
    payload::PrivatePayloadStore, send_api::SendApi, send_control::HttpSendControl,
};
type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    port: u16,
    control_origin: String,
    control_secret: String,
    control_authority: Option<PathBuf>,
    payload_directory: PathBuf,
    payload_key: String,
    webmail_secret: Option<String>,
}
fn file(path: &Path, private: bool) -> Result<Vec<u8>, Error> {
    if !path.is_absolute() {
        return Err("Absolute Mail configuration path required".into());
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
        return Err("Untrusted Mail configuration file".into());
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("Mail configuration limit".into());
    }
    Ok(bytes)
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 || args[0] != "--config" {
        return Err("Usage: webdock-mail-api --config /absolute/private.json".into());
    }
    let config: Configuration = serde_json::from_slice(&file(Path::new(&args[1]), true)?)?;
    if config.port == 0 {
        return Err("Mail API port required".into());
    }
    let trust = config
        .control_authority
        .map(|path| reqwest::Certificate::from_pem(&file(&path, false)?).map_err(Error::from))
        .transpose()?
        .into_iter()
        .collect::<Vec<_>>();
    let control = Arc::new(HttpSendControl::new(
        &config.control_origin,
        &config.control_secret,
        &trust,
    )?);
    let store = Arc::new(PrivatePayloadStore::open(
        &config.payload_directory,
        &config.payload_key,
    )?);
    let router = match config.webmail_secret {
        Some(secret) => SendApi::with_internal(control, store, secret)?,
        None => SendApi::new(control, store),
    }
    .router();
    let listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, config.port)).await?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
        })
        .await?;
    Ok(())
}
