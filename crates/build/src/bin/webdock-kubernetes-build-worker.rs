//! Trusted control-plane daemon. This executable never launches a VM or customer commands locally.
use std::process::ExitCode;
use webdock_build::{Error, Result, kubernetes_worker::KubernetesWorkerConfig};
fn fail(code: &'static str) -> Error {
    Error {
        code,
        logs: String::new(),
    }
}
async fn execute() -> Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--config") {
        return Err(fail("invalid_worker_arguments"));
    }
    let path = args
        .next()
        .ok_or_else(|| fail("invalid_worker_arguments"))?;
    let once = match args.next().as_deref() {
        Some("--once") => true,
        None => false,
        _ => return Err(fail("invalid_worker_arguments")),
    };
    if args.next().is_some() {
        return Err(fail("invalid_worker_arguments"));
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| fail("worker_signal_unavailable"))?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| fail("worker_signal_unavailable"))?;
    let worker = KubernetesWorkerConfig::load(std::path::Path::new(&path))?
        .build()
        .await?;
    worker
        .run(once, async move {
            tokio::select! {_=terminate.recv()=>{},_=interrupt.recv()=>{}}
        })
        .await
}
#[tokio::main]
async fn main() -> ExitCode {
    match execute().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", error.code);
            if error.code == "invalid_worker_arguments" {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
