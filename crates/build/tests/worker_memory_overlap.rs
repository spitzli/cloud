//! Actual maximum gateway/collector + maintained publisher overlap in an owned memory cgroup.
#![cfg(all(feature = "local-registry-test", feature = "kubernetes-job"))]
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_build::{
    artifact::{Archive, ArtifactStore, MAX_OUTPUT},
    container_runner::{ContainerBuildRequest, Strategy},
    execution::Limits,
    kubernetes_transfer::{AttemptTransfer, MAX_TRANSFER_RESULT, ResultIdentity, verify_result},
    kubernetes_upload::{AttemptUploadGateway, router},
    publisher::{PublicationGuard, RegistryPublisher},
};
use std::{
    collections::BTreeMap,
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
const FILESYSTEM_BYTES: u64 = 640 * 1024 * 1024;
fn write(root: &Path, name: &str, value: &Value) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = root.join(format!("{name}.tmp"));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)
        .unwrap();
    file.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
    drop(file);
    std::fs::rename(tmp, root.join(name)).unwrap();
}
fn used(file: &File) -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: file is live and info points to sufficient writable storage.
    let status = unsafe { libc::fstatfs(file.as_raw_fd(), info.as_mut_ptr()) };
    assert_eq!(status, 0);
    // SAFETY: successful fstatfs initialized all fields read below.
    let info = unsafe { info.assume_init() };
    assert_eq!(info.f_type, libc::TMPFS_MAGIC);
    (info.f_blocks - info.f_bavail) * info.f_bsize as u64
}
fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
        })
        .unwrap()
        * 1024
}
fn attempt() -> AttemptTransfer {
    let source = Archive::from_files(BTreeMap::from([(
        "Dockerfile".into(),
        b"FROM scratch\n".to_vec(),
    )]))
    .unwrap()
    .encode()
    .unwrap();
    AttemptTransfer {
        customer_id: "1001".into(),
        project_id: "2001".into(),
        build_id: "3001".into(),
        generation: 1,
        request: ContainerBuildRequest {
            source_sha: "a".repeat(40),
            archive_sha256: format!("sha256:{}", hex::encode(Sha256::digest(&source))),
            root_directory: ".".into(),
            context_directory: ".".into(),
            dockerfile: None,
            strategy: Strategy::Auto,
            build_args: BTreeMap::new(),
            limits: Limits {
                cpu: 1,
                memory_mi_b: 512,
                disk_bytes: 10_000_000,
                duration_seconds: 30,
            },
        },
        source_size_bytes: source.len() as u64,
        source_url: "https://maximum-overlap.test/source.tar".into(),
        result_put_url: String::new(),
        build_secrets: BTreeMap::new(),
    }
}
struct Guard {
    work: PathBuf,
    checks: AtomicUsize,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl PublicationGuard for Guard {
    async fn check(&self) -> spitzli_build::Result<()> {
        let checks = self.checks.fetch_add(1, Ordering::SeqCst) + 1;
        if checks == 1 {
            // The real publisher holds parsed maximum OCI buffers here; no dummy allocation.
            write(
                &self.work,
                "publisher-ready.json",
                &json!({"checks":checks,"parsed":true,"peakRssBytes":rss()}),
            );
            self.release.notified().await;
        }
        Ok(())
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires tools/tests/build-worker-memory.py owned namespace/cgroup/registry"]
async fn maximum_worker_overlap_service() {
    let work = PathBuf::from(std::env::var_os("SPITZLI_WORKER_MAX_WORK").unwrap());
    let config: Value =
        serde_json::from_slice(&std::fs::read(work.join("config.json")).unwrap()).unwrap();
    let address: std::net::SocketAddr =
        config["registryAddress"].as_str().unwrap().parse().unwrap();
    assert!(address.ip().is_loopback());
    let store = Arc::new(ArtifactStore::open(work.join("store"), MAX_OUTPUT * 2).unwrap());
    let seed = std::fs::read(work.join("publication-input.tar")).unwrap();
    assert_eq!(seed.len() as u64, MAX_OUTPUT);
    let artifact = store.put("1002", "3002", 1, &seed).unwrap();
    assert_eq!(artifact.size(), MAX_OUTPUT);
    drop(seed);
    std::fs::remove_file(work.join("publication-input.tar")).unwrap();
    let expected = artifact.image_digest().to_string();
    let publisher = Arc::new(RegistryPublisher::loopback_test(address).unwrap());
    let guard = Arc::new(Guard {
        work: work.clone(),
        checks: AtomicUsize::new(0),
        release: tokio::sync::Notify::new(),
    });
    let fd = File::open(work.join("gateway-tmpfs")).unwrap();
    let baseline = used(&fd);
    let gateway = AttemptUploadGateway::open(
        &work.join("gateway-tmpfs"),
        FILESYSTEM_BYTES,
        "https://maximum-overlap.test",
        MAX_TRANSFER_RESULT,
        90,
    )
    .unwrap();
    let attempt = attempt();
    let grant = gateway.issue(&attempt, 300).unwrap();
    gateway.bind(&attempt, "maximum-overlap-job").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_address = listener.local_addr().unwrap();
    let app = router(gateway.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    write(
        &work,
        "control.json",
        &json!({"address":gateway_address.to_string(),"grant":grant.as_str(),"identity":ResultIdentity::for_attempt(&attempt,"maximum-overlap-job").unwrap(),"publisherManifestDigest":expected,"publisherArtifactBytes":artifact.size(),"maxResultBytes":MAX_TRANSFER_RESULT,"maxOutputBytes":MAX_OUTPUT}),
    );
    let mut publishing = None;
    let mut collected = false;
    let mut published = false;
    let deadline = Instant::now() + Duration::from_secs(240);
    while Instant::now() < deadline {
        let command = work.join("command.json");
        if command.try_exists().unwrap() {
            let value: Value = serde_json::from_slice(&std::fs::read(&command).unwrap()).unwrap();
            std::fs::remove_file(command).unwrap();
            match value.as_str().unwrap() {
                "publish" => {
                    assert!(publishing.is_none());
                    let publisher = publisher.clone();
                    let store = store.clone();
                    let artifact = artifact.clone();
                    let guard = guard.clone();
                    publishing = Some(tokio::spawn(async move {
                        publisher
                            .publish("1002", "2002", "4002", &store, &artifact, guard.as_ref())
                            .await
                            .unwrap()
                    }));
                }
                "allow-push" => guard.release.notify_one(),
                "collect" => {
                    write(&work, "collector-started.json", &json!({"started":true}));
                    let bytes = gateway.read(&attempt, "maximum-overlap-job").unwrap();
                    assert_eq!(bytes.len() as u64, MAX_TRANSFER_RESULT);
                    let result = verify_result(&attempt, "maximum-overlap-job", &bytes).unwrap();
                    assert_eq!(result.artifact.len() as u64, MAX_OUTPUT);
                    drop(result);
                    drop(bytes);
                    gateway.retire_scope("1001", "2001", "3001", 1).unwrap();
                    assert_eq!(used(&fd), baseline);
                    collected = true;
                    write(
                        &work,
                        "collector-finished.json",
                        &json!({"collected":true,"tmpfsUsedBytes":used(&fd)-baseline,"peakRssBytes":rss()}),
                    );
                }
                "quit" => break,
                _ => panic!("invalid private fixture command"),
            }
        }
        if publishing
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            let result = publishing.take().unwrap().await.unwrap();
            assert_eq!(
                result,
                format!("{address}/customers/1002/projects/2002@{expected}")
            );
            published = true;
        }
        write(
            &work,
            "progress.json",
            &json!({"collected":collected,"published":published,"publisherChecks":guard.checks.load(Ordering::SeqCst),"tmpfsUsedBytes":used(&fd)-baseline,"peakRssBytes":rss()}),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(collected && published);
    assert!(guard.checks.load(Ordering::SeqCst) > 5);
    server.abort();
    let _ = server.await;
    drop(gateway);
    assert_eq!(used(&fd), baseline);
    assert!(
        std::fs::read_dir(work.join("gateway-tmpfs"))
            .unwrap()
            .next()
            .is_none()
    );
}
