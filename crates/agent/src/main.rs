use clap::{Parser, Subcommand};
use std::path::PathBuf;
use webdock_agent::{config::AgentConfig, management::Daemon};
#[derive(Parser)]
#[command(name = "webdock-agent", about = "Webdock enrolled runtime executor")]
struct Arguments {
    #[arg(long)]
    config: PathBuf,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    Validate,
    Run,
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();
    let args = Arguments::parse();
    let result = async {
        let config = AgentConfig::load(&args.config)?;
        match args.action {
            Action::Validate => webdock_agent::management::validate_configuration(&config),
            Action::Run => {
                Daemon::open(config)
                    .await?
                    .run(async {
                        let _ = tokio::signal::ctrl_c().await;
                    })
                    .await
            }
        }
    }
    .await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
