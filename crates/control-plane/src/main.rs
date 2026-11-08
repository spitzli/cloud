use webdock_control_plane::{
    app::build_runtime,
    config::runtime_config,
    runtime::{self, Error, Result},
};

fn main() -> Result<()> {
    let executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = executor.block_on(run());
    // A cancelled resolver/blocking task must not hold process teardown indefinitely.
    executor.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}

async fn run() -> Result<()> {
    let filter = match std::env::var("RUST_LOG") {
        Ok(value) => tracing_subscriber::EnvFilter::try_new(value)
            .map_err(|_| Error::Message("Invalid logging filter".into()))?,
        Err(_) => tracing_subscriber::EnvFilter::new("info"),
    };
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .try_init()
        .map_err(|_| Error::Message("Logging initialization failed".into()))?;
    let config = runtime_config(&std::env::vars().collect())?;
    let mut shutdown = Box::pin(runtime::shutdown_signal()?);
    let app = tokio::select! {
        result = build_runtime(&config) => result?,
        _ = &mut shutdown => return Ok(()),
    };
    let listener = tokio::select! {
        result = tokio::net::TcpListener::bind((config.server.binding.as_str(), config.server.port)) => result?,
        _ = &mut shutdown => return Ok(()),
    };
    tracing::info!(address=%listener.local_addr()?, "Spitzli Cloud HTTP runtime ready");
    runtime::serve_with_shutdown(listener, app, shutdown, config.server.shutdown_timeout).await?;
    Ok(())
}
