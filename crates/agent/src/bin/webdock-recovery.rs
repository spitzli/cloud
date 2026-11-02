//! Operator-only independent cold restore. Never creates a browser or send authority.
#[cfg(feature = "docker")]
use clap::{Parser, Subcommand};
#[cfg(feature = "docker")]
#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    config: std::path::PathBuf,
    #[command(subcommand)]
    action: Action,
}
#[cfg(feature = "docker")]
#[derive(Subcommand)]
enum Action {
    Validate,
    Capabilities,
    TargetIdentity {
        #[arg(long)]
        target: String,
    },
    Issue {
        #[arg(long)]
        admitted_set: std::path::PathBuf,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        operator_key: std::path::PathBuf,
        #[arg(long, default_value_t = 300)]
        seconds: u64,
        #[arg(long)]
        output: std::path::PathBuf,
    },
    Restore {
        #[arg(long)]
        capsule: std::path::PathBuf,
        #[arg(long)]
        target: String,
        #[arg(long)]
        receipt: std::path::PathBuf,
    },
}
#[cfg(feature = "docker")]
fn write(
    path: &std::path::Path,
    value: &impl serde::Serialize,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(&serde_json::to_vec(value)?)?;
    f.sync_all()?;
    std::fs::File::open(path.parent().ok_or("private output parent required")?)?.sync_all()?;
    Ok(())
}
#[cfg(feature = "docker")]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    use webdock_agent::recovery::{
        NativeRecoveryExecutor,
        capsule::{ColdRecoveryCapsule, SignedDocument, seal_operator_capsule},
        config::RecoveryRuntimeConfig,
    };
    let args = Arguments::parse();
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let config = RecoveryRuntimeConfig::load(&args.config)?;
        let runtime = NativeRecoveryExecutor::open(args.config).await?;
        match args.action {
            Action::Validate => {}
            Action::Capabilities => println!("{}", runtime.capabilities()?),
            Action::TargetIdentity { target } => println!("{}", runtime.target_identity(&target)?),
            Action::Issue {
                admitted_set,
                actor,
                target,
                operator_key,
                seconds,
                output,
            } => {
                if !(1..=300).contains(&seconds) {
                    return Err("bounded cold deadline required".into());
                }
                let admitted_set: SignedDocument = serde_json::from_slice(
                    &webdock_agent::config::read_private(&admitted_set, 262144)?,
                )?;
                let descriptor = config
                    .targets
                    .iter()
                    .find(|t| t.id == target)
                    .ok_or("operator-installed target required")?;
                let now = chrono::Utc::now().timestamp();
                let capsule = ColdRecoveryCapsule {
                    version: 1,
                    admitted_set,
                    actor_id: actor,
                    target_profile_id: target.clone(),
                    target_identity_sha256: runtime.target_identity(&target)?,
                    target_nonce: descriptor.nonce.clone(),
                    capacity_bytes: descriptor.capacity_bytes,
                    issued_at: now,
                    expires_at: now + seconds as i64,
                    quarantine_only: true,
                };
                let signed =
                    seal_operator_capsule(&capsule, &operator_key, &config.staging_directory)?;
                write(&output, &signed)?;
            }
            Action::Restore {
                capsule,
                target,
                receipt,
            } => {
                let capsule: SignedDocument = serde_json::from_slice(
                    &webdock_agent::config::read_private(&capsule, 262144)?,
                )?;
                let result = runtime.execute_cold(&capsule, &target).await?;
                write(&receipt, &result)?;
            }
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("native_recovery_failed_private_details_withheld");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(not(feature = "docker"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::FAILURE
}
