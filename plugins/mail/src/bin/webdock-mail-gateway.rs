//! Explicit operator cutover entry point. No provider/private database credentials.
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf, process::ExitCode, sync::Arc};
use webdock_plugin_kit::{read_file, transport::CoreTls};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    listen: SocketAddr,
    runtime: CoreTls,
    preauthentication: Preauthentication,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preauthentication {
    origin: String,
    secret_file: PathBuf,
    authority: PathBuf,
}
async fn run() -> Result<(), webdock_plugin_kit::Error> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("configuration path required")?);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let config: Configuration = serde_json::from_slice(&read_file(&path, true)?)?;
    let transport = webdock_plugin_kit::transport::RuntimeTransport {
        listen: Some(PathBuf::from("/unused/listen")),
        core_tls: Some(config.runtime),
        ..Default::default()
    };
    transport.validate()?;
    let channel = transport.core_client_lazy().await?.channel();
    let secret = String::from_utf8(read_file(&config.preauthentication.secret_file, true)?)?;
    let ca =
        reqwest::Certificate::from_pem(&read_file(&config.preauthentication.authority, false)?)?;
    let control = webdock_mail_runtime::send_control::HttpSendControl::new(
        &config.preauthentication.origin,
        secret.trim(),
        &[ca],
    )?;
    let router = webdock_mail_runtime::gateway::Gateway::new(Arc::new(control), channel).router();
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("mail_gateway_start_failed");
            ExitCode::FAILURE
        }
    }
}
