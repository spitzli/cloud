//! Explicit offline agent transport enrollment. No runtime self-enrollment or replacement.
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
    match webdock_control_plane::plugin_bootstrap::enroll_agent(
        &std::env::vars().collect(),
        &args.target,
        &args.config,
        &args.output,
    )
    .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(webdock_control_plane::plugin_bootstrap::SetupError::CommitUncertain) => {
            eprintln!("agent_enrollment_commit_uncertain");
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("agent_enrollment_failed");
            ExitCode::FAILURE
        }
    }
}
