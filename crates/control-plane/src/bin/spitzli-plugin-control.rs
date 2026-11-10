//! Independently supervised private plugin host/callback listener. It never installs or enrolls itself.
use clap::Parser;
use spitzli_control_plane::{
    plugin_config::{ControlListener, PluginConfig, authority_services},
    plugins::{PluginDependencies, PluginEndpoints},
};
use std::{path::PathBuf, sync::Arc};
#[derive(Parser)]
struct Args {
    #[arg(long)]
    config: PathBuf,
}
type Error = Box<dyn std::error::Error + Send + Sync>;
fn required(key: &str) -> Result<String, Error> {
    std::env::var(key).map_err(|_| format!("Missing {key}").into())
}
async fn run() -> Result<(), Error> {
    let args = Args::parse();
    let config = PluginConfig::load(&args.config)?;
    let environment = std::env::vars().collect();
    let platform = spitzli_core::config::PlatformConfig::from_map(&environment)?;
    let auth_url = required("SPITZLI_DATABASE_URL")?;
    let auth = spitzli_control_plane::db::connect_runtime(
        &auth_url,
        "webdock_auth_bootstrap",
        platform.production,
    )
    .await?;
    let business = spitzli_control_plane::db::connect_runtime(
        &required("SPITZLI_SESSION_DATABASE_URL")?,
        "webdock_auth_runtime",
        platform.production,
    )
    .await?;
    let worker = spitzli_control_plane::db::connect_runtime(
        &required("SPITZLI_WORKER_DATABASE_URL")?,
        "webdock_worker_runtime",
        platform.production,
    )
    .await?;
    let ids = Arc::new(
        spitzli_persistence::IdAllocator::connect(
            &auth_url,
            spitzli_ids::Identity::platform(0, 2)?,
        )
        .await?,
    );
    let authority = authority_services(auth.clone(), ids.clone(), &platform).await?;
    let neon_connections = Arc::new(spitzli_provider_connections::Connections::new(
        business.clone(),
        auth.clone(),
        authority.identity.clone(),
        authority.browser.clone(),
        &required("DATABASE_RLS_SECRET")?,
        spitzli_control_plane::neon_connections::encryption_key(&environment)?,
    )?);
    let mail_dependencies = spitzli_mail::MailDependencies {
        auth: auth.clone(),
        business: business.clone(),
        worker: worker.clone(),
        identity: authority.identity.clone(),
        browser: authority.browser.clone(),
        ids: ids.clone(),
    };
    let services = PluginEndpoints::initialize(
        PluginDependencies {
            business,
            worker,
            auth,
            identity: authority.identity,
            browser: authority.browser,
            oauth: authority.oauth,
            ids,
            rls_secret: required("DATABASE_RLS_SECRET")?,
            database_new_orders_enabled: platform.database_new_orders_enabled,
            neon_connections: Some(neon_connections),
        },
        &config,
    )
    .await?;
    let mail = if spitzli_control_plane::mail::native_control_configured(
        config.mail.as_ref(),
        &std::env::vars().collect(),
    ) && let Some(configuration) = &config.mail
    {
        Some(
            spitzli_control_plane::mail::initialize_sender(
                mail_dependencies.clone(),
                services.host.clone(),
                configuration,
                &std::env::vars().collect(),
                None,
            )
            .await?,
        )
    } else {
        None
    };
    let native_mail = match &mail {
        Some(sender) => Some(
            Arc::new(spitzli_mail::managed_control::ManagedControl::new(
                sender.clone(),
                Arc::new(
                    spitzli_mail::submissions::Submissions::new(
                        mail_dependencies,
                        &required("DATABASE_RLS_SECRET")?,
                        true,
                    )
                    .await?,
                ),
                services.host.clone(),
            ))
            .service(),
        ),
        None => None,
    };
    let mail = mail.map(|sender| sender.service());
    let shutdown = async {
        #[cfg(unix)]
        {
            let mut signal =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("signal handler");
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=signal.recv()=>{}}
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    };
    let mut server = tonic::transport::Server::builder()
        .concurrency_limit_per_connection(32)
        .timeout(std::time::Duration::from_secs(35));
    let exports = services.host.clone().export_service(
        services
            .exports
            .unwrap_or_else(|| Arc::new(spitzli_plugin_host::UnconfiguredExportSources)),
    );
    let repository = services
        .repository
        .map(|repository| repository.authority_service());
    match &config.control {
        ControlListener::Unix { path } => {
            let mut socket = spitzli_plugin_runtime::UnixSocket::bind(path)?;
            let incoming = socket.incoming()?;
            server
                .add_service(services.host.rpc())
                .add_optional_service(repository)
                .add_service(exports)
                .add_optional_service(mail)
                .add_optional_service(native_mail)
                .serve_with_incoming_shutdown(incoming, shutdown)
                .await?;
            drop(socket);
        }
        ControlListener::MutualTls { address, .. } => {
            let tls = config.control.tls()?.ok_or("Mutual TLS is required")?;
            let listener = tokio::net::TcpListener::bind(address).await?;
            server
                .tls_config(
                    tonic::transport::ServerTlsConfig::new()
                        .identity(tls.identity)
                        .client_ca_root(tls.peer_ca),
                )?
                .add_service(services.host.rpc())
                .add_optional_service(repository)
                .add_service(exports)
                .add_optional_service(mail)
                .add_optional_service(native_mail)
                .serve_with_incoming_shutdown(
                    tokio_stream::wrappers::TcpListenerStream::new(listener),
                    shutdown,
                )
                .await?;
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("plugin_control_failed");
        std::process::exit(1);
    }
}
