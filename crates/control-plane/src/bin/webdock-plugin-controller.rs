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
fn next_idle_poll(previous: Duration, worked: bool) -> Duration {
    if worked {
        Duration::from_millis(500)
    } else {
        previous.saturating_mul(2).min(Duration::from_secs(5))
    }
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
                neon_connections: None,
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
    let mut idle_delay = Duration::from_millis(500);
    loop {
        let mut worked = false;
        if let Some(result) = jobs.try_join_next() {
            worked |= result??;
        }
        if jobs.is_empty() {
            let endpoints = endpoints.clone();
            jobs.spawn(async move { endpoints.advance_job().await });
        }
        if let Some(lease) = endpoints.host.claim_state_operation(None).await? {
            worked = true;
            endpoints
                .host
                .advance_state_operation(&lease, endpoints.as_ref())
                .await?;
        }
        if let Some(installer) = &installer {
            worked |= installer.advance(&endpoints).await?;
        }
        if args.once {
            while let Some(result) = jobs.join_next().await {
                result??;
            }
            return Ok(());
        }
        // Only empty discovery slows down; active jobs keep their existing authority/lease checks.
        idle_delay = next_idle_poll(idle_delay, worked);
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result?; return Ok(()); },
            _ = tokio::time::sleep(idle_delay) => {},
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

#[cfg(test)]
mod idle_poll_tests {
    use super::*;

    #[test]
    fn empty_cycles_back_off_but_work_restores_the_original_cadence() {
        let mut delay = Duration::from_millis(500);
        let mut observed = Vec::new();
        for _ in 0..6 {
            delay = next_idle_poll(delay, false);
            observed.push(delay.as_millis());
        }
        assert_eq!(observed, [1000, 2000, 4000, 5000, 5000, 5000]);
        assert_eq!(next_idle_poll(delay, true), Duration::from_millis(500));
        assert_eq!(next_idle_poll(Duration::MAX, false), Duration::from_secs(5));
    }

    #[test]
    fn twenty_idle_seconds_need_six_discovery_passes_instead_of_forty() {
        let mut elapsed = Duration::ZERO;
        let mut delay = Duration::from_millis(500);
        let mut passes = 0;
        while elapsed < Duration::from_secs(20) {
            passes += 1;
            delay = next_idle_poll(delay, false);
            elapsed += delay;
        }
        assert_eq!(passes, 6);
        assert_eq!(Duration::from_secs(20).as_millis() / 500, 40);
        assert_eq!(next_idle_poll(delay, true), Duration::from_millis(500));
    }
}
