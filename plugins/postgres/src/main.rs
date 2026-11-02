//! PostgreSQL plugin runtime over private Unix sockets, with its own storage database.
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
    shared_servers: Vec<webdock_postgres::shared::SharedServer>,
    dedicated: Option<webdock_postgres::dedicated::DedicatedConfig>,
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
    webdock_postgres::validate_manifest(&manifest)?;
    for server in &config.shared_servers {
        webdock_plugin_kit::loopback(&server.admin_url)?;
    }
    let store = match config.storage_ca {
        Some(path) => {
            webdock_postgres::store::Store::connect_tls(
                &config.storage_url,
                &webdock_plugin_kit::read_file(&path, false)?,
            )
            .await?
        }
        None => webdock_postgres::store::Store::connect(&config.storage_url).await?,
    };
    for server in &config.shared_servers {
        server.harden().await?;
    }
    let client = transport.core_client().await?;
    let placement = config.dedicated.map(|dedicated| {
        let workloads = webdock_postgres::dedicated::HostWorkloads {
            client: client.clone(),
            plugin: manifest.id().into(),
            generation: config.generation,
        };
        webdock_postgres::dedicated::Placement::new(Box::new(workloads), dedicated)
    });
    let plugin = webdock_postgres::Plugin::new(
        store,
        webdock_postgres::seal::Sealer::new(&config.secret_key_hex)?,
        config.shared_servers,
        placement,
    );
    plugin.resume().await?;
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
        Arc::new(webdock_postgres::Handler(plugin)),
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
            eprintln!("postgres_plugin_start_failed");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_tls_configuration_does_not_require_unix_paths() {
        let config = serde_json::json!({
            "manifest":"/run/webdock/runtime/manifest.json","issuerPublicKeyHex":"a".repeat(64),"generation":1,
            "listenTls":{"address":"0.0.0.0:8443","certificate":"/run/webdock/runtime/runtime.crt","privateKey":"/run/webdock/runtime/runtime.key","authority":"/run/webdock/runtime/core-ca.crt"},
            "coreTls":{"origin":"https://core.test:8443","domain":"core.test","certificate":"/run/webdock/runtime/runtime.crt","privateKey":"/run/webdock/runtime/runtime.key","authority":"/run/webdock/runtime/core-ca.crt","serverCertificateSha256":"b".repeat(64)},
            "corePeers":[{"kind":"mutualTls","certificateSha256":"b".repeat(64)}],
            "storageUrl":"postgresql://private.invalid/metadata","storageCa":"/run/webdock/runtime/storage-ca.crt",
            "secretKeyHex":"c".repeat(64),"sharedServers":[],"dedicated":{"domain":"db.test","port":5432,"entryPoints":["postgres"]}
        });
        serde_json::from_value::<Configuration>(config)
            .map(|_| ())
            .unwrap();
    }
}
