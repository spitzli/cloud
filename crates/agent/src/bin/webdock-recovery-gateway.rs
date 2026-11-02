//! Private read-only quarantine inspection; no public listener/config discovery.
#[cfg(feature = "docker")]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    let result = async {
        let mut args = std::env::args_os().skip(1);
        let path = std::path::PathBuf::from(args.next().ok_or("private gateway config required")?);
        if args.next().is_some() {
            return Err("unexpected gateway argument");
        }
        let bytes = webdock_agent::config::read_private(&path, 65536)
            .map_err(|_| "private gateway config unavailable")?;
        let config = serde_json::from_slice(&bytes).map_err(|_| "invalid gateway config")?;
        webdock_agent::recovery::gateway::serve(config)
            .await
            .map_err(|_| "quarantine gateway stopped")
    }
    .await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("recovery_gateway_failed_private_details_withheld");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(not(feature = "docker"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::FAILURE
}
