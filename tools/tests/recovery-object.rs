//! Operator fixture transport; uses the same verified multipart adapter as native storage.
use serde::Deserialize;
use std::{os::unix::fs::OpenOptionsExt, path::PathBuf};
use webdock_object_storage::{Credentials, ObjectReceipt, S3Config, Store, MAX_TRANSFER};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration { config: S3Config, credentials: Credentials }
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 5 { return Err("expected mode/config/scope/file/receipt".into()); }
    let bytes = webdock_plugin_kit::read_file(&PathBuf::from(&args[1]), true).map_err(|_| "private config unavailable")?;
    let config: Configuration = serde_json::from_slice(&bytes)?;
    let store = Store::loopback_test(config.config, config.credentials)?;
    let scope = args[2].to_str().ok_or("invalid scope")?;
    match args[0].to_str() {
        Some("upload") => {
            let file = tokio::fs::File::open(&args[3]).await?;
            let receipt = store.upload(scope, file, MAX_TRANSFER).await?;
            let mut output = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&args[4])?;
            std::io::Write::write_all(&mut output, &serde_json::to_vec(&receipt)?)?;
            output.sync_all()?;
        }
        Some("download") => {
            let receipt: ObjectReceipt = serde_json::from_slice(&webdock_plugin_kit::read_file(&PathBuf::from(&args[4]), true).map_err(|_| "private receipt unavailable")?)?;
            let mut output = tokio::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&args[3]).await?;
            if let Err(error) = store.download(scope, &receipt, &mut output, MAX_TRANSFER).await {
                drop(output); tokio::fs::remove_file(&args[3]).await?; return Err(error.into());
            }
            output.sync_all().await?;
        }
        _ => return Err("invalid mode".into()),
    }
    Ok(())
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await { Ok(()) => std::process::ExitCode::SUCCESS, Err(_) => { eprintln!("recovery_object_failed_private_details_withheld"); std::process::ExitCode::FAILURE } }
}
