//! Protected export/diagnostic producer; no production activation is implicit.
use serde::Deserialize;
use std::{path::PathBuf, process::ExitCode, sync::Arc};
use webdock_plugin_kit::transport::{CoreTls, ListenTls, RuntimeTransport};
use webdock_plugin_runtime::{self as runtime};
type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(Debug)]
struct StartupFailure(&'static str);
impl std::fmt::Display for StartupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for StartupFailure {}

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
    backend: webdock_archives::backend::BackendConfig,
    staging_directory: PathBuf,
    #[serde(default)]
    database_sources: Vec<webdock_archives::producer::DatabaseSource>,
}
async fn run() -> Result<(), Error> {
    webdock_archives::initialize_crypto();
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("configuration path required")?);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let config: Configuration = serde_json::from_slice(
        &webdock_plugin_kit::read_file(&path, true)
            .map_err(|_| StartupFailure("configuration_file"))?,
    )
    .map_err(|_| StartupFailure("configuration"))?;
    if !config.database_sources.is_empty() {
        return Err(StartupFailure("legacy_database_sources").into());
    }
    let transport = RuntimeTransport {
        listen: config.listen,
        core: config.core,
        listen_tls: config.listen_tls,
        core_tls: config.core_tls,
    };
    transport
        .validate()
        .map_err(|_| StartupFailure("transport_configuration"))?;
    let manifest = webdock_plugin_protocol::Manifest::parse(&webdock_plugin_kit::read_file(
        &config.manifest,
        false,
    )?)?;
    webdock_archives::validate_manifest(&manifest).map_err(|_| StartupFailure("manifest"))?;
    let ca = config
        .storage_ca
        .as_ref()
        .map(|path| webdock_plugin_kit::read_file(path, false))
        .transpose()?;
    webdock_archives::prepare_staging(&config.staging_directory)
        .map_err(|_| StartupFailure("staging"))?;
    let backend = Arc::new(
        webdock_archives::backend::Backend::new(config.backend)
            .await
            .map_err(|code| {
                StartupFailure(match code {
                    "Archive bucket policy must be private and absent" => "storage_policy",
                    "Storage retention unavailable" => "storage_versioning",
                    "Archive retention policy unavailable" => "storage_lifecycle",
                    "Bounded archive retention required" => "storage_retention",
                    "Pinned archive fixture CA required"
                    | "Pinned archive fixture CA unavailable"
                    | "Archive fixture CA mismatch" => "storage_fixture_ca",
                    "Invalid bounded archive fixture profile" => "storage_fixture_profile",
                    _ => "storage_configuration",
                })
            })?,
    );
    let engine = Arc::new(
        webdock_archives::Engine::connect(
            &config.storage_url,
            ca.as_deref(),
            backend,
            config.staging_directory,
            config.database_sources,
        )
        .await
        .map_err(|_| StartupFailure("metadata"))?,
    );
    let client = transport
        .core_client()
        .await
        .map_err(|_| StartupFailure("core_tls"))?;
    let plugin = Arc::new(webdock_archives::Handler {
        engine,
        host: client.clone(),
    });
    let key: [u8; 32] = hex::decode(config.issuer_public_key_hex)?
        .try_into()
        .map_err(|_| "invalid key")?;
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
        plugin.clone(),
    )?
    .with_jobs(plugin);
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    match (&transport.listen, &transport.listen_tls) {
        (Some(path), None) => {
            runtime
                .serve_unix(runtime::UnixSocket::bind(path)?, shutdown)
                .await?
        }
        (None, Some(tls)) => {
            runtime
                .serve_mtls(
                    tokio::net::TcpListener::bind(tls.address).await?,
                    tls.material()?,
                    shutdown,
                )
                .await?
        }
        _ => return Err("listener required".into()),
    }
    Ok(())
}
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if let Some(stage) = error.downcast_ref::<StartupFailure>() {
                eprintln!("archives_plugin_start_failed_{}", stage.0);
            } else {
                eprintln!("archives_plugin_start_failed");
            }
            ExitCode::FAILURE
        }
    }
}
