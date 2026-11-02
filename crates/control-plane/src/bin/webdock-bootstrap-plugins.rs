//! Reviewed offline global plugin-kernel setup. Never invoked by serving Core.
use clap::Parser;
use std::{path::PathBuf, process::ExitCode};
#[derive(Parser)]
struct Args {
    #[arg(long)]
    target: String,
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    output: PathBuf,
}
#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match webdock_control_plane::plugin_bootstrap::bootstrap_plugins(
        &std::env::vars().collect(),
        &args.target,
        &args.config,
        &args.output,
    )
    .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("plugin_bootstrap_failed");
            ExitCode::FAILURE
        }
    }
}
