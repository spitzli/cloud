//! Owned native HTTPS relay diagnostic. Run tools/tests/build-upload-tls-large.py.
#![cfg(all(feature = "service", feature = "worker", feature = "kubernetes-job"))]
use axum::{
    body::Body,
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::json;
use sha2::{Digest, Sha256};
use spitzli_build::{
    artifact::{Archive, MAX_OUTPUT},
    container_runner::{ContainerBuildRequest, Strategy},
    execution::Limits,
    kubernetes_transfer::{
        AttemptTransfer, MAX_TRANSFER_RESULT, MAX_TRANSFER_SOURCE, TransferClient, TransferPolicy,
    },
    kubernetes_upload::{AttemptUploadGateway, router},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
const ORIGIN: &str = "https://167.86.75.144:19444";
const FS_BYTES: u64 = 1024 * 1024 * 1024;
struct Tap {
    start: Mutex<Instant>,
    headers_ms: AtomicU64,
    last_data_ms: AtomicU64,
    response_ms: AtomicU64,
    bytes: AtomicU64,
    max_frame: AtomicU64,
    status: AtomicU64,
    body_error: AtomicU64,
}
impl Tap {
    fn new() -> Self {
        Self {
            start: Mutex::new(Instant::now()),
            headers_ms: 0.into(),
            last_data_ms: 0.into(),
            response_ms: 0.into(),
            bytes: 0.into(),
            max_frame: 0.into(),
            status: 0.into(),
            body_error: 0.into(),
        }
    }
    fn elapsed(&self) -> u64 {
        self.start.lock().unwrap().elapsed().as_millis() as u64
    }
    fn reset(&self) {
        *self.start.lock().unwrap() = Instant::now();
        for value in [
            &self.headers_ms,
            &self.last_data_ms,
            &self.response_ms,
            &self.bytes,
            &self.max_frame,
            &self.status,
            &self.body_error,
        ] {
            value.store(0, Ordering::SeqCst);
        }
    }
}
async fn tap(State(stats): State<Arc<Tap>>, request: Request, next: Next) -> Response {
    stats
        .headers_ms
        .store(stats.elapsed().max(1), Ordering::SeqCst);
    let (parts, body) = request.into_parts();
    let observed = stats.clone();
    let stream = body.into_data_stream().map(move |frame| {
        match &frame {
            Ok(bytes) => {
                observed
                    .bytes
                    .fetch_add(bytes.len() as u64, Ordering::SeqCst);
                observed
                    .max_frame
                    .fetch_max(bytes.len() as u64, Ordering::SeqCst);
                observed
                    .last_data_ms
                    .store(observed.elapsed(), Ordering::SeqCst);
            }
            Err(_) => {
                observed.body_error.store(1, Ordering::SeqCst);
            }
        }
        frame
    });
    let response = next
        .run(Request::from_parts(parts, Body::from_stream(stream)))
        .await;
    stats
        .status
        .store(u64::from(response.status().as_u16()), Ordering::SeqCst);
    stats.response_ms.store(stats.elapsed(), Ordering::SeqCst);
    response
}
fn used(root: &File) -> u64 {
    let mut value = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: root is live and value points to sufficient writable storage.
    let code = unsafe { libc::fstatfs(root.as_raw_fd(), value.as_mut_ptr()) };
    assert_eq!(code, 0);
    // SAFETY: fstatfs succeeded and initialized all fields used here.
    let value = unsafe { value.assume_init() };
    assert_eq!(value.f_type, libc::TMPFS_MAGIC);
    (value.f_blocks - value.f_bavail) * value.f_bsize as u64
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
        customer_id: "9101".into(),
        project_id: "9102".into(),
        build_id: "9103".into(),
        generation,
        request: ContainerBuildRequest {
            source_sha: "b".repeat(40),
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
        source_url: format!("{ORIGIN}/build-attempts/9101/9102/9103/{generation}/source.tar"),
        result_put_url: String::new(),
        build_secrets: BTreeMap::new(),
    }
}
struct Pattern {
    left: u64,
    offset: usize,
}
impl Read for Pattern {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = out.len().min(self.left as usize);
        for (i, value) in out[..n].iter_mut().enumerate() {
            *value = ((self.offset + i) % 251 + 1) as u8;
        }
        self.offset += n;
        self.left -= n as u64;
        Ok(n)
    }
}
fn append(builder: &mut tar::Builder<File>, name: &str, size: u64, reader: impl Read) {
    let mut header = tar::Header::new_ustar();
    header.set_size(size);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, name, reader).unwrap();
}
fn sha(path: &Path) -> String {
    let mut file = File::open(path).unwrap();
    let mut buf = [0; 65536];
    let mut hash = Sha256::new();
    loop {
        let n = file.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    hex::encode(hash.finalize())
}
fn output(root: &Path, attempt: &AttemptTransfer, mib: u64) -> (PathBuf, u64) {
    let directory = root.join(format!("output-{mib}"));
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let layer = directory.join("layer.tar");
    let mut builder = tar::Builder::new(File::create(&layer).unwrap());
    let data = mib * 1024 * 1024 - 16384;
    append(
        &mut builder,
        "owned-nonzero-padding",
        data,
        Pattern {
            left: data,
            offset: 0,
        },
    );
    builder.finish().unwrap();
    drop(builder);
    let layer_size = std::fs::metadata(&layer).unwrap().len();
    let layer_sha = sha(&layer);
    let config=serde_json::to_vec(&json!({"architecture":"amd64","os":"linux","rootfs":{"type":"layers","diff_ids":[format!("sha256:{layer_sha}")]},"history":[{"created_by":"owned TLS fixture"}]})).unwrap();
    let config_sha = hex::encode(Sha256::digest(&config));
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":config.len(),"digest":format!("sha256:{config_sha}")},"layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar","size":layer_size,"digest":format!("sha256:{layer_sha}")}]})).unwrap();
    let manifest_sha = hex::encode(Sha256::digest(&manifest));
    let index=serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{manifest_sha}")}]})).unwrap();
    let image = directory.join("image.tar");
    let mut image_tar = tar::Builder::new(File::create(&image).unwrap());
    for (name, bytes) in [
        (
            "oci-layout".into(),
            br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
        ),
        ("index.json".into(), index),
        (format!("blobs/sha256/{config_sha}"), config),
        (format!("blobs/sha256/{manifest_sha}"), manifest),
    ] {
        append(&mut image_tar, &name, bytes.len() as u64, &bytes[..]);
    }
    append(
        &mut image_tar,
        &format!("blobs/sha256/{layer_sha}"),
        layer_size,
        File::open(&layer).unwrap(),
    );
    image_tar.finish().unwrap();
    drop(image_tar);
    std::fs::remove_file(layer).unwrap();
    let image_size = std::fs::metadata(&image).unwrap().len();
    assert!(image_size <= MAX_OUTPUT);
    std::fs::write(directory.join("status.json"),serde_json::to_vec(&json!({"success":true,"sourceSha":attempt.request.source_sha,"archiveSha256":attempt.request.archive_sha256,"artifactDigest":format!("sha256:{}",sha(&image)),"imageDigest":format!("sha256:{manifest_sha}"),"artifactBytes":image_size})).unwrap()).unwrap();
    std::fs::write(
        directory.join("build.log"),
        b"owned nonzero OCI TLS fixture\n",
    )
    .unwrap();
    std::fs::write(directory.join("events.jsonl"), []).unwrap();
    (directory, image_size)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires owned dedicated tmpfs namespace, live fixed TLS relay and reverse SSH tunnel"]
async fn actual_large_tls_transfer() {
    let work = PathBuf::from(std::env::var_os("SPITZLI_TLS_LARGE_WORK").unwrap());
    let root = work.join("gateway");
    let data = work.join("fixture-data");
    let fixture = PathBuf::from(std::env::var_os("SPITZLI_TLS_LARGE_CERTS").unwrap());
    assert!(fixture.is_absolute());
    let timeout = std::env::var("SPITZLI_TLS_LARGE_TIMEOUT")
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let sizes = std::env::var("SPITZLI_TLS_LARGE_SIZES").unwrap();
    let evidence = PathBuf::from(std::env::var_os("SPITZLI_TLS_LARGE_EVIDENCE").unwrap());
    let fd = File::open(&root).unwrap();
    let initial = used(&fd);
    let gateway =
        AttemptUploadGateway::open(&root, FS_BYTES, ORIGIN, MAX_TRANSFER_RESULT, timeout).unwrap();
    let stats = Arc::new(Tap::new());
    let app = router(gateway.clone()).layer(middleware::from_fn_with_state(stats.clone(), tap));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:19445")
        .await
        .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let ca = std::fs::read_to_string(fixture.join("ca.crt")).unwrap();
    let policy = TransferPolicy {
        allowed_origins: vec![ORIGIN.into()],
        certificate_authority_pem: Some(ca),
        max_source_bytes: MAX_TRANSFER_SOURCE,
        max_result_bytes: MAX_TRANSFER_RESULT,
        timeout_seconds: timeout,
    };
    let mut results = Vec::new();
    for (index, size) in sizes.split(',').enumerate() {
        let mib = size.parse::<u64>().unwrap();
        assert!([64, 200, 256].contains(&mib));
        let mut attempt = attempt(index as u64 + 1);
        let uid = format!("tls-large-job-{index}");
        let (output, image_size) = output(&data, &attempt, mib);
        attempt.result_put_url = gateway.issue(&attempt, 600).unwrap().as_str().into();
        gateway.bind(&attempt, &uid).unwrap();
        let client = TransferClient::new(policy.clone()).unwrap();
        stats.reset();
        let started = Instant::now();
        let result = client.upload(&attempt, &uid, &output).await;
        let elapsed = started.elapsed().as_millis() as u64;
        let code = result.as_ref().err().map(|error| error.code);
        assert!(
            result
                .as_ref()
                .err()
                .is_none_or(|error| error.logs.is_empty())
        );
        drop(client);
        if result.is_err() {
            gateway
                .revoke_scope("9101", "9102", "9103", attempt.generation)
                .unwrap();
        } else {
            let bytes = gateway.read(&attempt, &uid).unwrap();
            let verified =
                spitzli_build::kubernetes_transfer::verify_result(&attempt, &uid, &bytes).unwrap();
            assert_eq!(verified.artifact.len() as u64, image_size);
            drop(verified);
            drop(bytes);
        }
        gateway
            .retire_scope("9101", "9102", "9103", attempt.generation)
            .unwrap();
        let cleanup_deadline = Instant::now() + Duration::from_secs(60);
        while used(&fd) != initial
            || stats.status.load(Ordering::SeqCst) == 0
                && stats.headers_ms.load(Ordering::SeqCst) != 0
        {
            assert!(Instant::now() < cleanup_deadline);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        results.push(json!({"requestedImageMiB":mib,"imageBytes":image_size,"policyTimeoutSeconds":timeout,"success":result.is_ok(),"safeErrorCode":code,"clientTotalMsIncludingPackaging":elapsed,"gatewayHeadersAtMs":stats.headers_ms.load(Ordering::SeqCst),"gatewayLastBodyAtMs":stats.last_data_ms.load(Ordering::SeqCst),"gatewayResponseAtMs":stats.response_ms.load(Ordering::SeqCst),"gatewayBytesRead":stats.bytes.load(Ordering::SeqCst),"gatewayMaxFrameBytes":stats.max_frame.load(Ordering::SeqCst),"gatewayStatus":stats.status.load(Ordering::SeqCst),"gatewayBodyReadError":stats.body_error.load(Ordering::SeqCst)!=0,"exactTempCleanup":used(&fd)==initial}));
        std::fs::remove_dir_all(output).unwrap();
        std::fs::write(&evidence,serde_json::to_vec_pretty(&json!({"profile":"actual-transfer-client-large-tls-relay-reverse-ssh","results":results,"remoteRelayModified":false,"productionParserModified":false})).unwrap()).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    server.abort();
    let _ = server.await;
    drop(gateway);
    assert_eq!(used(&fd), initial);
    assert!(std::fs::read_dir(root).unwrap().next().is_none());
    assert!(std::fs::read_dir(data).unwrap().next().is_none());
}
