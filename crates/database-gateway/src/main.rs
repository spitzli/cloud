use std::{path::PathBuf, sync::Arc, time::Duration};
use webdock_database_gateway::{
    control::HttpControl,
    managed::{ManagedRuntimes, ProcessConfig, ProcessManager},
    server::{Gateway, Options},
};
type Error = Box<dyn std::error::Error + Send + Sync>;
#[derive(clap::Parser)]
struct Args {
    #[arg(long)]
    healthcheck: bool,
}
fn required(name: &str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| format!("Missing {name}").into())
}
#[tokio::main]
async fn main() -> Result<(), Error> {
    use clap::Parser;
    if Args::parse().healthcheck {
        let port: u16 = std::env::var("PORT")
            .unwrap_or_else(|_| "3137".into())
            .parse()
            .map_err(|_| "Invalid listen port")?;
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()?
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err("Database gateway unavailable".into());
        }
        return Ok(());
    }
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let managed: Option<Arc<dyn ManagedRuntimes>> =
        if std::env::var("WEBDOCK_DATABASE_MANAGED_RUNTIMES").as_deref() == Ok("true") {
            Some(Arc::new(ProcessManager::new(ProcessConfig {
                executable: PathBuf::from(
                    std::env::var("TABULARIS_EXECUTABLE")
                        .unwrap_or_else(|_| "/usr/local/bin/tabularis".into()),
                ),
                web_root: Some(PathBuf::from(
                    std::env::var("TABULARIS_WEB_ROOT")
                        .unwrap_or_else(|_| "/opt/tabularis/web".into()),
                )),
                temporary_directory: PathBuf::from(
                    std::env::var("WEBDOCK_DATABASE_RUNTIME_DIRECTORY")
                        .unwrap_or_else(|_| "/tmp".into()),
                ),
                capacity: std::env::var("WEBDOCK_DATABASE_RUNTIME_CAPACITY")
                    .unwrap_or_else(|_| "8".into())
                    .parse()
                    .map_err(|_| "Invalid runtime capacity")?,
                startup_timeout: Duration::from_secs(10),
            })?))
        } else {
            None
        };
    let expected = required("TABULARIS_EXPECTED_COMMIT")?;
    if expected.len() != 40 || !expected.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err("Expected runtime source commit must be a full Git SHA1".into());
    }
    let ca_path = std::env::var("WEBDOCK_DATABASE_RUNTIME_CA_FILE")
        .or_else(|_| std::env::var("NODE_EXTRA_CA_CERTS"))
        .ok();
    let pem = if let Some(path) = ca_path {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        Some(bytes)
    } else {
        None
    };
    let trust = webdock_database_gateway::trust::Trust::from_pem(pem.as_deref())?;
    let gateway = Gateway::new_with_trust(
        Options {
            public_origin: required("WEBDOCK_DATABASE_GATEWAY_ORIGIN")?,
            allowed_origins: required("WEBDOCK_DATABASE_BROWSER_ORIGINS")?
                .split(',')
                .map(str::to_owned)
                .collect(),
            runtime_origins: std::env::var("WEBDOCK_DATABASE_RUNTIME_ORIGINS")
                .unwrap_or_default()
                .split(',')
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect(),
            expected_commit: Some(expected),
            control: Arc::new(HttpControl::new_with_trust(
                &required("WEBDOCK_AUTH_ORIGIN")?,
                required("WEBDOCK_DATABASE_GATEWAY_SECRET")?,
                &trust,
            )?),
            managed,
        },
        trust,
    )?;
    let address: std::net::IpAddr = std::env::var("HOST")
        .unwrap_or_else(|_| "127.0.0.1".into())
        .parse()
        .map_err(|_| "Invalid listen address")?;
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3137".into())
        .parse()
        .map_err(|_| "Invalid listen port")?;
    let listener = tokio::net::TcpListener::bind((address, port)).await?;
    let shutdown_gateway = gateway.clone();
    let shutdown = async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
        shutdown_gateway.shutdown().await;
    };
    axum::serve(listener, gateway.router())
        .with_graceful_shutdown(shutdown)
        .await?;
    gateway.shutdown().await;
    Ok(())
}
