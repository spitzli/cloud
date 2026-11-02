//! Explicit owned-database + dedicated build-cluster acceptance. No RUN cluster,
//! ambient kubeconfig, VM image, provider credential, or fabricated OCI result.
use super::*;
use k8s_openapi::api::{
    batch::v1::Job,
    core::v1::{Node, Pod, Secret},
};
use kube::{
    Api, Client,
    api::{ListParams, LogParams, PostParams},
};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, Instant},
};
use webdock_build::{
    BuildService, RepositoryProvider, RepositoryRef, VerifiedCommit,
    artifact::{Archive, ArtifactStore, MAX_OUTPUT, validate_oci},
    execution::GuestRequest,
    kubernetes::{
        KubernetesBuildConfig, KubernetesBuildLimits, KubernetesBuildProfile,
        KubernetesBuildRequest, KubernetesJobBuildExecutor, TrustedProject,
    },
    kubernetes_dispatcher::{KubernetesArtifactTransfer, KubernetesBuildDispatcher},
    kubernetes_staging::KubernetesStagingTransfer,
    kubernetes_transfer::TransferPolicy,
    worker::ControlClient,
};

const NS: &str = "webdock-build-qualification";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LiveConfig {
    build_kubeconfig: PathBuf,
    kube_context: String,
    api_endpoint: String,
    #[serde(rename = "buildCaSHA256")]
    build_ca_sha256: String,
    runner_image: String,
    s3_config: PathBuf,
    credentials: PathBuf,
    ca_file: PathBuf,
    transfer_policy: PathBuf,
    evidence_directory: PathBuf,
    timeout_seconds: u64,
    #[serde(default)]
    upload_gateway_root: Option<PathBuf>,
}

fn private_read(path: &Path, limit: u64) -> Vec<u8> {
    assert!(path.is_absolute());
    assert_eq!(
        path.parent().unwrap().canonicalize().unwrap(),
        path.parent().unwrap()
    );
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .unwrap();
    let metadata = file.metadata().unwrap();
    assert!(metadata.is_file() && metadata.nlink() == 1 && metadata.mode() & 0o077 == 0);
    assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
    assert!(metadata.len() <= limit);
    let mut bytes = vec![];
    file.take(limit + 1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() as u64 <= limit);
    bytes
}

fn private_dir(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let meta = path.symlink_metadata().unwrap();
    assert!(meta.is_dir() && meta.mode() & 0o777 == 0o700);
    assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(path.canonicalize().unwrap(), path);
}

