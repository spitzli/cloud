use clap::Parser;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use webdock_agent::{
    config::read_private,
    storage::{
        StorageGuard, StorageKind, StoragePool,
        build::BuildStorageTrust,
        build_kubernetes::{BuildVolumeReconciler, NodeVolumePhase, client_config},
        build_queue::BuildVolumeQueue,
    },
};
use webdock_agent_protocol::build_storage::SignedBuildStorageReservation;
use webdock_supervisor::{ExecutionError, Result};

#[derive(Parser)]
#[command(about = "Node-only signed fixed build-volume reconciler; no ambient cluster credentials")]
struct Arguments {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    trust: PathBuf,
    #[arg(
        long,
        required_unless_present = "requests_namespace",
        conflicts_with = "requests_namespace"
    )]
    request: Option<PathBuf>,
    #[arg(long, requires = "receipts_namespace")]
    requests_namespace: Option<String>,
    #[arg(long, requires = "requests_namespace")]
    receipts_namespace: Option<String>,
    #[arg(long)]
    kubeconfig: PathBuf,
    #[arg(long)]
    context: String,
    #[arg(long)]
    endpoint: String,
    #[arg(long)]
    ca_sha256: String,
    #[arg(long)]
    namespace: String,
    #[arg(long,default_value_t=0,value_parser=clap::value_parser!(u64).range(0..=120))]
    wait_seconds: u64,
}
async fn run(a: Arguments) -> Result<()> {
    let pool = StoragePool::open_kind(&a.config, StorageKind::Builds)?;
    // Mounts are local to the configured node, never a remote/runtime-node action.
    if std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|_| ExecutionError::Runtime)?
        .trim()
        != pool.configuration().node
    {
        return Err(ExecutionError::Ownership);
    }
    let trust: BuildStorageTrust = serde_json::from_slice(
        &read_private(&a.trust, 8192).map_err(|_| ExecutionError::Ownership)?,
    )
    .map_err(|_| ExecutionError::Invalid("build storage trust"))?;
    let config = client_config(
        &read_private(&a.kubeconfig, 131072).map_err(|_| ExecutionError::Ownership)?,
        &a.context,
        &a.endpoint,
        &a.ca_sha256,
        &a.namespace,
    )
    .await?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = kube::Client::try_from(config).map_err(|_| ExecutionError::Runtime)?;
    let mut reconciler = BuildVolumeReconciler::new(pool, client.clone(), a.namespace.clone())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    if let (Some(requests), Some(receipts)) = (a.requests_namespace, a.receipts_namespace) {
        if a.wait_seconds != 0 {
            return Err(ExecutionError::Invalid("queue wait configuration"));
        }
        let queue = BuildVolumeQueue::new(client, a.namespace, requests, receipts)?;
        let signal = cancelled.clone();
        tokio::spawn(async move {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("signal handler");
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
            signal.store(true, Ordering::SeqCst);
        });
        while !cancelled.load(Ordering::SeqCst) {
            if queue
                .poll(&mut reconciler, &trust, cancelled.clone())
                .await
                .is_err()
            {
                tracing::warn!("Build storage queue unavailable; retaining owned allocations");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        return Ok(());
    }
    let request = a
        .request
        .ok_or(ExecutionError::Invalid("build storage request"))?;
    let signed: SignedBuildStorageReservation = serde_json::from_slice(
        &read_private(&request, 8192).map_err(|_| ExecutionError::Ownership)?,
    )
    .map_err(|_| ExecutionError::Invalid("build storage authority"))?;
    let guard = StorageGuard::new(
        signed
            .reservation
            .expires_at
            .min(chrono::Utc::now() + chrono::Duration::seconds(180)),
        cancelled,
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(a.wait_seconds);
    loop {
        let result = reconciler.reconcile(&signed, &trust, &guard).await?;
        if matches!(
            result.phase,
            NodeVolumePhase::Ready | NodeVolumePhase::Retired
        ) || tokio::time::Instant::now() >= deadline
        {
            println!(
                "{}",
                serde_json::to_string(&result).map_err(|_| ExecutionError::Runtime)?
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    match run(Arguments::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
