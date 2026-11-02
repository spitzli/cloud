//! Reviewed first-install operator and Console provisioning; never run at normal startup.
use clap::Parser;
use std::{path::PathBuf, process::ExitCode};
#[derive(Parser)]
#[command(about = "Atomically bootstrap the first operator, Portal client and private output")]
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
    match webdock_control_plane::bootstrap::bootstrap_operator(
        &std::env::vars().collect(),
        &args.target,
        &args.config,
        &args.output,
    )
    .await
    {
        Ok(summary) => {
            match serde_json::to_string(&summary) {
                Ok(summary) => println!("{summary}"),
                Err(_) => {
                    eprintln!("operator_bootstrap_commit_uncertain");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(webdock_control_plane::bootstrap::BootstrapError::CommitUncertain) => {
            eprintln!(
                "operator_bootstrap_commit_uncertain: retain private output and inspect recorded IDs before retrying"
            );
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("operator_bootstrap_failed");
            ExitCode::FAILURE
        }
    }
}
