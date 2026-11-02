use std::{io::Read, sync::Arc, time::Duration};
use webdock_mail::{
    gateway::{Gateway, GatewayOptions},
    gateway_control::HttpControl,
};
fn required(name: &str) -> Result<String, &'static str> {
    std::env::var(name).map_err(|_| "Missing Mail gateway configuration")
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "3138".into())
        .parse::<u16>()?;
    if port < 1024 {
        return Err("Invalid Mail gateway port".into());
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["--healthcheck"] {
        let response = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()?
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err("Mail gateway is not healthy".into());
        }
        return Ok(());
    }
    if !args.is_empty() {
        return Err("Unsupported Mail gateway option".into());
    }
    let mut trust = Vec::new();
    if let Ok(path) = std::env::var("WEBDOCK_MAIL_CONTROL_CA_FILE") {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(1_048_577)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 1_048_576 || bytes.windows(11).any(|w| w == b"PRIVATE KEY") {
            return Err("Invalid Mail control trust bundle".into());
        }
        trust = reqwest::Certificate::from_pem_bundle(&bytes)?;
        if trust.is_empty() || trust.len() > 32 {
            return Err("Invalid Mail control trust bundle".into());
        }
    }
    let control = Arc::new(HttpControl::new(
        &required("WEBDOCK_AUTH_ORIGIN")?,
        &std::env::var("MAIL_SEND_INTERNAL_ORIGIN")
            .unwrap_or_else(|_| "http://127.0.0.1:3139".into()),
        &required("MAIL_GATEWAY_SECRET")?,
        &trust,
    )?);
    let options = GatewayOptions::new(
        required("MAIL_BROWSER_ORIGINS")?
            .split(',')
            .map(str::to_owned)
            .collect(),
        control,
    )?;
    let router = Gateway::new(options)?.router();
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, router).with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    });
    let mut task = tokio::spawn(async move { server.await });
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {result=&mut task=>{result??;return Ok(());},result=tokio::signal::ctrl_c()=>{result?;},_=terminate.recv()=>{}}
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    let _ = shutdown_tx.send(());
    tokio::time::timeout(Duration::from_secs(30), task).await???;
    Ok(())
}
