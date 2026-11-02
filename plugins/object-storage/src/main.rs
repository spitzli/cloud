//! Managed Object Storage native runtime with private durable metadata.
use serde::Deserialize;
use std::{path::PathBuf, process::ExitCode, sync::Arc};
use webdock_plugin_kit::transport::{CoreTls, ListenTls, RuntimeTransport};
use webdock_plugin_runtime::{self as runtime};
type Error = Box<dyn std::error::Error + Send + Sync>;

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
    backend: webdock_object_storage_plugin::sdk::BackendConfig,
}
async fn run() -> Result<(), Error> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("configuration path required")?);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let config: Configuration =
        serde_json::from_slice(&webdock_plugin_kit::read_file(&path, true)?)?;
    let transport = RuntimeTransport {
        listen: config.listen,
        core: config.core,
        listen_tls: config.listen_tls,
        core_tls: config.core_tls,
    };
    transport.validate()?;
    let manifest = webdock_plugin_protocol::Manifest::parse(&webdock_plugin_kit::read_file(
        &config.manifest,
        false,
    )?)?;
    webdock_object_storage_plugin::validate_manifest(&manifest)?;
    let ca = config
        .storage_ca
        .as_ref()
        .map(|path| webdock_plugin_kit::read_file(path, false))
        .transpose()?;
    let store =
        webdock_object_storage_plugin::store::Store::connect(&config.storage_url, ca.as_deref())
            .await?;
    let provider = webdock_object_storage_plugin::sdk::Control::new(config.backend).await?;
    let client = transport.core_client().await?;
    let plugin = Arc::new(webdock_object_storage_plugin::engine::Engine::new(
        store,
        webdock_plugin_kit::seal::Sealer::new(&config.secret_key_hex)?,
        Arc::new(provider),
    ));
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
        Arc::new(webdock_object_storage_plugin::Handler(plugin)),
    )?;
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
        Err(_) => {
            eprintln!("object_storage_plugin_start_failed");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_metadata_configuration_needs_no_local_volume() {
        let config = serde_json::json!({
            "manifest":"/run/webdock/runtime/manifest.json",
            "issuerPublicKeyHex":"a".repeat(64), "generation":2,
            "listen":"/run/webdock/runtime/plugin.sock", "core":"/run/webdock/runtime/core.sock",
            "corePeers":[], "storageUrl":"postgresql://metadata.invalid/object_storage",
            "storageCa":"/run/webdock/runtime/storage-ca.crt", "secretKeyHex":"b".repeat(64),
            "backend":{"id":"managed","kind":"wasabi","endpoint":"https://s3.eu-central-1.wasabisys.com",
                "region":"eu-central-1","account":"operator-bound-account","accessKey":"test-only","secretKey":"test-only"}
        });
        assert!(serde_json::from_value::<Configuration>(config.clone()).is_ok());
        let mut legacy = config;
        legacy.as_object_mut().unwrap().remove("storageUrl");
        legacy.as_object_mut().unwrap().remove("storageCa");
        legacy["metadataDirectory"] = serde_json::json!("/unprovisioned/metadata");
        assert!(serde_json::from_value::<Configuration>(legacy).is_err());
    }
}
