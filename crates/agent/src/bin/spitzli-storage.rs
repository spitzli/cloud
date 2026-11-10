use clap::{Parser, Subcommand};
use spitzli_agent::storage::{StorageKind, StoragePool, serve_kind_with_recovery};
use std::path::PathBuf;
#[derive(Parser)]
#[command(about = "Separately privileged Webdock fixed filesystem helper")]
struct Arguments {
    #[arg(long)]
    config: PathBuf,
    /// Serve a separate PostgreSQL installation pool (UID/GID 26).
    #[arg(long, conflicts_with = "builds")]
    database: bool,
    /// Dedicated build pool (UID/GID 1000); legacy unsigned RPC is disabled.
    #[arg(long, conflicts_with = "recovery_trust")]
    builds: bool,
    /// Root-installed public signer/profile trust for isolated recovery, disabled by default.
    #[arg(long)]
    recovery_trust: Option<PathBuf>,
    #[command(subcommand)]
    action: Action,
}
#[derive(Subcommand)]
enum Action {
    Initialize,
    Restore,
    /// Verify a root-installed public trust and short-lived signed attempt request.
    BuildApply {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        trust: PathBuf,
    },
    Serve {
        #[arg(long)]
        socket: PathBuf,
    },
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();
    let arguments = Arguments::parse();
    let kind = if arguments.database {
        StorageKind::Database
    } else if arguments.builds {
        StorageKind::Builds
    } else {
        StorageKind::Apps
    };
    let result = match arguments.action {
        Action::Initialize => StoragePool::initialize(&arguments.config)
            .and_then(|()| StoragePool::open_kind(&arguments.config, kind).map(|_| ())),
        Action::Restore => {
            StoragePool::open_kind(&arguments.config, kind).and_then(|pool| pool.restore())
        }
        Action::BuildApply { request, trust } => {
            if kind != StorageKind::Builds {
                Err(spitzli_supervisor::ExecutionError::Denied)
            } else {
                StoragePool::open_kind(&arguments.config, kind).and_then(|mut pool| {
                    let guard = spitzli_agent::storage::StorageGuard::new(
                        chrono::Utc::now() + chrono::Duration::seconds(120),
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    );
                    let result = pool.apply_build_files(&request, &trust, &guard)?;
                    println!(
                        "{}",
                        serde_json::to_string(&result)
                            .map_err(|_| spitzli_supervisor::ExecutionError::Runtime)?
                    );
                    Ok(())
                })
            }
        }
        Action::Serve { socket } => {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("signal handler");
            serve_kind_with_recovery(
                &arguments.config,
                &socket,
                kind,
                arguments.recovery_trust.as_deref(),
                async {
                    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
                },
            )
            .await
        }
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
