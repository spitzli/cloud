//! Attempt-scoped staging only. HTTPS capabilities and uploaded bytes never grant release authority.
use crate::{
    Result,
    artifact::{Archive, MAX_ENTRIES, MAX_OUTPUT, MAX_SOURCE, validate_oci},
    container_runner::{BuildSecrets, ContainerBuildRequest, read_input_file},
    error,
    execution::{BuildOutput, MAX_LOG},
    io,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    time::Duration,
};
use url::Url;
pub const MAX_TRANSFER_SOURCE: u64 = MAX_SOURCE + MAX_ENTRIES as u64 * 1024 + 10240;
pub const MAX_HANDOFF_BYTES: u64 = 96 * 1024;
const MAX_EVENTS: u64 = MAX_LOG as u64 * 6 + 8192;
const fn tar_member_bytes(payload: u64) -> u64 {
    512 + payload.div_ceil(512) * 512
}
// Five short USTAR paths: exact maximum padded members, five headers and two end records.
pub const MAX_TRANSFER_RESULT: u64 = tar_member_bytes(MAX_OUTPUT)
    + 2 * tar_member_bytes(4096)
    + tar_member_bytes(MAX_LOG as u64)
    + tar_member_bytes(MAX_EVENTS)
    + 1024;
/// Operator policy is independent of the per-attempt capability file. Never infer it from a URL.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferPolicy {
    pub allowed_origins: Vec<String>,
    #[serde(default, rename = "certificateAuthorityPEM")]
    pub certificate_authority_pem: Option<String>,
    pub max_source_bytes: u64,
    pub max_result_bytes: u64,
    pub timeout_seconds: u64,
}
/// Confidential, immutable handoff, delivered through the operator's read-only staging channel.
/// Deliberately no Debug implementation: signed query strings and customer secrets are capabilities.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptTransfer {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "buildID")]
    pub build_id: String,
    pub generation: u64,
    pub request: ContainerBuildRequest,
    pub source_size_bytes: u64,
    pub source_url: String,
    pub result_put_url: String,
    #[serde(default)]
    pub build_secrets: BTreeMap<String, String>,
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn parse_url(value: &str, permit_http: bool) -> Result<Url> {
    if value.len() > 8192 {
        return Err(error("invalid_transfer_url"));
    }
    let url = Url::parse(value).map_err(|_| error("invalid_transfer_url"))?;
    let test_http = permit_http && url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    if (url.scheme() != "https" && !test_http)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(error("invalid_transfer_url"));
    }
    Ok(url)
}
impl TransferPolicy {
    fn validate(&self, permit_http: bool) -> Result<()> {
        if self.allowed_origins.is_empty()
            || self.allowed_origins.len() > 8
            || self.max_source_bytes == 0
            || self.max_source_bytes > MAX_TRANSFER_SOURCE
            || self.max_result_bytes == 0
            || self.max_result_bytes > MAX_TRANSFER_RESULT
            || !(1..=300).contains(&self.timeout_seconds)
        {
            return Err(error("invalid_transfer_policy"));
        }
        for value in &self.allowed_origins {
            let url = parse_url(value, permit_http)?;
            if url.path() != "/" || url.query().is_some() {
                return Err(error("invalid_transfer_policy"));
            }
        }
        Ok(())
    }
    fn url(&self, value: &str, suffix: &str, permit_http: bool) -> Result<Url> {
        let url = parse_url(value, permit_http)?;
        if !url.path().ends_with(&format!("/{suffix}"))
            || !self
                .allowed_origins
                .iter()
                .any(|origin| Url::parse(origin).is_ok_and(|o| o.origin() == url.origin()))
        {
            return Err(error("transfer_scope_mismatch"));
        }
        Ok(url)
    }
}
impl AttemptTransfer {
    pub fn keys(&self) -> Result<(String, String)> {
        self.validate_identity()?;
        let prefix = format!(
            "build-attempts/{}/{}/{}/{}",
            self.customer_id, self.project_id, self.build_id, self.generation
        );
        Ok((
            format!("{prefix}/source.tar"),
            format!("{prefix}/result.tar"),
        ))
    }
    fn validate_identity(&self) -> Result<()> {
        if !identifier(&self.customer_id)
            || !identifier(&self.project_id)
            || !identifier(&self.build_id)
            || self.generation == 0
        {
            return Err(error("invalid_transfer_identity"));
        }
        self.request.validate()?;
        BuildSecrets::new(self.build_secrets.clone())?;
        if self
            .build_secrets
            .keys()
            .any(|k| self.request.build_args.contains_key(k))
        {
            return Err(error("build_secret_argument_conflict"));
        }
        if serde_json::to_vec(self)
            .map_err(|_| error("invalid_transfer_configuration"))?
            .len() as u64
            > MAX_HANDOFF_BYTES
        {
            return Err(error("transfer_configuration_limit"));
        }
        Ok(())
    }
    fn validate_inner(&self, policy: &TransferPolicy, permit_http: bool) -> Result<()> {
        policy.validate(permit_http)?;
        let (source, result) = self.keys()?;
        if self.source_size_bytes == 0 || self.source_size_bytes > policy.max_source_bytes {
            return Err(error("source_transfer_limit"));
        }
        policy.url(&self.source_url, &source, permit_http)?;
        policy.url(&self.result_put_url, &result, permit_http)?;
        Ok(())
    }
    pub fn validate(&self, policy: &TransferPolicy) -> Result<()> {
        self.validate_inner(policy, false)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResultIdentity {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "buildID")]
    pub build_id: String,
    pub generation: u64,
    pub source_sha: String,
    pub archive_sha256: String,
    pub job_uid: String,
}
impl ResultIdentity {
    pub fn for_attempt(attempt: &AttemptTransfer, job_uid: &str) -> Result<Self> {
        attempt.validate_identity()?;
        if !identifier(job_uid) {
            return Err(error("invalid_transfer_job_identity"));
        }
        Ok(Self {
            customer_id: attempt.customer_id.clone(),
            project_id: attempt.project_id.clone(),
            build_id: attempt.build_id.clone(),
            generation: attempt.generation,
            source_sha: attempt.request.source_sha.clone(),
            archive_sha256: attempt.request.archive_sha256.clone(),
            job_uid: job_uid.into(),
        })
    }
}
/// Presigned URLs are bearer capabilities; errors intentionally never preserve reqwest URL text.
pub struct TransferClient {
    client: reqwest::Client,
    policy: TransferPolicy,
    permit_http: bool,
}
impl TransferClient {
    pub fn new(policy: TransferPolicy) -> Result<Self> {
        Self::construct(policy, false)
    }
    fn construct(policy: TransferPolicy, permit_http: bool) -> Result<Self> {
        policy.validate(permit_http)?;
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let mut client = reqwest::Client::builder();
        if let Some(pem) = &policy.certificate_authority_pem {
            if pem.len() > 16 * 1024 {
                return Err(error("invalid_transfer_ca"));
            }
            use rustls::pki_types::{CertificateDer, pem::PemObject};
            let certificates = CertificateDer::pem_slice_iter(pem.as_bytes())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| error("invalid_transfer_ca"))?;
            if certificates.is_empty() {
                return Err(error("invalid_transfer_ca"));
            }
            let mut roots = rustls::RootCertStore::empty();
            for certificate in certificates {
                roots
                    .add(certificate)
                    .map_err(|_| error("invalid_transfer_ca"))?;
            }
            let certificate = reqwest::Certificate::from_pem(pem.as_bytes())
                .map_err(|_| error("invalid_transfer_ca"))?;
            client = client.add_root_certificate(certificate);
        }
        let client = client
            .no_proxy()
            .https_only(!permit_http)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(policy.timeout_seconds))
            .build()
            .map_err(|_| error("transfer_client_unavailable"))?;
        Ok(Self {
            client,
            policy,
            permit_http,
        })
    }
    #[cfg(test)]
    fn for_test(policy: TransferPolicy) -> Result<Self> {
        Self::construct(policy, true)
    }
    /// Init-container operation. No final input tree appears until exact digest and archive validation pass.
    pub async fn download(&self, attempt: &AttemptTransfer, input: &Path) -> Result<()> {
        attempt.validate_inner(&self.policy, self.permit_http)?;
        if !input.is_absolute() || input.symlink_metadata().is_ok() {
            return Err(error("invalid_transfer_staging"));
        }
        let parent = input
            .parent()
            .ok_or_else(|| error("invalid_transfer_staging"))?;
        if parent.canonicalize().map_err(io)? != parent {
            return Err(error("invalid_transfer_staging"));
        }
        let stage = tempfile::tempdir_in(parent).map_err(io)?;
        std::fs::set_permissions(stage.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(io)?;
        let path = stage.path().join("source.tar");
        let mut file = create(&path)?;
        let (source_key, _) = attempt.keys()?;
        let url = self
            .policy
            .url(&attempt.source_url, &source_key, self.permit_http)?;
        let mut response = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|_| error("source_transfer_failed"))?;
        if response.status() != reqwest::StatusCode::OK
            || response
                .content_length()
                .is_some_and(|n| n != attempt.source_size_bytes)
            || response
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .is_some_and(|v| v != "identity")
        {
            return Err(error("source_transfer_failed"));
        }
        let mut hash = Sha256::new();
        let mut count = 0u64;
        while let Some(bytes) = response
            .chunk()
            .await
            .map_err(|_| error("source_transfer_failed"))?
        {
            count = count
                .checked_add(bytes.len() as u64)
                .filter(|n| *n <= attempt.source_size_bytes && *n <= self.policy.max_source_bytes)
                .ok_or_else(|| error("source_transfer_limit"))?;
            hash.update(&bytes);
            file.write_all(&bytes).map_err(io)?;
        }
        if count != attempt.source_size_bytes
            || attempt.request.archive_sha256 != format!("sha256:{}", hex::encode(hash.finalize()))
        {
            return Err(error("source_digest_mismatch"));
        }
        file.sync_all().map_err(io)?;
        drop(file);
        Archive::read(
            &read_input_file(&path, self.policy.max_source_bytes)?,
            MAX_SOURCE.min(attempt.request.limits.disk_bytes),
        )?;
        write_json(&stage.path().join("request.json"), &attempt.request)?;
        write_json(
            &stage.path().join("source.json"),
            &serde_json::json!({"sourceSha":attempt.request.source_sha,"archiveSha256":attempt.request.archive_sha256}),
        )?;
        if !attempt.build_secrets.is_empty() {
            write_json(&stage.path().join("secrets.json"), &attempt.build_secrets)?;
        }
        // Init runs before any customer process; the private parent has one trusted writer.
        std::fs::rename(stage.path(), input).map_err(io)?;
        let _ = stage.keep();
        Ok(())
    }
    /// Uploads only this attempt's bounded output. It never reads arbitrary source-selected paths.
    pub async fn upload(
        &self,
        attempt: &AttemptTransfer,
        job_uid: &str,
        output: &Path,
    ) -> Result<()> {
        attempt.validate_inner(&self.policy, self.permit_http)?;
        let bytes = package_result(attempt, job_uid, output)?;
        if bytes.len() as u64 > self.policy.max_result_bytes {
            return Err(error("result_transfer_limit"));
        }
        let (_, result_key) = attempt.keys()?;
        let url = self
            .policy
            .url(&attempt.result_put_url, &result_key, self.permit_http)?;
        let response = self
            .client
            .put(url)
            .header(reqwest::header::CONTENT_TYPE, "application/x-tar")
            .body(bytes)
            .send()
            .await
            .map_err(|_| error("result_transfer_failed"))?;
        if !response.status().is_success() {
            return Err(error("result_transfer_failed"));
        }
        Ok(())
    }
}
fn create(path: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io)
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| error("invalid_transfer_configuration"))?;
    let mut file = create(path)?;
    file.write_all(&bytes).map_err(io)?;
    file.sync_all().map_err(io)
}
fn package_result(attempt: &AttemptTransfer, job_uid: &str, output: &Path) -> Result<Vec<u8>> {
    let identity = ResultIdentity::for_attempt(attempt, job_uid)?;
    let status = read_input_file(&output.join("status.json"), 4096)?;
    let status_json: serde_json::Value =
        serde_json::from_slice(&status).map_err(|_| error("invalid_transfer_result"))?;
    if !status_json["success"].is_boolean() {
        return Err(error("invalid_transfer_result"));
    }
    let mut files = BTreeMap::from([
        (
            "attempt.json".into(),
            serde_json::to_vec(&identity).map_err(|_| error("invalid_transfer_result"))?,
        ),
        ("status.json".into(), status),
    ]);
    for (name, limit) in [("build.log", MAX_LOG as u64), ("events.jsonl", MAX_EVENTS)] {
        let path = output.join(name);
        let bytes = if path.try_exists().map_err(io)? {
            read_input_file(&path, limit)?
        } else {
            Vec::new()
        };
        files.insert(name.into(), bytes);
    }
    if status_json["success"] == true {
        files.insert(
            "image.tar".into(),
            read_input_file(&output.join("image.tar"), MAX_OUTPUT)?,
        );
    }
    let bytes = Archive::from_files(files)?.encode()?;
    if bytes.len() as u64 > MAX_TRANSFER_RESULT {
        return Err(error("result_transfer_limit"));
    }
    // This is defense in depth; the trusted collector must independently repeat all checks.
    match verify_result(attempt, job_uid, &bytes) {
        Ok(_) => {}
        Err(e) if e.code == "execution_failed" => {}
        Err(e) => return Err(e),
    }
    Ok(bytes)
}
/// Trusted collector entrypoint: compare expected IDs and actual Job UID, then revalidate every OCI blob.
/// A successful PUT, a source receipt, and Job stdout are never completion authority.
pub fn verify_result(
    attempt: &AttemptTransfer,
    expected_job_uid: &str,
    bytes: &[u8],
) -> Result<BuildOutput> {
    if bytes.len() as u64 > MAX_TRANSFER_RESULT {
        return Err(error("result_transfer_limit"));
    }
    let expected = ResultIdentity::for_attempt(attempt, expected_job_uid)?;
    let archive = Archive::read(bytes, MAX_TRANSFER_RESULT)?;
    let files = archive.files();
    if files.keys().any(|k| {
        ![
            "attempt.json",
            "status.json",
            "image.tar",
            "build.log",
            "events.jsonl",
        ]
        .contains(&k.as_str())
    }) {
        return Err(error("invalid_transfer_result"));
    }
    let identity = files
        .get("attempt.json")
        .filter(|b| b.len() <= 4096)
        .ok_or_else(|| error("invalid_transfer_result"))?;
    let identity: ResultIdentity =
        serde_json::from_slice(identity).map_err(|_| error("invalid_transfer_result"))?;
    if identity != expected {
        return Err(error("transfer_identity_mismatch"));
    }
    let status = files
        .get("status.json")
        .filter(|b| b.len() <= 4096)
        .ok_or_else(|| error("invalid_transfer_result"))?;
    let status: serde_json::Value =
        serde_json::from_slice(status).map_err(|_| error("invalid_transfer_result"))?;
    let logs = files
        .get("build.log")
        .filter(|b| b.len() <= MAX_LOG)
        .ok_or_else(|| error("invalid_transfer_result"))?;
    let logs = String::from_utf8(logs.clone()).map_err(|_| error("invalid_transfer_result"))?;
    if files
        .get("events.jsonl")
        .is_none_or(|b| b.len() as u64 > MAX_EVENTS)
    {
        return Err(error("invalid_transfer_result"));
    }
    if status["success"] == false {
        return Err(crate::Error {
            code: "execution_failed",
            logs,
        });
    }
    if status["success"] != true
        || status["sourceSha"] != attempt.request.source_sha
        || status["archiveSha256"] != attempt.request.archive_sha256
    {
        return Err(error("transfer_identity_mismatch"));
    }
    let artifact = files
        .get("image.tar")
        .filter(|b| b.len() as u64 <= MAX_OUTPUT)
        .ok_or_else(|| error("invalid_transfer_result"))?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(artifact)));
    let image_digest = validate_oci(artifact)?;
    if status["artifactDigest"] != digest
        || status["imageDigest"] != image_digest
        || status["artifactBytes"] != artifact.len() as u64
    {
        return Err(error("artifact_digest_mismatch"));
    }
    Ok(BuildOutput {
        artifact: artifact.clone(),
        digest,
        image_digest,
        logs,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{artifact::Archive, container_runner::Strategy, execution::Limits};
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::TcpListener,
    };
    pub(crate) fn source() -> Vec<u8> {
        Archive::from_files(BTreeMap::from([(
            "Dockerfile".into(),
            b"FROM scratch\n".to_vec(),
        )]))
        .unwrap()
        .encode()
        .unwrap()
    }
    pub(crate) fn attempt(origin: &str, bytes: &[u8]) -> AttemptTransfer {
        AttemptTransfer {
            customer_id: "customer".into(),
            project_id: "project".into(),
            build_id: "build".into(),
            generation: 1,
            request: ContainerBuildRequest {
                source_sha: "a".repeat(40),
                archive_sha256: format!("sha256:{}", hex::encode(Sha256::digest(bytes))),
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
            source_size_bytes: bytes.len() as u64,
            source_url: format!(
                "{origin}/bucket/build-attempts/customer/project/build/1/source.tar?cap=source-secret"
            ),
            result_put_url: format!(
                "{origin}/bucket/build-attempts/customer/project/build/1/result.tar?cap=result-secret"
            ),
            build_secrets: BTreeMap::new(),
        }
    }
    fn policy(origin: &str) -> TransferPolicy {
        TransferPolicy {
            allowed_origins: vec![origin.into()],
            certificate_authority_pem: None,
            max_source_bytes: MAX_TRANSFER_SOURCE,
            max_result_bytes: MAX_TRANSFER_RESULT,
            timeout_seconds: 30,
        }
    }
    fn server(
        status: &str,
        headers: &str,
        body: Vec<u8>,
    ) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_owned();
        let headers = headers.to_owned();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&request[..end]);
                    let length = header
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                assert!(request.len() < 1024 * 1024);
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            request
        });
        (origin, handle)
    }
    #[test]
    fn result_ceiling_is_the_exact_maximum_ustar_bundle() {
        let files = BTreeMap::from([
            ("attempt.json".into(), vec![0; 4096]),
            ("status.json".into(), vec![0; 4096]),
            ("build.log".into(), vec![0; MAX_LOG]),
            ("events.jsonl".into(), vec![0; MAX_EVENTS as usize]),
            ("image.tar".into(), vec![0; 1024]),
        ]);
        let bytes = Archive::from_files(files).unwrap().encode().unwrap();
        assert_eq!(MAX_TRANSFER_RESULT, MAX_OUTPUT + bytes.len() as u64 - 1024);
    }
    #[test]
    fn optional_private_ca_requires_valid_pem_and_never_relaxes_https() {
        let mut p = policy("https://objects.example");
        p.certificate_authority_pem = Some("not a CA certificate".into());
        assert!(TransferClient::new(p).is_err());
        let mut p = policy("http://127.0.0.1:9000");
        p.certificate_authority_pem =
            Some(include_str!("../tests/fixtures/kubernetes-worker-ca.crt").into());
        assert!(TransferClient::new(p).is_err());
    }
    #[test]
    fn production_policy_rejects_http_credentials_redirect_targets_and_foreign_attempts() {
        let bytes = source();
        let origin = "https://objects.example";
        let p = policy(origin);
        let a = attempt(origin, &bytes);
        a.validate(&p).unwrap();
        assert!(TransferClient::new(policy("http://127.0.0.1:9000")).is_err());
        for url in [
            "https://evil.example/bucket/build-attempts/customer/project/build/1/source.tar?cap=hidden",
            "https://user:password@objects.example/bucket/build-attempts/customer/project/build/1/source.tar",
            "https://objects.example/bucket/build-attempts/customer/project/build/2/source.tar",
            "https://objects.example/bucket/build-attempts/other/project/build/1/source.tar",
        ] {
            let mut a = a.clone();
            a.source_url = url.into();
            assert!(a.validate(&p).is_err());
        }
    }
    #[tokio::test]
    async fn download_writes_only_verified_source_and_derived_metadata() {
        let bytes = source();
        let (origin, handle) = server("200 OK", "", bytes.clone());
        let a = attempt(&origin, &bytes);
        let client = TransferClient::for_test(policy(&origin)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        client.download(&a, &input).await.unwrap();
        assert_eq!(std::fs::read(input.join("source.tar")).unwrap(), bytes);
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(input.join("source.json")).unwrap()).unwrap();
        assert_eq!(metadata["sourceSha"], "a".repeat(40));
        assert!(
            !String::from_utf8_lossy(&std::fs::read(input.join("request.json")).unwrap())
                .contains("source-secret")
        );
        let request = handle.join().unwrap();
        assert!(request.starts_with(b"GET /bucket/build-attempts/customer/project/build/1/source.tar?cap=source-secret HTTP/1.1"));
    }
    #[tokio::test]
    async fn source_digest_mismatch_never_publishes_input_and_redirect_never_follows() {
        let expected = source();
        let mut tampered = expected.clone();
        tampered[100] ^= 1;
        let (origin, handle) = server("200 OK", "", tampered);
        let a = attempt(&origin, &expected);
        let client = TransferClient::for_test(policy(&origin)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        assert_eq!(
            client.download(&a, &input).await.err().unwrap().code,
            "source_digest_mismatch"
        );
        assert!(!input.exists());
        handle.join().unwrap();
        let (origin, handle) = server(
            "302 Found",
            "Location: https://forbidden.invalid/leak?cap=do-not-log\r\n",
            vec![],
        );
        let a = attempt(&origin, &expected);
        let client = TransferClient::for_test(policy(&origin)).unwrap();
        let err = client.download(&a, &input).await.err().unwrap();
        assert_eq!(err.code, "source_transfer_failed");
        assert!(!err.logs.contains("cap="));
        assert!(!input.exists());
        handle.join().unwrap();
    }
    #[test]
    fn result_identity_and_full_oci_are_checked_again_by_collector() {
        let bytes = source();
        let a = attempt("https://objects.example", &bytes);
        let identity = ResultIdentity::for_attempt(&a, "job-uid").unwrap();
        let bundle = Archive::from_files(BTreeMap::from([
            (
                "attempt.json".into(),
                serde_json::to_vec(&identity).unwrap(),
            ),
            ("status.json".into(), serde_json::json!({"success":true,"sourceSha":a.request.source_sha,"archiveSha256":a.request.archive_sha256}).to_string().into_bytes()),
            ("image.tar".into(), b"not an OCI archive".to_vec()),
            ("build.log".into(), b"bounded log".to_vec()),
            ("events.jsonl".into(), vec![]),
        ]))
        .unwrap()
        .encode()
        .unwrap();
        assert_eq!(
            verify_result(&a, "job-uid", &bundle).err().unwrap().code,
            "archive_truncated"
        );
        assert_eq!(
            verify_result(&a, "foreign-job", &bundle)
                .err()
                .unwrap()
                .code,
            "transfer_identity_mismatch"
        );
    }
    fn oci() -> Vec<u8> {
        let config = b"{}";
        let config_hash = hex::encode(Sha256::digest(config));
        let manifest=serde_json::json!({"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":2,"digest":format!("sha256:{config_hash}")},"layers":[]}).to_string().into_bytes();
        let hash = hex::encode(Sha256::digest(&manifest));
        Archive::from_files(BTreeMap::from([("oci-layout".into(),br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec()),("index.json".into(),serde_json::json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{hash}")}]}).to_string().into_bytes()),(format!("blobs/sha256/{hash}"),manifest),(format!("blobs/sha256/{config_hash}"),config.to_vec())])).unwrap().encode().unwrap()
    }
    fn output(attempt: &AttemptTransfer, root: &Path, image: &[u8]) {
        std::fs::write(root.join("image.tar"), image).unwrap();
        let image_digest = validate_oci(image).unwrap();
        let status = serde_json::json!({"success":true,"sourceSha":attempt.request.source_sha,"archiveSha256":attempt.request.archive_sha256,"artifactDigest":format!("sha256:{}",hex::encode(Sha256::digest(image))),"imageDigest":image_digest,"artifactBytes":image.len()});
        std::fs::write(root.join("status.json"), status.to_string()).unwrap();
        std::fs::write(root.join("build.log"), "bounded log").unwrap();
        std::fs::write(root.join("events.jsonl"), "").unwrap();
    }
    #[tokio::test]
    async fn upload_is_one_own_attempt_put_and_collector_rejects_consistent_outer_hash_with_bad_blob()
     {
        let source = source();
        let image = oci();
        let (origin, handle) = server("200 OK", "", vec![]);
        let a = attempt(&origin, &source);
        let client = TransferClient::for_test(policy(&origin)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        output(&a, dir.path(), &image);
        client.upload(&a, "job-uid", dir.path()).await.unwrap();
        let request = handle.join().unwrap();
        assert!(request.starts_with(b"PUT /bucket/build-attempts/customer/project/build/1/result.tar?cap=result-secret HTTP/1.1"));
        let start = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let received = &request[start..];
        let verified = verify_result(&a, "job-uid", received).unwrap();
        assert_eq!(verified.artifact, image);
        assert_eq!(verified.logs, "bounded log");
        let mut result = Archive::read(received, MAX_TRANSFER_RESULT)
            .unwrap()
            .into_files();
        let mut image_files = Archive::read(&image, MAX_OUTPUT).unwrap().into_files();
        let config_hash = hex::encode(Sha256::digest(b"{}"));
        image_files.insert(format!("blobs/sha256/{config_hash}"), b"{changed}".to_vec());
        let tampered = Archive::from_files(image_files).unwrap().encode().unwrap();
        let mut status: serde_json::Value = serde_json::from_slice(&result["status.json"]).unwrap();
        status["artifactDigest"] =
            format!("sha256:{}", hex::encode(Sha256::digest(&tampered))).into();
        status["artifactBytes"] = (tampered.len() as u64).into();
        result.insert("status.json".into(), status.to_string().into_bytes());
        result.insert("image.tar".into(), tampered);
        assert_eq!(
            verify_result(
                &a,
                "job-uid",
                &Archive::from_files(result).unwrap().encode().unwrap()
            )
            .err()
            .unwrap()
            .code,
            "oci_digest_mismatch"
        );
    }
    #[tokio::test]
    async fn caps_are_enforced_before_network_and_failure_results_retain_identity_and_logs() {
        let source = source();
        let a = attempt("http://127.0.0.1:9", &source);
        let mut limited = policy("http://127.0.0.1:9");
        limited.max_source_bytes = 1;
        let client = TransferClient::for_test(limited).unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            client
                .download(&a, &dir.path().join("input"))
                .await
                .err()
                .unwrap()
                .code,
            "source_transfer_limit"
        );
        let output_dir = dir.path().join("output");
        std::fs::create_dir(&output_dir).unwrap();
        output(&a, &output_dir, &oci());
        let mut limited = policy("http://127.0.0.1:9");
        limited.max_result_bytes = 1;
        let client = TransferClient::for_test(limited).unwrap();
        assert_eq!(
            client
                .upload(&a, "job-uid", &output_dir)
                .await
                .err()
                .unwrap()
                .code,
            "result_transfer_limit"
        );
        std::fs::write(
            output_dir.join("status.json"),
            br#"{"success":false,"errorCode":"execution_failed"}"#,
        )
        .unwrap();
        let bundle = package_result(&a, "job-uid", &output_dir).unwrap();
        let error = verify_result(&a, "job-uid", &bundle).err().unwrap();
        assert_eq!(error.code, "execution_failed");
        assert_eq!(error.logs, "bounded log");
        assert_eq!(
            verify_result(&a, "other-job", &bundle).err().unwrap().code,
            "transfer_identity_mismatch"
        );
    }
}
