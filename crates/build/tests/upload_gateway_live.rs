//! Real kernel tmpfs + native HTTP qualification. Run tools/tests/build-upload-gateway.sh.
#![cfg(all(feature = "service", feature = "worker", feature = "kubernetes-job"))]

use futures_util::{StreamExt, stream};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    net::TcpStream,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
use webdock_build::{
    artifact::{Archive, validate_oci},
    container_runner::{ContainerBuildRequest, Strategy},
    execution::Limits,
    kubernetes_transfer::{AttemptTransfer, ResultIdentity},
    kubernetes_upload::{AttemptUploadGateway, router},
};

const CAPACITY: u64 = 2 * 1024 * 1024;
const MAX_BYTES: u64 = 64 * 1024;
const ORIGIN: &str = "https://owned-upload.test";

fn fs_budget(file: &File) -> (u64, u64) {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: file is live and stat points to writable storage of the required size.
    let status = unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) };
    assert_eq!(status, 0);
    // SAFETY: successful fstatfs initialized all fields used here.
    let stat = unsafe { stat.assume_init() };
    assert_eq!(stat.f_type, libc::TMPFS_MAGIC);
    (
        stat.f_blocks * stat.f_bsize as u64,
        stat.f_bavail * stat.f_bsize as u64,
    )
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
        source_url: format!(
            "{ORIGIN}/build-attempts/1001/2001/3001/{generation}/source.tar?source=private"
        ),
        result_put_url: String::new(),
        build_secrets: BTreeMap::new(),
    }
}
fn result(attempt: &AttemptTransfer, uid: &str, log: &str) -> Vec<u8> {
    let config = b"{}";
    let config_digest = hex::encode(Sha256::digest(config));
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":config.len(),"digest":format!("sha256:{config_digest}")},"layers":[]})).unwrap();
    let manifest_digest = hex::encode(Sha256::digest(&manifest));
    let image=Archive::from_files(BTreeMap::from([
        ("oci-layout".into(),br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec()),
        ("index.json".into(),serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{manifest_digest}")}]})).unwrap()),
        (format!("blobs/sha256/{manifest_digest}"),manifest),
        (format!("blobs/sha256/{config_digest}"),config.to_vec()),
    ])).unwrap().encode().unwrap();
    let status=serde_json::to_vec(&json!({"success":true,"sourceSha":attempt.request.source_sha,"archiveSha256":attempt.request.archive_sha256,"artifactDigest":format!("sha256:{}",hex::encode(Sha256::digest(&image))),"imageDigest":validate_oci(&image).unwrap(),"artifactBytes":image.len()})).unwrap();
    Archive::from_files(BTreeMap::from([
        (
            "attempt.json".into(),
            serde_json::to_vec(&ResultIdentity::for_attempt(attempt, uid).unwrap()).unwrap(),
        ),
        ("status.json".into(), status),
        ("image.tar".into(), image),
        ("build.log".into(), log.as_bytes().to_vec()),
        ("events.jsonl".into(), vec![]),
    ]))
    .unwrap()
    .encode()
    .unwrap()
}
fn open(root: &Path) -> webdock_build::Result<Arc<AttemptUploadGateway>> {
    AttemptUploadGateway::open(root, CAPACITY, ORIGIN, MAX_BYTES, 1)
}
fn safe_error<T>(result: webdock_build::Result<T>) -> &'static str {
    match result {
        Err(e) => {
            assert!(e.logs.is_empty());
            assert!(!e.to_string().contains("grant="));
            e.code
        }
        Ok(_) => panic!("expected closed gateway"),
    }
}
fn endpoint(base: &str, grant: &str) -> String {
    let url = url::Url::parse(grant).unwrap();
    format!("{base}{}?{}", url.path(), url.query().unwrap())
}
async fn used_bytes(root: &File, baseline: u64, expected: u64) {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let (_, free) = fs_budget(root);
        if baseline - free == expected {
            return;
        }
        assert!(
            Instant::now() < until,
            "tmpfs allocation/cleanup did not reach expected byte count"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
fn mount(root: &Path) {
    assert!(
        Command::new("mount")
            .args([
                "-t",
                "tmpfs",
                "-o",
                "size=2097152,mode=0700,nosuid,nodev,noexec",
                "webdock-upload-owned-test"
            ])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
}
fn unmount(root: &Path) {
    assert!(Command::new("umount").arg(root).status().unwrap().success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires script-owned user/mount namespace and dedicated tmpfs"]
async fn real_tmpfs_and_native_http_upload_gate() {
    let root = PathBuf::from(
        std::env::var_os("WEBDOCK_UPLOAD_LIVE_ROOT")
            .expect("use tools/tests/build-upload-gateway.sh"),
    );
    let evidence = PathBuf::from(
        std::env::var_os("WEBDOCK_UPLOAD_LIVE_EVIDENCE").expect("evidence path required"),
    );
    assert!(root.is_absolute());
    let root_fd = File::open(&root).unwrap();
    let (capacity, baseline) = fs_budget(&root_fd);
    assert_eq!(capacity, CAPACITY);
    // SAFETY: geteuid has no arguments or preconditions.
    let euid = unsafe { libc::geteuid() };
    let meta = root_fd.metadata().unwrap();
    assert_eq!(meta.uid(), euid);
    assert_eq!(meta.mode() & 0o777, 0o700);
    let ordinary = tempfile::tempdir().unwrap();
    std::fs::set_permissions(ordinary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        safe_error(open(ordinary.path())),
        "upload_hard_filesystem_required"
    );
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o770)).unwrap();
    assert_eq!(
        safe_error(open(&root)),
        "invalid_upload_gateway_configuration"
    );
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    // The script maps one subordinate UID so this isolates the owner check on an otherwise valid mount.
    std::os::unix::fs::chown(&root, Some(1), None).unwrap();
    assert_eq!(std::fs::metadata(&root).unwrap().uid(), 1);
    assert_eq!(
        safe_error(open(&root)),
        "invalid_upload_gateway_configuration"
    );
    std::os::unix::fs::chown(&root, Some(euid), None).unwrap();
    let dirty = root.join("owned-startup-probe");
    std::fs::write(&dirty, []).unwrap();
    assert_eq!(safe_error(open(&root)), "invalid_upload_filesystem_budget");
    std::fs::remove_file(&dirty).unwrap();
    let child = root.join("owned-child-probe");
    std::fs::create_dir(&child).unwrap();
    std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(safe_error(open(&child)), "upload_hard_filesystem_required");
    std::fs::remove_dir(&child).unwrap();
    assert_eq!(
        safe_error(AttemptUploadGateway::open(
            &root,
            CAPACITY - 4096,
            ORIGIN,
            MAX_BYTES,
            1
        )),
        "invalid_upload_filesystem_budget"
    );
    // Confirm the kernel rejects actual writes beyond size; no semaphore is used for this probe.
    let probe = root.join("owned-physical-quota-probe");
    let mut file = File::create(&probe).unwrap();
    let err = file
        .write_all(&vec![0; CAPACITY as usize + 4096])
        .unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::ENOSPC));
    assert!(file.metadata().unwrap().len() <= CAPACITY);
    drop(file);
    std::fs::remove_file(&probe).unwrap();
    used_bytes(&root_fd, baseline, 0).await;

    let gateway = open(&root).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base = format!("http://{address}");
    let app = router(gateway.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap();

    // A lying oversized header gets a reply without sending any body bytes at all.
    let a = attempt(1);
    let grant = gateway.issue(&a, 60).unwrap();
    gateway.bind(&a, "owned-job-1").unwrap();
    let uri = endpoint("", grant.as_str());
    let response=tokio::task::spawn_blocking(move||{
        let mut connection=TcpStream::connect(address).unwrap();connection.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write!(connection,"PUT {uri} HTTP/1.1\r\nHost: owned-upload.test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",MAX_BYTES+1).unwrap();
        let mut reply=[0;256];let n=connection.read(&mut reply).unwrap();String::from_utf8_lossy(&reply[..n]).starts_with("HTTP/1.1 413")
    }).await.unwrap();
    assert!(response);
    used_bytes(&root_fd, baseline, 0).await;
    gateway.retire_scope("1001", "2001", "3001", 1).unwrap();

    // Missing Content-Length/chunked bytes are metered before the overflow byte is written.
    let a = attempt(2);
    let grant = gateway.issue(&a, 60).unwrap();
    gateway.bind(&a, "owned-job-2").unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(2);
    let chunks = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<_, std::io::Error>(b), rx))
    });
    let request = client
        .put(endpoint(&base, grant.as_str()))
        .body(reqwest::Body::wrap_stream(chunks));
    let uploading = tokio::spawn(async move { request.send().await.unwrap() });
    tx.send(vec![0; MAX_BYTES as usize]).await.unwrap();
    used_bytes(&root_fd, baseline, MAX_BYTES).await;
    tx.send(vec![1]).await.unwrap();
    drop(tx);
    assert_eq!(
        uploading.await.unwrap().status(),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE
    );
    used_bytes(&root_fd, baseline, 0).await;
    gateway.retire_scope("1001", "2001", "3001", 2).unwrap();

    // An unfinished body is interrupted by revocation, and its anonymous file is closed first.
    let a = attempt(3);
    let grant = gateway.issue(&a, 60).unwrap();
    gateway.bind(&a, "owned-job-3").unwrap();
    let chunks =
        stream::once(async { Ok::<_, std::io::Error>(vec![0; 4096]) }).chain(stream::pending());
    let request = client
        .put(endpoint(&base, grant.as_str()))
        .body(reqwest::Body::wrap_stream(chunks));
    let uploading = tokio::spawn(async move { request.send().await.unwrap() });
    used_bytes(&root_fd, baseline, 4096).await;
    gateway.revoke_scope("1001", "2001", "3001", 3).unwrap();
    assert_eq!(
        uploading.await.unwrap().status(),
        reqwest::StatusCode::FORBIDDEN
    );
    used_bytes(&root_fd, baseline, 0).await;
    gateway.retire_scope("1001", "2001", "3001", 3).unwrap();

    let a = attempt(4);
    let grant = gateway.issue(&a, 60).unwrap();
    gateway.bind(&a, "owned-job-4").unwrap();
    let chunks =
        stream::once(async { Ok::<_, std::io::Error>(vec![0; 4096]) }).chain(stream::pending());
    let before = Instant::now();
    let response = client
        .put(endpoint(&base, grant.as_str()))
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::REQUEST_TIMEOUT);
    assert!(before.elapsed() < Duration::from_secs(3));
    used_bytes(&root_fd, baseline, 0).await;
    gateway.retire_scope("1001", "2001", "3001", 4).unwrap();

    let a = attempt(5);
    let grant = gateway.issue(&a, 60).unwrap();
    gateway.bind(&a, "owned-job-5").unwrap();
    let bytes = result(&a, "owned-job-5", "owned valid receipt");
    let chunks = stream::iter(
        bytes
            .chunks(333)
            .map(|b| Ok::<_, std::io::Error>(b.to_vec()))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        client
            .put(endpoint(&base, grant.as_str()))
            .body(reqwest::Body::wrap_stream(chunks))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(gateway.read(&a, "owned-job-5").unwrap(), bytes);
    let allocated = (bytes.len() as u64).div_ceil(4096) * 4096;
    used_bytes(&root_fd, baseline, allocated).await;
    assert_eq!(
        client
            .put(endpoint(&base, grant.as_str()))
            .body(bytes.clone())
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        client
            .put(endpoint(&base, grant.as_str()))
            .body(result(&a, "owned-job-5", "different bytes"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    let foreign = endpoint(&base, grant.as_str()).replace("/1001/", "/1002/");
    assert_eq!(
        client
            .put(foreign)
            .body(bytes.clone())
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);

    // Replace the path with a different dedicated tmpfs while the old root fd/receipt remain live.
    mount(&root);
    assert_eq!(
        safe_error(gateway.issue(&attempt(6), 60)),
        "upload_filesystem_changed"
    );
    assert_eq!(
        safe_error(gateway.read(&a, "owned-job-5")),
        "upload_filesystem_changed"
    );
    assert_eq!(
        safe_error(gateway.bind(&a, "owned-job-5")),
        "upload_filesystem_changed"
    );
    let unread = stream::poll_fn(
        |_| -> std::task::Poll<Option<webdock_build::Result<Vec<u8>>>> {
            panic!("replacement body polled")
        },
    );
    assert_eq!(
        safe_error(gateway.upload(grant.as_str(), None, unread).await),
        "upload_filesystem_changed"
    );
    // Revocation remains available even when the pathname is replaced.
    gateway.retire_scope("1001", "2001", "3001", 5).unwrap();
    used_bytes(&root_fd, baseline, 0).await;
    unmount(&root);
    assert_eq!(fs_budget(&File::open(&root).unwrap()).0, CAPACITY);
    assert!(gateway.read(&a, "owned-job-5").is_err());
    assert_eq!(
        client
            .put(endpoint(&base, grant.as_str()))
            .body(bytes)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    let expiry_attempt = attempt(6);
    let expiry_grant = gateway.issue(&expiry_attempt, 1).unwrap();
    gateway.bind(&expiry_attempt, "owned-job-6").unwrap();
    assert_eq!(
        client
            .put(endpoint(&base, expiry_grant.as_str()))
            .body(result(&expiry_attempt, "owned-job-6", "expiring"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        safe_error(gateway.read(&expiry_attempt, "owned-job-6")),
        "upload_revoked"
    );
    gateway.reconcile_expired().unwrap();
    used_bytes(&root_fd, baseline, 0).await;
    assert_eq!(
        client
            .put(endpoint(&base, expiry_grant.as_str()))
            .body(vec![0; 512])
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    // Lazy detach removes this exact private mount from the path while the gateway still pins its fd.
    // A normal unmount correctly refuses pinned open directory fds as busy.
    assert!(
        Command::new("umount")
            .args(["--lazy", "--"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        safe_error(gateway.issue(&attempt(7), 60)),
        "upload_filesystem_changed"
    );
    assert_eq!(safe_error(open(&root)), "upload_hard_filesystem_required");
    server.abort();
    let _ = server.await;
    drop(gateway);
    used_bytes(&root_fd, baseline, 0).await;
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    std::fs::write(evidence,serde_json::to_vec_pretty(&json!({
        "profile":"owned-userns-dedicated-tmpfs-native-http","transport":"ephemeral loopback HTTP; production HTTPS policy unchanged","passed":true,"verifiedAt":chrono::Utc::now().to_rfc3339(),"filesystemBytes":capacity,"rawUploadCapBytes":MAX_BYTES,"ownerUIDInNamespace":euid,
        "cases":{"ordinaryDirectoryRejected":true,"tmpfsSubdirectoryRejected":true,"privateModeVerified":true,"foreignOwnerRejected":true,"dirtyStartupRejected":true,"ceilingMismatchRejected":true,
            "actualKernelEnospc":true,"headerRejectedBeforeBody":true,"chunkedOverflowMeteredBeforeWrite":true,"revocationInterruptsBody":true,"deadlineInterruptsBody":true,
            "scopedNativeHttpReceiptWithOciValidation":true,"identicalReplay":true,"changedReplayRejected":true,"foreignScopeRejected":true,"replacementMountFailsClosed":true,
            "retirementDuringReplacement":true,"bindingFailsAfterReplacement":true,"expiredReceiptReconciled":true,"lazyMountDetachNeverFallsBack":true,"lateUploadRejected":true,"anonymousFilesExactCleanup":true},
        "limits":{"productionDeployed":false,"podNetworkQualified":false,"tlsProxyQualified":false,"sourceProviderIamQuotaQualified":false,"rebootQualified":false}
    })).unwrap()).unwrap();
}
