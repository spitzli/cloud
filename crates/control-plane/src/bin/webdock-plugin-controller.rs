//! Reconciles persisted plugin lifecycle commands and durable job attempts.
use clap::Parser;
use std::{path::PathBuf, process::ExitCode, sync::Arc, time::Duration};
use webdock_control_plane::{
    plugin_config::{PluginConfig, authority_services},
    plugins::{PluginDependencies, PluginEndpoints},
};
#[derive(Parser)]
struct Args {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    once: bool,
    #[arg(long)]
    installation_config: Option<PathBuf>,
}
type Error = Box<dyn std::error::Error + Send + Sync>;
fn required(key: &str) -> Result<String, Error> {
    std::env::var(key).map_err(|_| format!("Missing {key}").into())
}
async fn run() -> Result<(), Error> {
    let args = Args::parse();
    let config = PluginConfig::load(&args.config)?;
    let platform = webdock_core::config::PlatformConfig::from_map(&std::env::vars().collect())?;
    let auth_url = required("WEBDOCK_DATABASE_URL")?;
    let auth = webdock_control_plane::db::connect_runtime(
        &auth_url,
        "webdock_auth_bootstrap",
        platform.production,
    )
    .await?;
    let business = webdock_control_plane::db::connect_runtime(
        &required("WEBDOCK_SESSION_DATABASE_URL")?,
        "webdock_auth_runtime",
        platform.production,
    )
    .await?;
    let worker = webdock_control_plane::db::connect_runtime(
        &required("WEBDOCK_WORKER_DATABASE_URL")?,
        "webdock_worker_runtime",
        platform.production,
    )
    .await?;
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2)?,
        )
        .await?,
    );
    let authority = authority_services(auth.clone(), ids.clone(), &platform).await?;
    let endpoints = Arc::new(
        PluginEndpoints::initialize(
            PluginDependencies {
                business: business.clone(),
                worker,
                auth,
                identity: authority.identity,
                browser: authority.browser,
                oauth: authority.oauth,
                ids,
                rls_secret: required("DATABASE_RLS_SECRET")?,
                database_new_orders_enabled: platform.database_new_orders_enabled,
            },
            &config,
        )
        .await?,
    );
    let installer = match args.installation_config {
        Some(path) => Some(
            webdock_control_plane::plugins::installation::worker::InstallationWorker::load(
                &path, &business, &config,
            )
            .await?,
        ),
        None => None,
    };
    // Dropping the set aborts requests on shutdown; their persisted leases remain recoverable.
    let mut jobs = tokio::task::JoinSet::new();
    loop {
        if let Some(result) = jobs.try_join_next() {
            result??;
        }
        if jobs.is_empty() {
            let endpoints = endpoints.clone();
            jobs.spawn(async move { endpoints.advance_job().await });
        }
        if let Some(lease) = endpoints.host.claim_state_operation(None).await? {
            endpoints
                .host
                .advance_state_operation(&lease, endpoints.as_ref())
                .await?;
        }
        if let Some(installer) = &installer {
            installer.advance(&endpoints).await?;
        }
        if args.once {
            while let Some(result) = jobs.join_next().await {
                result??;
            }
            return Ok(());
        }
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result?; return Ok(()); },
            _ = tokio::time::sleep(Duration::from_millis(500)) => {},
        }
    }
}
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("plugin_controller_failed");
            ExitCode::FAILURE
        }
    }
}
