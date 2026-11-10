//! Isolated maximum-size production gateway process, driven by build-upload-gateway-maximum.py.
#![cfg(all(feature = "service", feature = "worker", feature = "kubernetes-job"))]
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_build::{
    artifact::{Archive, MAX_OUTPUT},
    container_runner::{ContainerBuildRequest, Strategy},
    execution::Limits,
    kubernetes_transfer::{AttemptTransfer, MAX_TRANSFER_RESULT, ResultIdentity, verify_result},
    kubernetes_upload::{AttemptUploadGateway, router},
};
use std::{
    collections::BTreeMap,
    fs::File,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    time::Duration,
};
const FILESYSTEM_BYTES: u64 = 640 * 1024 * 1024;
fn write(root: &Path, name: &str, value: &Value) {
    use std::io::Write;
    let temporary = root.join(format!("{name}.tmp"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .unwrap();
    file.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
    drop(file);
    std::fs::rename(temporary, root.join(name)).unwrap();
}
fn memory() -> u64 {
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
fn used(root: &File) -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: root is live and info points to sufficient writable storage.
    let status = unsafe { libc::fstatfs(root.as_raw_fd(), info.as_mut_ptr()) };
    assert_eq!(status, 0);
    // SAFETY: successful fstatfs initialized the fields used here.
    let info = unsafe { info.assume_init() };
    assert_eq!(info.f_type, libc::TMPFS_MAGIC);
    (info.f_blocks - info.f_bavail) * info.f_bsize as u64
}
fn attempt(generation: u64) -> AttemptTransfer {
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
        generation,
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
        source_url: "https://maximum-upload.test/source.tar".into(),
        result_put_url: String::new(),
        build_secrets: BTreeMap::new(),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires private user/mount namespace and isolated memory cgroup"]
async fn maximum_gateway_service() {
    let work = std::path::PathBuf::from(std::env::var_os("WEBDOCK_UPLOAD_MAX_WORK").unwrap());
    let root = work.join("gateway-tmpfs");
    let fd = File::open(&root).unwrap();
    let baseline = used(&fd);
    let gateway = AttemptUploadGateway::open(
        &root,
        FILESYSTEM_BYTES,
        "https://maximum-upload.test",
        MAX_TRANSFER_RESULT,
        60,
    )
    .unwrap();
    let first = attempt(1);
    let first_url = gateway.issue(&first, 300).unwrap();
    gateway.bind(&first, "maximum-job-1").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = router(gateway.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    write(
        &work,
        "control.json",
        &json!({"address":address.to_string(),"grant":first_url.as_str(),"identity":ResultIdentity::for_attempt(&first,"maximum-job-1").unwrap(),"maxResultBytes":MAX_TRANSFER_RESULT,"maxOutputBytes":MAX_OUTPUT}),
    );
    let mut collected = false;
    let mut overflow = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(150);
    while std::time::Instant::now() < deadline {
        let command = work.join("command.json");
        if command.try_exists().unwrap() {
            let value: Value = serde_json::from_slice(&std::fs::read(&command).unwrap()).unwrap();
            std::fs::remove_file(command).unwrap();
            match value.as_str().unwrap() {
                "collect" => {
                    let bytes = gateway.read(&first, "maximum-job-1").unwrap();
                    assert_eq!(bytes.len() as u64, MAX_TRANSFER_RESULT);
                    let result = verify_result(&first, "maximum-job-1", &bytes).unwrap();
                    assert_eq!(result.artifact.len() as u64, MAX_OUTPUT);
                    drop(result);
                    drop(bytes);
                    gateway.retire_scope("1001", "2001", "3001", 1).unwrap();
                    assert_eq!(used(&fd), baseline);
                    collected = true;
                    let second = attempt(2);
                    let grant = gateway.issue(&second, 120).unwrap();
                    gateway.bind(&second, "maximum-job-2").unwrap();
                    write(
                        &work,
                        "control.json",
                        &json!({"address":address.to_string(),"grant":grant.as_str(),"generation":2,"maxResultBytes":MAX_TRANSFER_RESULT}),
                    );
                }
                "overflow-done" => {
                    gateway.retire_scope("1001", "2001", "3001", 2).unwrap();
                    assert_eq!(used(&fd), baseline);
                    overflow = true;
                }
                "quit" => break,
                _ => panic!("invalid private fixture command"),
            }
        }
        write(
            &work,
            "progress.json",
            &json!({"collected":collected,"overflowRetired":overflow,"tmpfsUsedBytes":used(&fd)-baseline,"peakRssBytes":memory()}),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.abort();
    let _ = server.await;
    drop(gateway);
    assert_eq!(used(&fd), baseline);
    assert!(std::fs::read_dir(root).unwrap().next().is_none());
}