fn evidence(path: &Path, value: &Value) {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .unwrap();
    file.write_all(serde_json::to_string_pretty(value).unwrap().as_bytes())
        .unwrap();
    file.as_file().sync_all().unwrap();
    file.persist(path).unwrap();
    std::fs::File::open(path.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
}

fn owned_database() -> (PathBuf, Value) {
    let path = PathBuf::from(
        std::env::var_os("WEBDOCK_KUBERNETES_LIVE_CUSTODY")
            .expect("owned local PostgreSQL custody"),
    );
    let custody: Value = serde_json::from_slice(&private_read(&path, 16384)).unwrap();
    let container = custody["containerID"].as_str().unwrap();
    assert!(container.len() == 64 && container.bytes().all(|b| b.is_ascii_hexdigit()));
    let output = Command::new("docker")
        .args([
            "--host",
            "unix:///var/run/docker.sock",
            "inspect",
            container,
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let inspect: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inspect.len(), 1);
    assert_eq!(inspect[0]["Id"], container);
    assert_eq!(
        inspect[0]["Config"]["Labels"]["webdock.kubernetes-admission"],
        custody["runID"]
    );
    let database =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(database.host_str(), Some("127.0.0.1"));
    assert_eq!(database.path(), "/webdock_rls_test");
    assert_eq!(
        inspect[0]["NetworkSettings"]["Ports"]["5432/tcp"][0]["HostIp"],
        "127.0.0.1"
    );
    assert_eq!(
        inspect[0]["NetworkSettings"]["Ports"]["5432/tcp"][0]["HostPort"]
            .as_str()
            .unwrap()
            .parse::<u16>()
            .unwrap(),
        database.port().unwrap()
    );
    (path, custody)
}

struct LocalGit {
    tree: tempfile::TempDir,
    customer: String,
    project: String,
    calls: Mutex<Vec<(String, String)>>,
}
impl LocalGit {
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(self.tree.path())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "owned local Git command failed");
        output.stdout
    }
    fn commit(&self) -> String {
        self.git(&["add", "--all"]);
        self.git(&[
            "-c",
            "user.name=Owned fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "owned live source",
        ]);
        String::from_utf8(self.git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .into()
    }
}
#[async_trait::async_trait]
impl RepositoryProvider for LocalGit {
    async fn resolve(
        &self,
        repository: &RepositoryRef<'_>,
        reference: &str,
    ) -> webdock_build::Result<VerifiedCommit> {
        assert_eq!(repository.customer_id, self.customer);
        assert_eq!(repository.project_id, self.project);
        assert_eq!(reference, "main");
        let sha = String::from_utf8(self.git(&["rev-parse", "--verify", "main^{commit}"]))
            .unwrap()
            .trim()
            .to_owned();
        self.calls
            .lock()
            .unwrap()
            .push(("resolve".into(), sha.clone()));
        VerifiedCommit::new(sha)
    }
    async fn archive(
        &self,
        repository: &RepositoryRef<'_>,
        commit: &str,
    ) -> webdock_build::Result<Vec<u8>> {
        assert_eq!(repository.customer_id, self.customer);
        assert_eq!(repository.project_id, self.project);
        VerifiedCommit::new(commit.into())?;
        self.calls
            .lock()
            .unwrap()
            .push(("archive".into(), commit.into()));
        Ok(self.git(&["archive", "--format=tar", commit]))
    }
}

fn repository(f: &Fixture) -> Arc<LocalGit> {
    let repository = Arc::new(LocalGit {
        tree: tempfile::tempdir().unwrap(),
        customer: f.cid.clone(),
        project: f.pid.clone(),
        calls: Mutex::new(vec![]),
    });
    repository.git(&["init", "-b", "main"]);
    // The only executable fixture is compiled locally but never run on the controller.
    let program = format!(
        r#"fn main() {{
        match std::env::args().nth(1).as_deref() {{
            Some("wait") => {{println!("owned-cancel-wait{{}}",".".repeat(256));std::thread::sleep(std::time::Duration::from_secs(120));}},
            Some("serve") => {{use std::io::{{Read,Write}};for incoming in std::net::TcpListener::bind("0.0.0.0:8080").unwrap().incoming(){{let mut socket=incoming.unwrap();let mut bytes=[0u8;1024];let _=socket.read(&mut bytes);let _=socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nowned");}}}},
            _ => {{std::fs::write("/proof.txt",b"{}").unwrap();if let Ok(secret)=std::fs::read_to_string("/run/secrets/BUILD_TOKEN"){{println!("owned-secret={{secret}}");}}println!("owned-real-RUN-proof");}}
        }}
    }}"#,
        f.nonce
    );
    let source = repository.tree.path().join("fixture.rs");
    std::fs::write(&source, program).unwrap();
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .unwrap();
    assert!(sysroot.status.success());
    let linker = Path::new(String::from_utf8(sysroot.stdout).unwrap().trim())
        .join("lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld");
    let status = Command::new("rustc")
        .args([
            "--crate-name",
            "owned_build_fixture",
            "--target",
            "x86_64-unknown-linux-musl",
            "-C",
            "link-self-contained=yes",
            "-C",
            "strip=symbols",
            "-C",
        ])
        .arg(format!("linker={}", linker.display()))
        .arg("-O")
        .arg(&source)
        .arg("-o")
        .arg(repository.tree.path().join("fixture"))
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::set_permissions(
        repository.tree.path().join("fixture"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(repository.tree.path().join("Dockerfile"),"FROM scratch\nCOPY fixture /fixture\nARG FIXTURE_REVISION\nLABEL org.opencontainers.image.revision=$FIXTURE_REVISION\nRUN --mount=type=secret,id=BUILD_TOKEN [\"/fixture\",\"build\"]\nENTRYPOINT [\"/fixture\"]\nCMD [\"serve\"]\n").unwrap();
    repository
}

async fn row(owner: &DatabaseConnection, build: &str) -> Value {
    owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE id=$1",
            [build.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}

pub(super) async fn retained_log_pages(
    service: &BuildService,
    principal: &webdock_identity::Principal,
    project: &str,
    build: &str,
) -> Value {
    let mut cursor = "0".to_owned();
    let mut entries = vec![];
    let mut pages = 0;
    loop {
        let page=service.execute(principal,json!({"action":"git.builds.logs","projectID":project,"buildID":build,"after":cursor,"limit":100})).await.unwrap();
        pages += 1;
        let values = page["entries"].as_array().unwrap();
        let next = page["cursor"].as_str().unwrap().to_owned();
        if !values.is_empty() {
            assert!(next.parse::<u64>().unwrap() > cursor.parse::<u64>().unwrap());
        }
        entries.extend(values.iter().cloned());
        assert!(entries.len() <= 10000, "fixture log reader remains bounded");
        cursor = next;
        if values.len() < 100 {
            return json!({"entries":entries,"cursor":cursor,"pages":pages});
        }
    }
}

async fn attempts(owner: &DatabaseConnection, customer: &str) -> Vec<Value> {
    owner.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(a)||jsonb_build_object('sourceSHA',b.source_sha) AS value FROM webdock_auth.git_build_attempt a JOIN webdock_auth.git_build b ON b.id=a.build_id WHERE a.customer_id=$1 ORDER BY a.created_at",[customer.into()])).await.unwrap().into_iter().map(|r|r.try_get("","value").unwrap()).collect()
}

fn request(attempt: &Value) -> KubernetesBuildRequest {
    let mut value = attempt["execution_request"].clone();
    value.as_object_mut().unwrap().remove("containerBuild");
    value["buildEnvironment"] = json!({});
    let guest: GuestRequest = serde_json::from_value(value).unwrap();
    KubernetesBuildRequest {
        customer_id: attempt["customer_id"].as_str().unwrap().into(),
        project_id: attempt["project_id"].as_str().unwrap().into(),
        build_id: attempt["build_id"].as_str().unwrap().into(),
        generation: attempt["generation"].as_u64().unwrap(),
        attempt: attempt["generation"].as_u64().unwrap().try_into().unwrap(),
        commit_sha: attempt["sourceSHA"].as_str().unwrap().into(),
        volume: attempt["volume_handle"]
            .as_object()
            .map(|_| serde_json::from_value(attempt["volume_handle"].clone()).unwrap()),
        limits: KubernetesBuildLimits {
            cpu_millis: guest.limits.cpu * 1000,
            memory_mib: guest.limits.memory_mi_b,
            workspace_mib: guest
                .limits
                .disk_bytes
                .div_ceil(1024 * 1024)
                .try_into()
                .unwrap(),
            cache_mib: 16384,
            deadline_seconds: guest.limits.duration_seconds.try_into().unwrap(),
        },
    }
}

async fn diagnostics(
    client: &Client,
    owner: &DatabaseConnection,
    customer: &str,
    directory: &Path,
) {
    for attempt in attempts(owner, customer).await {
        let name = attempt["job_name"].as_str().unwrap();
        if let Ok(Some(job)) = Api::<Job>::namespaced(client.clone(), NS)
            .get_opt(name)
            .await
        {
            evidence(
                &directory.join(format!("diagnostic-{name}.json")),
                &json!({"name":name,"uid":job.metadata.uid,"status":job.status}),
            );
        }
        let label = format!("batch.kubernetes.io/job-name={name}");
        if let Ok(pods) = Api::<Pod>::namespaced(client.clone(), NS)
            .list(&ListParams::default().labels(&label))
            .await
        {
            for pod in pods.items {
                if pod.metadata.owner_references.as_ref().is_none_or(|refs| {
                    refs.iter()
                        .all(|r| r.uid != attempt["job_uid"].as_str().unwrap_or_default())
                }) {
                    continue;
                }
                evidence(
                    &directory.join(format!("diagnostic-{name}-pod-status.json")),
                    &json!({"name":pod.metadata.name,"status":pod.status}),
                );
                for container in ["download", "runner"] {
                    if let Ok(logs) = Api::<Pod>::namespaced(client.clone(), NS)
                        .logs(
                            pod.metadata.name.as_deref().unwrap(),
                            &LogParams {
                                container: Some(container.into()),
                                limit_bytes: Some(65536),
                                tail_lines: Some(1000),
                                ..LogParams::default()
                            },
                        )
                        .await
                    {
                        let destination =
                            directory.join(format!("diagnostic-{name}-{container}-logs.json"));
                        retain_diagnostic_log(&destination, &logs);
                    }
                }
            }
        }
    }
}

fn retain_diagnostic_log(destination: &Path, logs: &str) {
    if logs.starts_with("unable to retrieve container logs") && destination.exists() {
        return;
    }
    evidence(destination, &json!({"logs":logs}));
}

async fn clean_remote(
    client: &Client,
    owner: &DatabaseConnection,
    customer: &str,
    executor: &KubernetesJobBuildExecutor,
    transfer: &KubernetesStagingTransfer,
) -> bool {
    for attempt in attempts(owner, customer).await {
        let request = request(&attempt);
        let observed = if let Some(fp) = attempt["spec_fingerprint"].as_str() {
            match executor.observe(&request, fp).await {
                Ok(value) => value,
                Err(_) => return false,
            }
        } else {
            None
        };
        if let Some(handle) = observed {
            if attempt["job_uid"]
                .as_str()
                .is_some_and(|uid| uid != handle.uid)
            {
                return false;
            }
            if executor.cancel(&handle).await.is_err() {
                return false;
            }
        }
        let until = Instant::now() + Duration::from_secs(120);
        loop {
            match executor.absent(&request, attempt["job_uid"].as_str()).await {
                Ok(true) => break,
                Ok(false) if Instant::now() < until => {
                    tokio::time::sleep(Duration::from_secs(1)).await
                }
                _ => return false,
            }
        }
        if (request.volume.is_some()
            || attempt["storage_request"].is_null()
            || attempt["spec_fingerprint"].is_string())
            && transfer.cleanup(&request).await.is_err()
        {
            return false;
        }
        if !secret_absent(client, executor, &request).await {
            return false;
        }
    }
    true
}

async fn secret_absent(
    client: &Client,
    executor: &KubernetesJobBuildExecutor,
    request: &KubernetesBuildRequest,
) -> bool {
    let Ok(name) = executor.handoff_name(request) else {
        return false;
    };
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        match Api::<Secret>::namespaced(client.clone(), NS)
            .get(&name)
            .await
        {
            Err(kube::Error::Api(response)) if response.code == 404 => return true,
            Ok(_) if Instant::now() < until => tokio::time::sleep(Duration::from_millis(250)).await,
            _ => return false,
        }
    }
}

fn assert_actual_run(bytes: &[u8], nonce: &str, sha: &str) -> String {
    let envelope = Archive::read(bytes, MAX_OUTPUT).unwrap();
    let image = envelope.files().get("image.tar").unwrap();
    let digest = validate_oci(image).unwrap();
    let image = Archive::read(image, MAX_OUTPUT).unwrap();
    let index: Value = serde_json::from_slice(image.files().get("index.json").unwrap()).unwrap();
    let blob = |digest: &str| {
        image
            .files()
            .get(&format!(
                "blobs/sha256/{}",
                digest.strip_prefix("sha256:").unwrap()
            ))
            .unwrap()
    };
    let manifest: Value =
        serde_json::from_slice(blob(index["manifests"][0]["digest"].as_str().unwrap())).unwrap();
    let config: Value =
        serde_json::from_slice(blob(manifest["config"]["digest"].as_str().unwrap())).unwrap();
    assert_eq!(
        config["config"]["Labels"]["org.opencontainers.image.revision"],
        sha
    );
    assert_eq!(config["config"]["Entrypoint"], json!(["/fixture"]));
    assert_eq!(config["config"]["Cmd"], json!(["serve"]));
    let mut proof = false;
    for layer in manifest["layers"].as_array().unwrap() {
        let bytes = blob(layer["digest"].as_str().unwrap());
        let mut decoded = vec![];
        if bytes.starts_with(&[0x1f, 0x8b]) {
            flate2::read::GzDecoder::new(bytes.as_slice())
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut decoded)
                .unwrap();
        } else {
            decoded.extend_from_slice(bytes);
        }
        assert!(decoded.len() <= 16 * 1024 * 1024);
        let files = Archive::read(&decoded, 16 * 1024 * 1024).unwrap();
        if let Some(bytes) = files.files().get("proof.txt") {
            assert_eq!(bytes, nonce.as_bytes());
            proof = true;
        }
    }
    assert!(
        proof,
        "real RUN-generated proof.txt absent from exported OCI layers"
    );
    digest
}

fn assert_railpack_run(bytes: &[u8], nonce: &str, sha: &str) -> String {
    let envelope = Archive::read(bytes, MAX_OUTPUT).unwrap();
    let image = envelope.files().get("image.tar").unwrap();
    let digest = validate_oci(image).unwrap();
    let archive = Archive::read(image, MAX_OUTPUT).unwrap();
    let index: Value = serde_json::from_slice(archive.files().get("index.json").unwrap()).unwrap();
    let blob = |digest: &str| {
        archive
            .files()
            .get(&format!(
                "blobs/sha256/{}",
                digest.strip_prefix("sha256:").unwrap()
            ))
            .unwrap()
    };
    let manifest: Value =
        serde_json::from_slice(blob(index["manifests"][0]["digest"].as_str().unwrap())).unwrap();
    let config: Value =
        serde_json::from_slice(blob(manifest["config"]["digest"].as_str().unwrap())).unwrap();
    let serialized = serde_json::to_string(&config).unwrap();
    assert!(!serialized.contains("owned-live-secret-no-provider-authority"));
    assert!(!serialized.contains("BUILD_TOKEN="));
    assert!(!serialized.contains("FIXTURE_REVISION="));
    assert!(!serialized.contains("http://10.83.0.1:3128"));
    let mut proof = false;
    for layer in manifest["layers"].as_array().unwrap() {
        let bytes = blob(layer["digest"].as_str().unwrap());
        let mut decoded = vec![];
        let limit = 256 * 1024 * 1024;
        if bytes.starts_with(&[0x1f, 0x8b]) {
            flate2::read::GzDecoder::new(bytes.as_slice())
                .take(limit + 1)
                .read_to_end(&mut decoded)
                .unwrap();
        } else {
            decoded.extend_from_slice(bytes);
        }
        assert!(decoded.len() as u64 <= limit);
        assert!(
            !decoded
                .windows(b"owned-live-secret-no-provider-authority".len())
                .any(|part| part == b"owned-live-secret-no-provider-authority"),
            "build secret must not leak into any OCI layer"
        );
        // Runtime base layers contain legitimate links. Inspect entries without extracting
        // them; source Archive restrictions remain unchanged and no link is followed.
        let mut tar = tar::Archive::new(decoded.as_slice());
        for entry in tar.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path().unwrap();
            if path == Path::new("app/proof.txt") && entry.header().entry_type().is_file() {
                assert!(entry.size() <= 4096);
                let mut content = String::new();
                entry.take(4097).read_to_string(&mut content).unwrap();
                let value: Value = serde_json::from_str(&content).unwrap();
                assert_eq!(value["nonce"], nonce);
                assert_eq!(value["source"], sha);
                assert_eq!(
                    value["buildSecretPresent"], true,
                    "actual Railpack build script must receive the scoped build secret"
                );
                proof = true;
            }
        }
    }
    assert!(
        proof,
        "real Railpack build-generated app/proof.txt absent from OCI layers"
    );
    digest
}

#[test]
#[ignore = "explicit private evidence from an already completed owned Railpack build; no cluster or provider calls"]
fn kubernetes_live_verify_retained_railpack_evidence() {
    let directory =
        PathBuf::from(std::env::var_os("WEBDOCK_KUBERNETES_RETAINED_EVIDENCE").unwrap());
    private_dir(&directory);
    let receipts = std::fs::read_dir(directory.join("dispatcher"))
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            (p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("attempt-")
                && p.extension().is_some_and(|e| e == "json"))
            .then_some(p)
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 1);
    let receipt: Value = serde_json::from_slice(&private_read(&receipts[0], 512 * 1024)).unwrap();
    assert_eq!(receipt["outcome"], "artifact_retained");
    assert_eq!(receipt["acknowledged"], true);
    assert_eq!(receipt["retired"], true);
    assert_eq!(receipt["abandoned"], false);
    assert!(receipt["pending_logs"].as_array().unwrap().is_empty());
    let acknowledged_entries = receipt["sent_log_keys"].as_array().unwrap().len();
    assert!(acknowledged_entries > 100 && acknowledged_entries <= 10000);
    let artifact = &receipt["completion"]["artifact"];
    let store = ArtifactStore::open(directory.join("artifacts"), MAX_OUTPUT * 4).unwrap();
    let stored = store
        .load(
            artifact["storageKey"].as_str().unwrap().into(),
            artifact["digest"].as_str().unwrap().into(),
            artifact["sizeBytes"].as_u64().unwrap(),
            artifact["imageDigest"].as_str().unwrap().into(),
        )
        .unwrap();
    let bytes = store.read(&stored).unwrap();
    let source = receipt["request"]["commit_sha"].as_str().unwrap();
    let nonce = directory.file_name().unwrap().to_str().unwrap();
    let digest = assert_railpack_run(&bytes, nonce, source);
    assert_eq!(digest, artifact["imageDigest"]);
    let image = private_read(&directory.join("verified-image.oci.tar"), MAX_OUTPUT);
    assert_eq!(
        &image,
        Archive::read(&bytes, MAX_OUTPUT)
            .unwrap()
            .files()
            .get("image.tar")
            .unwrap()
    );
    let manifest: Value = serde_json::from_slice(&private_read(
        &directory.join("exact-job-manifest.json"),
        128 * 1024,
    ))
    .unwrap();
    assert!(
        manifest["spec"]["template"]["spec"]["containers"][0]["env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "WEBDOCK_COMMIT_SHA" && e["value"] == source)
    );
    for logs in [
        receipt["logs"].as_str().unwrap(),
        receipt["completion"]["logs"].as_str().unwrap(),
    ] {
        assert!(logs.contains("owned-real-RUN-proof"));
        assert!(logs.contains("owned-secret=[REDACTED]"));
        assert!(!logs.contains("owned-live-secret-no-provider-authority"));
    }
    assert!(
        std::fs::read_dir(directory.join("staging"))
            .unwrap()
            .next()
            .is_none()
    );
    evidence(
        &directory.join("retained-verification.json"),
        &json!({"sourceSHA":source,"imageDigest":digest,"buildID":receipt["request"]["build_id"],"jobUID":receipt["uid"],"acknowledgedDurableLogEntries":acknowledged_entries,"retired":true,"actualNodeBuildLayerProof":true,"temporarySecretsAbsentFromImageConfig":true,"redactedRUNLogs":true,"stagingStateAbsent":true}),
    );
}

async fn drive(
    dispatcher: &mut KubernetesBuildDispatcher,
    service: &BuildService,
    worker: &webdock_build::WorkerIdentity,
    cluster: &str,
    owner: &DatabaseConnection,
    build: &str,
    timeout: u64,
) {
    let until = Instant::now() + Duration::from_secs(timeout);
    let mut last_error = None;
    loop {
        if let Err(error) = dispatcher.turn().await {
            last_error = Some(error.code);
            assert!(
                matches!(
                    error.code,
                    "kubernetes_unavailable"
                        | "kubernetes_api_unavailable"
                        | "pending_completion"
                        | "build_storage_pending"
                ),
                "live dispatch preparation failed; sanitized code={}",
                error.code
            );
        }
        let build_row = row(owner, build).await;
        assert_ne!(
            build_row["status"], "failed",
            "live build failed; diagnostics remain private"
        );
        if build_row["status"] == "artifact-ready"
            && service
                .list_kubernetes_attempts(worker, cluster)
                .await
                .unwrap()
                .is_empty()
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "live build deadline reached; last sanitized code={last_error:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[tokio::test]
#[ignore = "explicit private live configuration; validates policy and TLS pins without DB or cluster requests"]
async fn kubernetes_live_configuration_preflight() {
    let inputs = live_inputs();
    let _ =
        webdock_object_storage::Store::s3_with_ca(inputs.storage, inputs.credentials, &inputs.ca)
            .unwrap();
}

struct LiveInputs {
    config: LiveConfig,
    kube_config: kube::Config,
    storage: webdock_object_storage::S3Config,
    credentials: webdock_object_storage::Credentials,
    ca: Vec<u8>,
    policy: TransferPolicy,
}

fn live_inputs() -> LiveInputs {
    let config_path = PathBuf::from(std::env::var_os("WEBDOCK_KUBERNETES_LIVE_CONFIG").unwrap());
    let config: LiveConfig =
        serde_json::from_slice(&private_read(&config_path, 128 * 1024)).unwrap();
    assert!((30..=1800).contains(&config.timeout_seconds));
    assert_eq!(config.api_endpoint, "https://127.0.0.1:16443");
    assert_eq!(config.kube_context, "owned-build-live");
    let kube_config = webdock_build::kubernetes_worker::pinned_kubernetes_config(
        &private_read(&config.build_kubeconfig, 128 * 1024),
        &config.kube_context,
        &config.api_endpoint,
        &config.build_ca_sha256,
    )
    .unwrap();
    let storage: webdock_object_storage::S3Config =
        serde_json::from_slice(&private_read(&config.s3_config, 16384)).unwrap();
    assert_eq!(storage.endpoint, "https://167.86.75.144:19443");
    assert_eq!(storage.namespace, "worker-staging");
    assert!(storage.bucket.starts_with("wd-stage-"));
    let credentials = serde_json::from_slice(&private_read(&config.credentials, 16384)).unwrap();
    let ca = private_read(&config.ca_file, 65536);
    let policy: TransferPolicy =
        serde_json::from_slice(&private_read(&config.transfer_policy, 128 * 1024)).unwrap();
    assert_eq!(
        policy.allowed_origins,
        if config.upload_gateway_root.is_some() {
            vec![
                storage.endpoint.clone(),
                "https://167.86.75.144:19444".into(),
            ]
        } else {
            vec![storage.endpoint.clone()]
        }
    );
    assert_eq!(
        policy.certificate_authority_pem.as_deref(),
        Some(std::str::from_utf8(&ca).unwrap())
    );
    webdock_build::kubernetes_transfer::TransferClient::new(policy.clone()).unwrap();
    LiveInputs {
        config,
        kube_config,
        storage,
        credentials,
        ca,
        policy,
    }
}

#[tokio::test]
#[ignore = "explicit owned local PostgreSQL, scoped NEW build-cluster client, pinned real runner and owned TLS staging"]
async fn kubernetes_live_real_run_retains_verified_artifact_logs_and_cancels_owned_job() {
    run_live_acceptance(false).await;
}

#[tokio::test]
#[ignore = "explicit owned local PostgreSQL, scoped NEW build cluster, qualified package gateway and pinned real Railpack frontend"]
async fn kubernetes_live_railpack_retains_verified_image_and_temporary_proxy_secrets() {
    run_live_acceptance(true).await;
}

#[tokio::test]
#[ignore = "explicit retained disposable database custody and private exact native fixture directory; cleanup only"]
async fn kubernetes_live_reconcile_retained_native_fixture() {
    let (_, custody) = owned_database();
    let inputs = live_inputs();
    let directory = PathBuf::from(std::env::var_os("WEBDOCK_NATIVE_RECOVERY_DIRECTORY").unwrap());
    assert_eq!(
        directory.parent().unwrap(),
        inputs.config.evidence_directory
    );
    private_dir(&directory);
    let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let records = owner
        .query_all_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT to_jsonb(c) AS value FROM webdock_auth.git_build_cluster c",
        ))
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let cluster: Value = records[0].try_get("", "value").unwrap();
    let cluster_id = cluster["id"].as_str().unwrap();
    let hosting = Arc::new(
        HostingService::new(
            db("DATABASE_URL").await,
            db("AUTH_DATABASE_URL").await,
            db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            std::env::var("BETTER_AUTH_SECRET").unwrap(),
            Arc::new(webdock_persistence::IdAllocator::new(
                db("DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )),
        )
        .await
        .unwrap(),
    );
    let service = Arc::new(
        BuildService::new(
            db("DATABASE_URL").await,
            db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            Arc::new(webdock_persistence::IdAllocator::new(
                db("DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )),
            hosting,
            "registry.invalid".into(),
        )
        .await
        .unwrap(),
    );
    // Only this custody-verified disposable database is rekeyed; no live credential is read or rotated.
    let mut random = [0u8; 32];
    getrandom::getrandom(&mut random).unwrap();
    use base64::Engine;
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
    use sha2::Digest;
    sql(
        &owner,
        "UPDATE webdock_auth.git_worker SET credential_hash=$2 WHERE id=$1",
        vec![
            cluster["worker_id"].as_str().unwrap().into(),
            hex::encode(sha2::Sha256::digest(token.as_bytes())).into(),
        ],
    )
    .await;
    sql(
        &owner,
        "UPDATE webdock_auth.git_build_admission SET enabled=false WHERE singleton",
        vec![],
    )
    .await;
    sql(&owner,"UPDATE webdock_auth.git_build_attempt SET cancel_requested=true WHERE cluster_id=$1 AND retired_at IS NULL",vec![cluster_id.into()]).await;
    let worker = service.authenticate_worker(&token).await.unwrap();
    let all = service
        .list_kubernetes_attempts(&worker, cluster_id)
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
    let attempt = &all[0];
    let customer = attempt["customer_id"].as_str().unwrap();
    let project = attempt["project_id"].as_str().unwrap();
    let client = Client::try_from(inputs.kube_config).unwrap();
    let executor = KubernetesJobBuildExecutor::new(
        client.clone(),
        KubernetesBuildConfig {
            profile: KubernetesBuildProfile::RootlessIdmapSandbox,
            namespace: NS.into(),
            service_account: "webdock-build-job".into(),
            sandbox_runtime_class: "webdock-build-sandbox".into(),
            runner_image: inputs.config.runner_image,
            storage_cluster_id: Some(cluster_id.into()),
            hold_execution: true,
            trusted_projects: BTreeSet::from([TrustedProject {
                customer_id: customer.into(),
                project_id: project.into(),
            }]),
        },
    )
    .unwrap();
    assert_eq!(
        executor.runtime_fingerprint(),
        cluster["storage_runtime_fingerprint"]
    );
    let gateway = webdock_build::kubernetes_upload::AttemptUploadGateway::open(
        inputs.config.upload_gateway_root.as_ref().unwrap(),
        1073741824,
        "https://167.86.75.144:19444",
        inputs.policy.max_result_bytes,
        inputs.policy.timeout_seconds,
    )
    .unwrap();
    let transfer = Arc::new(
        KubernetesStagingTransfer::new_with_upload_gateway(
            webdock_object_storage::Store::s3_with_ca(
                inputs.storage,
                inputs.credentials,
                &inputs.ca,
            )
            .unwrap(),
            Arc::new(executor.clone()),
            inputs.policy,
            directory.join("staging"),
            gateway,
        )
        .unwrap(),
    );
    let storage_fixture = directory.join("storage-fixture");
    let owned: Value = serde_json::from_slice(&private_read(
        &storage_fixture.join("storage-owner.json"),
        131072,
    ))
    .unwrap();
    let seed: Value = serde_json::from_slice(&private_read(
        &storage_fixture.join("private-signer.json"),
        1024,
    ))
    .unwrap();
    let seed: [u8; 32] = hex::decode(seed["seed"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let storage = webdock_build::build_storage::KubernetesBuildStorage::new(
        client.clone(),
        NS.into(),
        "webdock-build-storage-requests".into(),
        "webdock-build-storage-receipts".into(),
        serde_json::from_value(owned["target"].clone()).unwrap(),
        ed25519_dalek::SigningKey::from_bytes(&seed),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = webdock_build::http::worker_router(service.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let control = ControlClient::loopback_test(&format!("http://{address}"), token).unwrap();
    let mut dispatcher = KubernetesBuildDispatcher::new(
        control,
        executor.clone(),
        transfer.clone(),
        ArtifactStore::open(directory.join("artifacts"), MAX_OUTPUT * 4).unwrap(),
        directory.join("dispatcher"),
        cluster_id.into(),
    )
    .unwrap()
    .with_storage(storage);
    let until = Instant::now() + Duration::from_secs(180);
    while !service
        .list_kubernetes_attempts(&worker, cluster_id)
        .await
        .unwrap()
        .is_empty()
    {
        if let Err(error) = dispatcher.turn().await {
            assert!(
                matches!(
                    error.code,
                    "build_storage_pending"
                        | "kubernetes_unavailable"
                        | "kubernetes_api_unavailable"
                ),
                "cleanup failed safe code={}",
                error.code
            );
        }
        assert!(Instant::now() < until);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(clean_remote(&client, &owner, customer, &executor, &transfer).await);
    let output = Command::new("python3")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/tests/build-native-storage.py"),
        )
        .args(["stop", "--state-root", storage_fixture.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owned node stop failed; retain custody"
    );
    evidence(
        &PathBuf::from(std::env::var_os("WEBDOCK_KUBERNETES_LIVE_CLEANUP_PROOF").unwrap()),
        &json!({"containerID":custody["containerID"],"allJobsAbsent":true,"physicalStorageRetired":true,"evidenceDirectory":directory}),
    );
    server.abort();
}

async fn run_live_acceptance(railpack_build: bool) {
    let (_custody_path, custody) = owned_database();
    let cleanup_path =
        PathBuf::from(std::env::var_os("WEBDOCK_KUBERNETES_LIVE_CLEANUP_PROOF").unwrap());
    let LiveInputs {
        config,
        kube_config,
        storage,
        credentials,
        ca,
        policy,
    } = live_inputs();
    private_dir(&config.evidence_directory);
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    fixture.configure("k3s").await;
    let directory = config.evidence_directory.join(&fixture.nonce);
    private_dir(&directory);
    for name in ["artifacts", "staging", "dispatcher"] {
        private_dir(&directory.join(name));
    }
    let repository = repository(&fixture);
    if railpack_build {
        std::fs::remove_file(repository.tree.path().join("Dockerfile")).unwrap();
        std::fs::write(
            repository.tree.path().join("package.json"),
            serde_json::to_vec(&json!({
                "name":"owned-railpack-fixture", "version":"1.0.0", "private":true,
                "engines":{"node":"22.22.3"},
                "scripts":{"build":"node build.js", "start":"node index.js"}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(repository.tree.path().join("build.js"),format!("require('node:fs').writeFileSync('proof.txt',JSON.stringify({{nonce:{},source:process.env.FIXTURE_REVISION,buildSecretPresent:Boolean(process.env.BUILD_TOKEN)}})); console.log('owned-secret='+process.env.BUILD_TOKEN); console.log('owned-real-RUN-proof');\n",serde_json::to_string(&fixture.nonce).unwrap())).unwrap();
        std::fs::write(repository.tree.path().join("index.js"),"require('node:http').createServer((_,res)=>{res.writeHead(200);res.end('owned');}).listen(8080,'0.0.0.0');\n").unwrap();
    }
    if !railpack_build && config.upload_gateway_root.is_some() {
        let rendered = Command::new("python3")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tools/tests/build-network-isolation.py"),
            )
            .arg("--dockerfile")
            .output()
            .unwrap();
        assert!(rendered.status.success());
        let mut network = String::from_utf8(rendered.stdout).unwrap();
        network = network.replacen(" AS network-qualification", "", 1);
        let first_newline = network.find('\n').unwrap();
        network.insert_str(first_newline, " AS network-qualification");
        network.push_str("RUN printf 'OWNED_NETWORK_RUN_OK\\n' >/tmp/network-proof\n");
        let original = std::fs::read_to_string(repository.tree.path().join("Dockerfile")).unwrap();
        network.push_str(&original.replace(
            "FROM scratch\n",
            "FROM scratch\nCOPY --from=network-qualification /tmp/network-proof /network-proof\n",
        ));
        std::fs::write(repository.tree.path().join("Dockerfile"), network).unwrap();
    }
    let source_sha = repository.commit();
    let service = Arc::new(
        BuildService::new(
            db("DATABASE_URL").await,
            db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            Arc::new(webdock_persistence::IdAllocator::new(
                db("DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )),
            fixture.service.clone(),
            "registry.invalid".into(),
        )
        .await
        .unwrap()
        .with_repository_provider(repository.clone()),
    );
    let enrolled = service
        .enroll_worker(
            &fixture.principal,
            "DE",
            "artifact-only",
            "Owned live dedicated build acceptance; public admission disabled",
            1,
        )
        .await
        .unwrap();
    let credential = enrolled["credential"].as_str().unwrap().to_owned();
    evidence(
        &directory.join("private-worker-credential.json"),
        &json!({"credential":credential}),
    );
    let worker = service.authenticate_worker(&credential).await.unwrap();
    let cluster = service
        .enroll_kubernetes_cluster(
            &fixture.principal,
            worker.id(),
            NS,
            "DE",
            "Owned NEW build node pinned CA and sandbox installation acceptance",
            1,
        )
        .await
        .unwrap();
    let cluster = cluster["id"].as_str().unwrap().to_owned();
    service
        .configure_kubernetes_admission(&fixture.principal, true, 1, 1)
        .await
        .unwrap();
    service
        .trust_kubernetes_project(&fixture.principal, &fixture.cid, &fixture.pid, true)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let error_directory = directory.clone();
    let observation_client = Arc::new(Mutex::new(None::<Client>));
    let response_client = observation_client.clone();
    let diagnostic_fixture = fixture.clone();
    let router =
        webdock_build::http::worker_router(service.clone()).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let directory = error_directory.clone();
                let client = response_client.clone();
                let fixture = diagnostic_fixture.clone();
                async move {
                    let path = request.uri().path().to_owned();
                    let response = next.run(request).await;
                    // Observe terminal Pods before the real dispatcher receives Core's
                    // acknowledgement and performs its normal foreground cleanup.
                    if path == "/complete" {
                        let client = client.lock().unwrap().clone();
                        if let Some(client) = client {
                            diagnostics(&client, &fixture.owner, &fixture.cid, &directory).await;
                        }
                    }
                    if response.status().is_success() {
                        return response;
                    }
                    let (parts, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, 32768).await.unwrap();
                    let error: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    evidence(
                        &directory.join(format!(
                            "worker-http-error-{}.json",
                            chrono::Utc::now().timestamp_nanos_opt().unwrap()
                        )),
                        &json!({"path":path,"status":parts.status.as_u16(),"code":error["error"]}),
                    );
                    axum::response::Response::from_parts(parts, axum::body::Body::from(bytes))
                }
            },
        ));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let control = ControlClient::loopback_test(&format!("http://{address}"), credential).unwrap();
    let client = Client::try_from(kube_config).unwrap();
    *observation_client.lock().unwrap() = Some(client.clone());
    assert!(
        matches!(Api::<Node>::all(client.clone()).list(&ListParams::default()).await,Err(kube::Error::Api(status))if status.code==403)
    );
    assert!(
        matches!(Api::<Secret>::namespaced(client.clone(),"kube-system").get("not-allowed").await,Err(kube::Error::Api(status))if status.code==403)
    );
    let executor = Arc::new(
        KubernetesJobBuildExecutor::new(
            client.clone(),
            KubernetesBuildConfig {
                profile: KubernetesBuildProfile::RootlessIdmapSandbox,
                namespace: NS.into(),
                service_account: "webdock-build-job".into(),
                sandbox_runtime_class: "webdock-build-sandbox".into(),
                runner_image: config.runner_image.clone(),
                storage_cluster_id: config.upload_gateway_root.as_ref().map(|_| cluster.clone()),
                hold_execution: config.upload_gateway_root.is_some(),
                trusted_projects: BTreeSet::from([TrustedProject {
                    customer_id: fixture.cid.clone(),
                    project_id: fixture.pid.clone(),
                }]),
            },
        )
        .unwrap(),
    );
    let upload_gateway = config.upload_gateway_root.as_ref().map(|root| {
        webdock_build::kubernetes_upload::AttemptUploadGateway::open(
            root,
            1073741824,
            "https://167.86.75.144:19444",
            policy.max_result_bytes,
            policy.timeout_seconds,
        )
        .unwrap()
    });
    let upload_server = if let Some(gateway) = upload_gateway.clone() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:19445")
            .await
            .unwrap();
        let diagnostics = directory.clone();
        let router=webdock_build::kubernetes_upload::router(gateway).layer(axum::middleware::from_fn(move |request:axum::extract::Request,next:axum::middleware::Next|{
            let diagnostics=diagnostics.clone();
            async move {
                use tokio_stream::StreamExt;
                let started=Instant::now();let expected=request.headers().get(axum::http::header::CONTENT_LENGTH).and_then(|h|h.to_str().ok()).and_then(|h|h.parse::<u64>().ok());
                let count=Arc::new(std::sync::atomic::AtomicU64::new(0));let sampled=count.clone();
                let (parts,body)=request.into_parts();let body=axum::body::Body::from_stream(body.into_data_stream().map(move |frame|{if let Ok(bytes)=&frame{sampled.fetch_add(bytes.len() as u64,std::sync::atomic::Ordering::Relaxed);}frame}));
                let response=next.run(axum::extract::Request::from_parts(parts,body)).await;
                let (parts,body)=response.into_parts();let body=axum::body::to_bytes(body,256).await.unwrap();
                evidence(&diagnostics.join(format!("gateway-response-{}.json",chrono::Utc::now().timestamp_nanos_opt().unwrap())),&json!({"status":parts.status.as_u16(),"safeCode":String::from_utf8_lossy(&body),"expectedBytes":expected,"receivedBytes":count.load(std::sync::atomic::Ordering::Relaxed),"elapsedSeconds":started.elapsed().as_secs_f64()}));
                axum::response::Response::from_parts(parts,axum::body::Body::from(body))
            }
        }));
        Some(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap()
        }))
    } else {
        None
    };
    let staging_store =
        webdock_object_storage::Store::s3_with_ca(storage, credentials, &ca).unwrap();
    let transfer = Arc::new(if let Some(gateway) = upload_gateway {
        KubernetesStagingTransfer::new_with_upload_gateway(
            staging_store,
            executor.clone(),
            policy,
            directory.join("staging"),
            gateway,
        )
        .unwrap()
    } else {
        KubernetesStagingTransfer::trusted_pilot_only_unbounded_provider_put(
            staging_store,
            executor.clone(),
            policy,
            directory.join("staging"),
        )
        .unwrap()
    });
    let storage_fixture = directory.join("storage-fixture");
    let storage = if config.upload_gateway_root.is_some() {
        private_dir(&storage_fixture);
        evidence(
            &cleanup_path,
            &json!({"containerID":custody["containerID"],"allJobsAbsent":false,"evidenceDirectory":directory}),
        );
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).unwrap();
        let signer = ed25519_dalek::SigningKey::from_bytes(&seed);
        evidence(
            &storage_fixture.join("private-signer.json"),
            &json!({"seed":hex::encode(seed)}),
        );
        let output = Command::new("python3")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tools/tests/build-native-storage.py"),
            )
            .args([
                "start",
                "--state-root",
                storage_fixture.to_str().unwrap(),
                "--cluster-id",
                &cluster,
                "--public-key",
                &hex::encode(signer.verifying_key().to_bytes()),
            ])
            .output()
            .unwrap();
        if !output.status.success() {
            evidence(
                &storage_fixture.join("setup-error.json"),
                &json!({"stderr":String::from_utf8_lossy(&output.stderr)}),
            );
        }
        assert!(
            output.status.success(),
            "owned node fixture setup failed; retain private storage custody"
        );
        let target: webdock_agent_protocol::build_storage::BuildStorageTarget =
            serde_json::from_slice(&output.stdout).unwrap();
        service
            .configure_kubernetes_storage(
                &fixture.principal,
                &cluster,
                Some(webdock_build::KubernetesStoragePolicy {
                    node: target.node.clone(),
                    pool_id: target.pool_id.clone(),
                    volume_bytes: 27 * 1024 * 1024 * 1024,
                }),
                Some(&executor.runtime_fingerprint()),
            )
            .await
            .unwrap();
        Some(
            webdock_build::build_storage::KubernetesBuildStorage::new(
                client.clone(),
                NS.into(),
                "webdock-build-storage-requests".into(),
                "webdock-build-storage-receipts".into(),
                target,
                signer,
            )
            .unwrap(),
        )
    } else {
        None
    };
    service
        .enable_kubernetes_cluster(&fixture.principal, &cluster, true)
        .await
        .unwrap();
    let store = Arc::new(ArtifactStore::open(directory.join("artifacts"), MAX_OUTPUT * 4).unwrap());
    let mut dispatcher = KubernetesBuildDispatcher::new(
        control.clone(),
        executor.as_ref().clone(),
        transfer.clone(),
        store.clone(),
        directory.join("dispatcher"),
        cluster.clone(),
    )
    .unwrap();
    if let Some(storage) = storage.clone() {
        dispatcher = dispatcher.with_storage(storage);
    }
    let cleanup_store = store.clone();
    let hard_storage = config.upload_gateway_root.is_some();
    let connection = id(&fixture.owner).await;
    let f = fixture.clone();
    let live_service = service.clone();
    let git = repository.clone();
    let live_directory = directory.clone();
    let live_worker = worker.clone();
    let live_cluster = cluster.clone();
    let live_executor = executor.clone();
    let live_client = client.clone();
    let live_custody = custody.clone();
    let outcome=tokio::spawn(async move {
        let mut draft=spec();draft.as_object_mut().unwrap().remove("image");draft.as_object_mut().unwrap().remove("template");let target=f.service.execute(&f.principal,json!({"action":"apps.prepare","projectID":f.pid,"name":"owned-live-build","spec":draft,"subscriptionRevision":1,"idempotencyKey":"owned-live-target"})).await.unwrap();let target=target["appID"].as_str().or_else(||target["id"].as_str()).unwrap();
        sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,'1234','1234','owned-local-git','{}','active')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'55','{}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        let strategy=if railpack_build {"railpack"} else {"dockerfile"};
        live_service.execute(&f.principal,json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","executionBackend":"kubernetes","containerBuild":{"strategy":strategy,"contextDirectory":".","dockerfile":"Dockerfile","buildArgs":{"FIXTURE_REVISION":source_sha}},"revision":0,"autoPublish":false,"buildEnvironment":[{"name":"BUILD_TOKEN","value":"owned-live-secret-no-provider-authority"}]})).await.unwrap();
        let first=live_service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":target,"idempotencyKey":"owned-live-build-first"})).await.unwrap();let first=first["id"].as_str().unwrap();
        let next_generation=row(&f.owner,first).await["generation"].as_u64().unwrap()+1;
        if !hard_storage {
        let proposed=KubernetesBuildRequest{customer_id:f.cid.clone(),project_id:f.pid.clone(),build_id:first.into(),generation:next_generation,attempt:next_generation.try_into().unwrap(),commit_sha:source_sha.clone(),volume:None,limits:KubernetesBuildLimits{cpu_millis:2000,memory_mib:4096,workspace_mib:10240,cache_mib:16384,deadline_seconds:1500}};
        let proposed=live_executor.manifest(&proposed).unwrap();evidence(&live_directory.join("exact-job-manifest.json"),&serde_json::to_value(&proposed).unwrap());
        if let Err(kube::Error::Api(response))=Api::<Job>::namespaced(live_client.clone(),NS).create(&PostParams{dry_run:true,..PostParams::default()},&proposed).await {
            // A Job has only fixed controls/IDs and Secret NAME, never capabilities.
            let message=regex::Regex::new(r"https?://[^\s]+" ).unwrap().replace_all(&response.message,"[URL]").to_string();
            evidence(&live_directory.join("job-admission-error.json"),&json!({"code":response.code,"reason":response.reason,"message":message}));
            panic!("exact Job dry-run rejected: status={} reason={} message={}",response.code,response.reason,message);
        }
        }
        evidence(&cleanup_path,&json!({"containerID":live_custody["containerID"],"allJobsAbsent":false,"evidenceDirectory":live_directory}));
        drive(&mut dispatcher,&live_service,&live_worker,&live_cluster,&f.owner,first,config.timeout_seconds).await;
        for attempt in attempts(&f.owner,&f.cid).await {assert!(secret_absent(&live_client,&live_executor,&request(&attempt)).await);}
        let built=row(&f.owner,first).await;assert_eq!(built["source_sha"],source_sha);assert_eq!(built["status"],"artifact-ready");assert!(built["registry_verified_at"].is_null());
        let artifact=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(a) AS value FROM webdock_auth.git_artifact a WHERE build_id=$1",[first.into()])).await.unwrap().unwrap().try_get::<Value>("","value").unwrap();
        let retained=store.load(artifact["storage_key"].as_str().unwrap().into(),artifact["digest"].as_str().unwrap().into(),artifact["size_bytes"].as_u64().unwrap(),artifact["image_digest"].as_str().unwrap().into()).unwrap();let bytes=store.read(&retained).unwrap();let image_digest=if railpack_build {assert_railpack_run(&bytes,&f.nonce,&source_sha)}else {assert_actual_run(&bytes,&f.nonce,&source_sha)};assert_eq!(image_digest,artifact["image_digest"]);
        // Preserve only the verified image for the separate, owned registry/runtime acceptance.
        // Source credentials and attempt capabilities are never copied into this evidence.
        {
            use std::io::Write;
            let envelope=Archive::read(&bytes,MAX_OUTPUT).unwrap();
            let image=envelope.files().get("image.tar").unwrap();
            assert_eq!(validate_oci(image).unwrap(),image_digest);
            let mut file=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(live_directory.join("verified-image.oci.tar")).unwrap();
            file.write_all(image).unwrap();file.sync_all().unwrap();
            std::fs::File::open(&live_directory).unwrap().sync_all().unwrap();
        }
        let log_page=retained_log_pages(&live_service,&f.principal,&f.pid,first).await;let texts=log_page["entries"].as_array().unwrap().iter().filter_map(|entry|entry["text"].as_str()).collect::<Vec<_>>().join("");assert!(texts.contains("owned-real-RUN-proof"));assert!(!texts.contains("owned-live-secret-no-provider-authority"));
        assert!(std::fs::read_dir(live_directory.join("staging")).unwrap().next().is_none(),"handoff state retained after Job cleanup");
        evidence(&live_directory.join("success.json"),&json!({"buildID":first,"customerID":f.cid,"projectID":f.pid,"sourceSHA":source_sha,"imageDigest":image_digest,"artifact":artifact,"logEntries":log_page["entries"],"status":"artifact-ready","registryVerified":false,"repositoryCalls":git.calls.lock().unwrap().clone()}));
        if !railpack_build {
        std::fs::write(git.tree.path().join("Dockerfile"),"FROM scratch\nCOPY fixture /fixture\nRUN [\"/fixture\",\"wait\"]\n").unwrap();let cancelled_sha=git.commit();
        let second=live_service.execute(&f.principal,json!({"action":"git.builds.request","projectID":f.pid,"targetID":target,"idempotencyKey":"owned-live-build-cancel"})).await.unwrap();let second=second["id"].as_str().unwrap();
        let until=Instant::now()+Duration::from_secs(config.timeout_seconds);
        loop {let _=dispatcher.turn().await;let page=live_service.execute(&f.principal,json!({"action":"git.builds.logs","projectID":f.pid,"buildID":second,"after":"0","limit":100})).await.unwrap();if page["entries"].as_array().unwrap().iter().any(|entry|entry["text"].as_str().is_some_and(|text|text.contains("owned-cancel-wait"))){break;}assert!(Instant::now()<until,"actual cancel fixture never entered RUN");tokio::time::sleep(Duration::from_secs(1)).await;}
        assert_eq!(row(&f.owner,second).await["source_sha"],cancelled_sha);
        assert_eq!(row(&f.owner,second).await["status"],"running","cancellation must interrupt a live RUN, not a completed export");
        live_service.execute(&f.principal,json!({"action":"git.builds.cancel","projectID":f.pid,"buildID":second})).await.unwrap();
        let until=Instant::now()+Duration::from_secs(120);
        loop {
            if let Err(error)=dispatcher.turn().await {
                assert!(matches!(error.code,"build_storage_pending"|"kubernetes_unavailable"|"kubernetes_api_unavailable"),"cancellation reconciliation failed safe code={}",error.code);
            }
            if live_service.list_kubernetes_attempts(&live_worker,&live_cluster).await.unwrap().is_empty() {
                break;
            }
            assert!(Instant::now()<until,"cancelled owned Job/Pods remain");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        for attempt in attempts(&f.owner,&f.cid).await {assert!(attempt["retired_at"].as_str().is_some());assert!(live_executor.absent(&request(&attempt),attempt["job_uid"].as_str()).await.unwrap());assert!(secret_absent(&live_client,&live_executor,&request(&attempt)).await);}
        let calls=git.calls.lock().unwrap().clone();assert!(calls.contains(&("archive".into(),source_sha.clone())));assert!(calls.contains(&("archive".into(),cancelled_sha.clone())));
        evidence(&live_directory.join("cancel.json"),&json!({"buildID":second,"sourceSHA":cancelled_sha,"status":row(&f.owner,second).await["status"],"jobAndPodsAbsent":true,"handoffStateAbsent":std::fs::read_dir(live_directory.join("staging")).unwrap().next().is_none(),"repositoryCalls":calls}));
        }
    }).await;
    if outcome.is_err() {
        diagnostics(&client, &fixture.owner, &fixture.cid, &directory).await;
    }
    if let Some(storage) = storage {
        for attempt in service
            .list_kubernetes_attempts(&worker, &cluster)
            .await
            .unwrap()
        {
            let _=service.execute(&fixture.principal,json!({"action":"git.builds.cancel","projectID":fixture.pid,"buildID":attempt["build_id"]})).await;
        }
        let mut cleanup_dispatcher = KubernetesBuildDispatcher::new(
            control,
            executor.as_ref().clone(),
            transfer.clone(),
            cleanup_store,
            directory.join("dispatcher"),
            cluster.clone(),
        )
        .unwrap()
        .with_storage(storage);
        let until = Instant::now() + Duration::from_secs(120);
        while !service
            .list_kubernetes_attempts(&worker, &cluster)
            .await
            .unwrap()
            .is_empty()
            && Instant::now() < until
        {
            let _ = cleanup_dispatcher.turn().await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    let mut absent =
        clean_remote(&client, &fixture.owner, &fixture.cid, &executor, &transfer).await;
    if hard_storage {
        absent &= service
            .list_kubernetes_attempts(&worker, &cluster)
            .await
            .unwrap()
            .is_empty();
        absent &= attempts(&fixture.owner, &fixture.cid)
            .await
            .iter()
            .all(|a| a["storage_retired_at"].is_string());
        if absent {
            let output = Command::new("python3")
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../tools/tests/build-native-storage.py"),
                )
                .args(["stop", "--state-root", storage_fixture.to_str().unwrap()])
                .output()
                .unwrap();
            absent &= output.status.success();
        }
    }
    if let Some(server) = upload_server {
        server.abort();
    }

    let proof_path =
        PathBuf::from(std::env::var_os("WEBDOCK_KUBERNETES_LIVE_CLEANUP_PROOF").unwrap());
    evidence(
        &proof_path,
        &json!({"containerID":custody["containerID"],"allJobsAbsent":absent,"evidenceDirectory":directory}),
    );
    server.abort();
    if absent {
        sql(
            &fixture.owner,
            "UPDATE webdock_auth.git_build_admission SET enabled=false WHERE singleton",
            vec![],
        )
        .await;
        for table in [
            "git_build_log_entry",
            "git_build_attempt",
            "git_build_trusted_project",
            "git_release",
            "git_artifact",
            "git_check_outbox",
            "git_build",
            "git_source",
            "git_repository",
            "git_connection",
        ] {
            let selector = if table == "git_check_outbox" {
                "build_id IN(SELECT id FROM webdock_auth.git_build WHERE customer_id=$1)"
            } else {
                "customer_id=$1"
            };
            sql(
                &fixture.owner,
                &format!("DELETE FROM webdock_auth.{table} WHERE {selector}"),
                vec![fixture.cid.clone().into()],
            )
            .await;
        }
        sql(
            &fixture.owner,
            "DELETE FROM webdock_auth.git_build_cluster WHERE id=$1",
            vec![cluster.into()],
        )
        .await;
        sql(
            &fixture.owner,
            "DELETE FROM webdock_auth.git_worker WHERE id=$1",
            vec![worker.id().into()],
        )
        .await;
        fixture.cleanup().await;
    }
    assert!(
        absent,
        "owned cluster cleanup not proven; harness must retain its PostgreSQL ledger"
    );
    outcome.unwrap();
}
