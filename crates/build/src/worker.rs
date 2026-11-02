//! Standalone worker transport; enrollment secrets never enter guest requests or redirects.
use crate::{
    Result,
    artifact::{ArtifactStore, MAX_OUTPUT, MAX_SOURCE},
    error,
    execution::{GuestRequest, VmImage},
    io,
    publisher::{PublicationGuard, RegistryCredential, RegistryPublisher},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
#[derive(Clone)]
pub struct ControlClient {
    base: url::Url,
    credential: String,
    client: reqwest::Client,
}
impl ControlClient {
    pub fn new(base: &str, credential: String) -> Result<Self> {
        Self::configured(base, credential, false)
    }
    #[cfg(feature = "local-registry-test")]
    pub fn loopback_test(base: &str, credential: String) -> Result<Self> {
        Self::configured(base, credential, true)
    }
    fn configured(base: &str, credential: String, loopback: bool) -> Result<Self> {
        let mut base = url::Url::parse(base).map_err(|_| error("invalid_control_origin"))?;
        let local = base
            .host_str()
            .and_then(|v| v.parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if (base.scheme() != "https" && !(loopback && base.scheme() == "http" && local))
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || credential.len() != 43
            || !credential
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(error("invalid_control_configuration"));
        }
        base.set_path(base.path().trim_end_matches('/').to_owned().as_str());
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| error("control_unavailable"))?;
        Ok(Self {
            base,
            credential,
            client,
        })
    }
    async fn fetch(
        &self,
        path: &str,
        body: Option<&Value>,
        limit: u64,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        if !path.starts_with('/') || path.starts_with("//") || path.contains(['#', '\\']) {
            return Err(error("invalid_control_path"));
        }
        let url = format!("{}{}", self.base.as_str().trim_end_matches('/'), path);
        let mut request = if let Some(body) = body {
            self.client.post(url).json(body)
        } else {
            self.client.get(url)
        };
        request = request.bearer_auth(&self.credential).timeout(timeout);
        let mut response = request
            .send()
            .await
            .map_err(|_| error("control_request_failed"))?;
        if !response.status().is_success() {
            if response.status() == reqwest::StatusCode::BAD_REQUEST
                && path
                    .split('?')
                    .next()
                    .is_some_and(|p| p.ends_with("/archive"))
            {
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| error("control_request_failed"))?
                {
                    if bytes.len() + chunk.len() > 1024 {
                        return Err(error("control_request_failed"));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if matches!(
                    body["error"].as_str(),
                    Some(
                        "source_limit"
                            | "archive_limit"
                            | "archive_entry_limit"
                            | "repository_archive_limit"
                    )
                ) {
                    return Err(error("source_limit"));
                }
                return Err(error("control_request_failed"));
            }
            return Err(error(match response.status().as_u16() {
                401 | 403 => "worker_revoked",
                409 => "lease_changed",
                _ => "control_request_failed",
            }));
        }
        if response.content_length().is_some_and(|n| n > limit) {
            return Err(error("control_response_limit"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| error("control_request_failed"))?
        {
            if bytes.len() as u64 + chunk.len() as u64 > limit {
                return Err(error("control_response_limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    pub async fn post(&self, endpoint: &str, body: &Value) -> Result<Value> {
        if ![
            "capabilities",
            "claim",
            "kubernetes-log",
            "kubernetes-claim",
            "kubernetes-attempts",
            "kubernetes-storage",
            "kubernetes-environment",
            "kubernetes-volume",
            "kubernetes-volume-retired",
            "kubernetes-intent",
            "kubernetes-job",
            "kubernetes-retire",
            "complete",
            "release-claim",
            "release-prepare",
            "container-publish",
            "observe",
            "release-complete",
            "retention",
            "retention-complete",
            "orphan-retention",
        ]
        .contains(&endpoint)
        {
            return Err(error("invalid_control_path"));
        }
        let bytes = self
            .fetch(
                &format!("/{endpoint}"),
                Some(body),
                1024 * 1024,
                Duration::from_secs(30),
            )
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| error("invalid_control_response"))
    }
    pub async fn guard_build(&self, id: &str, generation: i32) -> Result<()> {
        self.guard("builds", id, generation).await
    }
    pub async fn guard_release(&self, id: &str, generation: i32) -> Result<()> {
        self.guard("releases", id, generation).await
    }
    async fn guard(&self, kind: &str, id: &str, generation: i32) -> Result<()> {
        crate::contracts::id(id)?;
        if generation <= 0 {
            return Err(error("invalid_generation"));
        }
        let bytes = self
            .fetch(
                &format!("/{kind}/{id}/lease?generation={generation}"),
                None,
                1024,
                Duration::from_secs(5),
            )
            .await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| error("invalid_control_response"))?;
        if value != json!({"active":true}) {
            return Err(error("lease_changed"));
        }
        Ok(())
    }
    pub async fn archive(&self, id: &str, generation: i32) -> Result<Vec<u8>> {
        crate::contracts::id(id)?;
        if generation <= 0 {
            return Err(error("invalid_generation"));
        }
        self.fetch(
            &format!("/builds/{id}/archive?generation={generation}"),
            None,
            MAX_SOURCE,
            Duration::from_secs(60),
        )
        .await
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: u8,
    endpoint: String,
    body: Value,
}
/// Private exact-request outbox. A completion is fsynced before its first HTTP request.
pub struct Outbox {
    root: PathBuf,
}
impl Outbox {
    pub fn open(root: PathBuf) -> Result<Self> {
        let m = root.symlink_metadata().map_err(io)?;
        // SAFETY: geteuid has no preconditions or pointer arguments.
        if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
            return Err(error("private_outbox_required"));
        }
        Ok(Self {
            root: root.canonicalize().map_err(io)?,
        })
    }
    pub fn has_pending(&self) -> bool {
        self.root.join("pending.json").exists()
    }
    pub fn put(&self, endpoint: &str, body: Value) -> Result<()> {
        if !["complete", "container-publish", "release-complete"].contains(&endpoint)
            || self.has_pending()
        {
            return Err(error("pending_completion"));
        }
        let bytes = serde_json::to_vec(&Pending {
            version: 1,
            endpoint: endpoint.into(),
            body,
        })
        .map_err(|_| error("invalid_completion"))?;
        if bytes.len() > 400000 {
            return Err(error("completion_limit"));
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.root).map_err(io)?;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        file.as_file().sync_all().map_err(io)?;
        file.persist_noclobber(self.root.join("pending.json"))
            .map_err(|_| error("pending_completion"))?;
        std::fs::File::open(&self.root)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        Ok(())
    }
    /// Caller must hold its worker-directory lock, retain the abandoned attempt receipt,
    /// and prove authoritative cancellation/lease loss before discarding this exact request.
    /// A transport failure alone never authorizes dropping a pending completion.
    pub fn discard_exact(&self, endpoint: &str, body: &Value) -> Result<()> {
        let path = self.root.join("pending.json");
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io(e)),
        };
        let meta = file.metadata().map_err(io)?;
        // SAFETY: geteuid takes no arguments and has no preconditions.
        let current_uid = unsafe { libc::geteuid() };
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.mode() & 0o077 != 0
            || meta.uid() != current_uid
            || meta.len() > 400000
        {
            return Err(error("invalid_outbox"));
        }
        let mut bytes = Vec::new();
        file.take(400001).read_to_end(&mut bytes).map_err(io)?;
        if bytes.len() > 400000 {
            return Err(error("invalid_outbox"));
        }
        let pending: Pending =
            serde_json::from_slice(&bytes).map_err(|_| error("invalid_outbox"))?;
        if pending.version != 1
            || pending.endpoint != endpoint
            || pending.body != *body
            || !["complete", "container-publish", "release-complete"].contains(&endpoint)
        {
            return Err(error("completion_conflict"));
        }
        std::fs::remove_file(path).map_err(io)?;
        std::fs::File::open(&self.root)
            .map_err(io)?
            .sync_all()
            .map_err(io)
    }
    fn pending_publication(&self) -> Result<Option<(String, Value)>> {
        let path = self.root.join("pending.json");
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let meta = file.metadata().map_err(io)?;
        if !meta.is_file() || meta.mode() & 0o077 != 0 || meta.nlink() != 1 || meta.len() > 400000 {
            return Err(error("invalid_outbox"));
        }
        let mut bytes = Vec::new();
        file.take(400001).read_to_end(&mut bytes).map_err(io)?;
        let pending: Pending =
            serde_json::from_slice(&bytes).map_err(|_| error("invalid_outbox"))?;
        if pending.version != 1
            || !["container-publish", "release-complete"].contains(&pending.endpoint.as_str())
        {
            return Err(error("pending_completion"));
        }
        Ok(Some((pending.endpoint, pending.body)))
    }
    fn record_abandoned_publication(
        &self,
        endpoint: &str,
        body: &Value,
        reason: &str,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(
            &json!({"version":1,"endpoint":endpoint,"body":body,"reason":reason}),
        )
        .map_err(|_| error("invalid_outbox"))?;
        if bytes.len() > 400000 {
            return Err(error("invalid_outbox"));
        }
        let key = hex::encode(Sha256::digest(
            serde_json::to_vec(&json!([endpoint, body])).map_err(|_| error("invalid_outbox"))?,
        ));
        let path = self.root.join(format!("abandoned-publication-{key}.json"));
        if path.exists() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
                .map_err(io)?;
            let meta = file.metadata().map_err(io)?;
            if !meta.is_file()
                || meta.nlink() != 1
                || meta.mode() & 0o077 != 0
                || meta.len() > 400000
            {
                return Err(error("invalid_outbox"));
            }
            let mut old = Vec::new();
            file.take(400001).read_to_end(&mut old).map_err(io)?;
            let previous: Value =
                serde_json::from_slice(&old).map_err(|_| error("invalid_outbox"))?;
            if previous["endpoint"] != endpoint || previous["body"] != *body {
                return Err(error("completion_conflict"));
            }
            return Ok(());
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.root).map_err(io)?;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        file.as_file().sync_all().map_err(io)?;
        file.persist_noclobber(path)
            .map_err(|_| error("pending_completion"))?;
        std::fs::File::open(&self.root)
            .map_err(io)?
            .sync_all()
            .map_err(io)
    }
    pub async fn flush(&self, client: &ControlClient) -> Result<Option<Value>> {
        let path = self.root.join("pending.json");
        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let m = file.metadata().map_err(io)?;
        if !m.is_file() || m.mode() & 0o077 != 0 || m.len() > 400000 {
            return Err(error("invalid_outbox"));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io)?;
        let pending: Pending =
            serde_json::from_slice(&bytes).map_err(|_| error("invalid_outbox"))?;
        if pending.version != 1
            || !["complete", "container-publish", "release-complete"]
                .contains(&pending.endpoint.as_str())
        {
            return Err(error("invalid_outbox"));
        }
        let result = client.post(&pending.endpoint, &pending.body).await?;
        std::fs::remove_file(path).map_err(io)?;
        std::fs::File::open(&self.root)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        Ok(Some(result))
    }
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| error("invalid_control_response"))
}
fn generation(value: &Value) -> Result<i32> {
    value["generation"]
        .as_i64()
        .filter(|g| *g > 0 && *g <= i64::from(i32::MAX))
        .map(|g| g as i32)
        .ok_or_else(|| error("invalid_generation"))
}
fn deadline(value: &Value) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(string(value, "leaseUntil")?)
        .map(|d| d.timestamp())
        .map_err(|_| error("invalid_lease"))
}
pub struct Worker {
    _lock: std::fs::File,
    control: ControlClient,
    image: Arc<VmImage>,
    store: Arc<ArtifactStore>,
    scratch: PathBuf,
    outbox: Outbox,
    publishers: BTreeMap<String, (String, Arc<RegistryPublisher>)>,
    remote_stores: BTreeMap<String, crate::remote::RemoteArtifacts>,
    upload_binding: Option<String>,
    evidence_signer: Option<Arc<ed25519_dalek::SigningKey>>,
}
impl Worker {
    pub fn new(
        control: ControlClient,
        image: VmImage,
        store: ArtifactStore,
        scratch: PathBuf,
        outbox: Outbox,
    ) -> Result<Self> {
        let m = scratch.symlink_metadata().map_err(io)?;
        if !m.is_dir() || m.mode() & 0o077 != 0 {
            return Err(error("private_scratch_required"));
        }
        use std::os::fd::AsRawFd;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(outbox.root.join("worker.lock"))
            .map_err(io)?;
        // SAFETY: the live File owns this descriptor and flock does not retain it.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(error("worker_already_running"));
        }
        Ok(Self {
            _lock: lock,
            control,
            image: Arc::new(image),
            store: Arc::new(store),
            scratch,
            outbox,
            publishers: BTreeMap::new(),
            remote_stores: BTreeMap::new(),
            upload_binding: None,
            evidence_signer: None,
        })
    }
    /// Operator-selected evidence key stays in the trusted worker, outside guest requests.
    pub fn with_evidence_signer(mut self, key: ed25519_dalek::SigningKey) -> Self {
        self.evidence_signer = Some(Arc::new(key));
        self
    }
    pub fn with_publisher(
        mut self,
        project: String,
        repository: String,
        publisher: RegistryPublisher,
    ) -> Result<Self> {
        crate::contracts::id(&project)?;
        self.publishers
            .insert(project, (repository, Arc::new(publisher)));
        Ok(self)
    }
    pub fn with_artifact_store(
        mut self,
        store: crate::remote::RemoteArtifacts,
        upload: bool,
    ) -> Result<Self> {
        let binding = store.binding().to_owned();
        if self.remote_stores.contains_key(&binding) || (upload && self.upload_binding.is_some()) {
            return Err(error("invalid_artifact_bindings"));
        }
        if upload {
            self.upload_binding = Some(binding.clone());
        }
        self.remote_stores.insert(binding, store);
        Ok(self)
    }
    async fn transfer_with_lease<T>(
        &self,
        kind: &str,
        id: &str,
        generation: i32,
        transfer: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::pin!(transfer);
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                result = &mut transfer => {
                    self.control.guard(kind, id, generation).await?;
                    return result;
                },
                _ = interval.tick() => self.control.guard(kind, id, generation).await?,
            }
        }
    }
    pub async fn build_once(&self) -> Result<bool> {
        self.outbox.flush(&self.control).await?;
        let frontend = self
            .image
            .supported_recipes()
            .contains(&crate::frontend::RECIPE);
        if self.upload_binding.is_some() || self.evidence_signer.is_some() || frontend {
            let capabilities = self.control.post("capabilities", &json!({})).await?;
            if !capabilities["artifactFormats"]
                .as_array()
                .is_some_and(|formats| formats.contains(&json!(if frontend { 2 } else { 1 })))
            {
                return Err(error("artifact_format_unavailable"));
            }
            if frontend
                && !capabilities["buildRecipes"]
                    .as_array()
                    .is_some_and(|recipes| recipes.contains(&json!(crate::frontend::RECIPE)))
            {
                return Err(error("recipe_unavailable"));
            }
        }
        let claim = if frontend {
            json!({"recipes": self.image.supported_recipes()})
        } else {
            json!({})
        };
        let job = self.control.post("claim", &claim).await?;
        if job.is_null() {
            return Ok(false);
        }
        let id = string(&job, "buildID")?.to_owned();
        let customer = string(&job, "customerID")?.to_owned();
        crate::contracts::id(&id)?;
        crate::contracts::id(&customer)?;
        crate::contracts::id(string(&job, "projectID")?)?;
        crate::contracts::sha(string(&job, "sourceSHA")?)?;
        let generation = generation(&job)?;
        let request: GuestRequest =
            serde_json::from_value(job["request"].clone()).map_err(|_| error("invalid_job"))?;
        request.validate()?;
        if !self
            .image
            .supported_recipes()
            .contains(&request.recipe.as_str())
        {
            return Err(error("recipe_unavailable"));
        }
        if deadline(&job)? - chrono::Utc::now().timestamp()
            < request.limits.duration_seconds as i64 + 60
        {
            return Err(error("lease_too_short"));
        }
        let source = match self.control.archive(&id, generation).await {
            Ok(source) => Arc::new(source),
            Err(e) if matches!(e.code, "source_limit" | "control_response_limit") => {
                self.control.guard_build(&id, generation).await?;
                self.outbox.put("complete", json!({"buildID":id,"generation":generation,"status":"failed","failureCode":"BUILD_FAILED","logs":"Retained source preparation exceeded its finite limit."}))?;
                self.outbox.flush(&self.control).await?;
                return Ok(true);
            }
            Err(e) => return Err(e),
        };
        self.control.guard_build(&id, generation).await?;
        let control = self.control.clone();
        let build = id.clone();
        let image = self.image.clone();
        let scratch = self.scratch.clone();
        let runtime = tokio::runtime::Handle::current();
        let redactor = request.clone();
        let evidence = crate::evidence::BuildEvidence {
            customer_id: customer.clone(),
            project_id: string(&job, "projectID")?.into(),
            build_id: id.clone(),
            generation,
            source_commit: string(&job, "sourceSHA")?.into(),
            recipe: request.recipe.clone(),
            root_directory: request.root_directory.clone(),
            machine: self.image.identity(),
        };
        let guest_source = source.clone();
        let capture_evidence = self.upload_binding.is_some()
            || self.evidence_signer.is_some()
            || request.recipe == crate::frontend::RECIPE;
        let evidence_signer = self.evidence_signer.clone();
        let built =
            tokio::task::spawn_blocking(move || -> Result<crate::execution::BuildOutput> {
                let mut output = image.execute(&request, &guest_source, &scratch, || {
                    runtime.block_on(control.guard_build(&build, generation))
                })?;
                if capture_evidence {
                    runtime.block_on(control.guard_build(&build, generation))?;
                    output.artifact = crate::evidence::assemble(
                        &output.artifact,
                        &guest_source,
                        &evidence,
                        evidence_signer.as_deref(),
                    )
                    .map_err(|mut error| {
                        error.logs = format!(
                            "Retained source/evidence preparation failed ({}).\n{}",
                            error.code, output.logs
                        );
                        error
                    })?;
                    output.digest =
                        format!("sha256:{}", hex::encode(Sha256::digest(&output.artifact)));
                }
                Ok(output)
            })
            .await
            .map_err(|_| error("build_execution_failed"))?;
        let prepared = built.and_then(|output| {
            self.store
                .put(&customer, &id, generation, &output.artifact)
                .map(|artifact| (output, artifact))
                .map_err(|mut e| {
                    e.logs = format!("Local retained artifact preparation failed ({}).", e.code);
                    e
                })
        });
        let body = match prepared {
            Ok((output, mut artifact)) => {
                self.control.guard_build(&id, generation).await?;
                if let Some(binding) = &self.upload_binding {
                    let remote = &self.remote_stores[binding];
                    artifact = self
                        .transfer_with_lease(
                            "builds",
                            &id,
                            generation,
                            remote.persist(&self.store, &artifact),
                        )
                        .await?;
                }
                json!({"buildID":id,"generation":generation,"status":"succeeded","logs":output.logs,"artifact":{"kind":"oci","digest":artifact.digest(),"storageKey":artifact.storage_key(),"sizeBytes":artifact.size(),"imageDigest":artifact.image_digest(),"remoteStorage":artifact.remote_storage()}})
            }
            Err(e)
                if matches!(
                    e.code,
                    "worker_revoked" | "lease_changed" | "build_lease_lost"
                ) =>
            {
                return Err(e);
            }
            Err(e) => {
                json!({"buildID":id,"generation":generation,"status":"failed","failureCode":"BUILD_FAILED","logs":redactor.redact(&e.logs)})
            }
        };
        self.outbox.put("complete", body)?;
        self.outbox.flush(&self.control).await?;
        Ok(true)
    }
    pub async fn publish_once(&self) -> Result<bool> {
        publish_once_without_builder(
            &self.control,
            &self.store,
            &self.outbox,
            &self.publishers,
            &self.remote_stores,
        )
        .await
    }
    pub async fn turn(&self, prefer_release: bool) -> Result<bool> {
        self.outbox.flush(&self.control).await?;
        if prefer_release {
            if self.publish_once().await? {
                Ok(true)
            } else {
                self.build_once().await
            }
        } else if self.build_once().await? {
            Ok(true)
        } else {
            self.publish_once().await
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerConfig {
    #[serde(rename = "controlURL")]
    control_url: String,
    credential_file: PathBuf,
    image: PathBuf,
    #[serde(rename = "imageSHA256")]
    image_sha256: String,
    #[serde(default)]
    frontend_output: bool,
    #[serde(default)]
    kernel: Option<PathBuf>,
    #[serde(default, rename = "kernelSHA256")]
    kernel_sha256: Option<String>,
    #[serde(default)]
    initrd: Option<PathBuf>,
    #[serde(default, rename = "initrdSHA256")]
    initrd_sha256: Option<String>,
    artifact_root: PathBuf,
    scratch_root: PathBuf,
    state_root: PathBuf,
    country: String,
    isolation: String,
    location_evidence: String,
    isolation_evidence: String,
    storage_evidence: String,
    #[serde(default)]
    registry_projects: BTreeMap<String, RegistryPolicy>,
    #[serde(default = "store_limit")]
    artifact_store_limit: u64,
    #[serde(default)]
    artifact_stores: Vec<ArtifactStorage>,
    #[serde(default)]
    artifact_upload_store: Option<String>,
    #[serde(default)]
    artifact_signing_key_file: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtifactStorage {
    storage: webdock_object_storage::S3Config,
    credential_file: PathBuf,
}
fn store_limit() -> u64 {
    20 * 1024 * 1024 * 1024
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistryPolicy {
    repository: String,
    credential_file: PathBuf,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrySecret {
    username: String,
    password: String,
}
fn private_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    configured_file(path, limit, false)
}
fn secret_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    configured_file(path, limit, true)
}
fn configured_file(path: &Path, limit: u64, secret: bool) -> Result<Vec<u8>> {
    configured_file_as(path, limit, secret, 0)
}
fn configured_file_as(path: &Path, limit: u64, secret: bool, owner: u32) -> Result<Vec<u8>> {
    if !path.is_absolute() {
        return Err(error("untrusted_configuration"));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file()
        || meta.uid() != owner
        || meta.mode() & if secret { 0o077 } else { 0o022 } != 0
        || meta.nlink() != 1
        || meta.len() > limit
    {
        return Err(error("untrusted_configuration"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > limit {
        return Err(error("configuration_limit"));
    }
    Ok(bytes)
}

impl WorkerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&private_file(path, 65536)?)
            .map_err(|_| error("invalid_worker_configuration"))
    }
    pub fn build(self) -> Result<Worker> {
        if self.country.len() != 2
            || !self.country.bytes().all(|c| c.is_ascii_uppercase())
            || self.isolation != "qemu-kvm"
            || self.location_evidence.is_empty()
            || self.isolation_evidence.is_empty()
            || self.storage_evidence.is_empty()
            || (self.country == "ZZ"
                && !self
                    .location_evidence
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|s| s.eq_ignore_ascii_case("unverified")))
        {
            return Err(error("worker_evidence_required"));
        }
        let credential = String::from_utf8(secret_file(&self.credential_file, 256)?)
            .map_err(|_| error("invalid_credential"))?
            .trim()
            .to_owned();
        let control = ControlClient::new(&self.control_url, credential)?;
        let meta = self.image.symlink_metadata().map_err(io)?;
        if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(error("untrusted_image"));
        }
        let mut image = VmImage::new(self.image, self.image_sha256)?;
        if self.frontend_output {
            image = image.with_frontend_output();
        }
        match (
            self.kernel,
            self.kernel_sha256,
            self.initrd,
            self.initrd_sha256,
        ) {
            (Some(kernel), Some(ksha), Some(initrd), Some(isha)) => {
                for file in [&kernel, &initrd] {
                    let m = file.symlink_metadata().map_err(io)?;
                    if !m.is_file() || m.uid() != 0 || m.mode() & 0o022 != 0 {
                        return Err(error("untrusted_image"));
                    }
                }
                image = image.with_boot(kernel, ksha, initrd, isha)?;
            }
            (None, None, None, None) => {}
            _ => return Err(error("invalid_boot_configuration")),
        }
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .map_err(|_| error("kvm_required"))?;
        let store = ArtifactStore::open(self.artifact_root, self.artifact_store_limit)?;
        let outbox = Outbox::open(self.state_root)?;
        let mut worker = Worker::new(control, image, store, self.scratch_root, outbox)?;
        if let Some(path) = self.artifact_signing_key_file {
            let key = String::from_utf8(secret_file(&path, 256)?)
                .map_err(|_| error("invalid_artifact_signing_key"))?;
            let bytes: [u8; 32] = hex::decode(key.trim())
                .map_err(|_| error("invalid_artifact_signing_key"))?
                .try_into()
                .map_err(|_| error("invalid_artifact_signing_key"))?;
            worker = worker.with_evidence_signer(ed25519_dalek::SigningKey::from_bytes(&bytes));
        }
        if self.artifact_stores.len() > 16 {
            return Err(error("invalid_artifact_bindings"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for artifact_store in self.artifact_stores {
            if !ids.insert(artifact_store.storage.id.clone()) {
                return Err(error("invalid_artifact_bindings"));
            }
            let meta = artifact_store
                .credential_file
                .symlink_metadata()
                .map_err(io)?;
            if meta.mode() & 0o077 != 0 {
                return Err(error("untrusted_configuration"));
            }
            let credentials =
                serde_json::from_slice(&secret_file(&artifact_store.credential_file, 8192)?)
                    .map_err(|_| error("invalid_artifact_credentials"))?;
            let upload =
                self.artifact_upload_store.as_deref() == Some(artifact_store.storage.id.as_str());
            let store = webdock_object_storage::Store::s3(artifact_store.storage, credentials)
                .map_err(|_| error("invalid_artifact_configuration"))?;
            worker =
                worker.with_artifact_store(crate::remote::RemoteArtifacts::new(store), upload)?;
        }
        if self.artifact_upload_store.is_some() && worker.upload_binding.is_none() {
            return Err(error("invalid_artifact_bindings"));
        }
        for (project, policy) in self.registry_projects {
            crate::contracts::id(&project)?;
            let prefix = policy
                .repository
                .split('/')
                .next()
                .ok_or_else(|| error("invalid_registry_policy"))?;
            if !policy.repository.ends_with(&format!("/projects/{project}"))
                || policy.repository.contains("..")
                || !policy.repository.contains("/customers/")
            {
                return Err(error("invalid_registry_policy"));
            }
            let auth: RegistrySecret =
                serde_json::from_slice(&secret_file(&policy.credential_file, 16384)?)
                    .map_err(|_| error("invalid_registry_credential"))?;
            if auth.username.is_empty() || auth.password.is_empty() {
                return Err(error("invalid_registry_credential"));
            }
            let publisher = RegistryPublisher::new(
                prefix.into(),
                RegistryCredential::Basic(auth.username, auth.password),
            )?;
            worker = worker.with_publisher(project, policy.repository, publisher)?;
        }
        Ok(worker)
    }
}

#[cfg(all(test, feature = "local-registry-test"))]
mod transfer_tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[tokio::test]
    async fn evidence_worker_requires_server_capability_before_claiming_a_build() {
        use axum::{Json, Router, routing::post};
        let claims = Arc::new(AtomicUsize::new(0));
        let count = claims.clone();
        let router = Router::new()
            .route(
                "/capabilities",
                post(|| async { Json(json!({"artifactFormats":[0]})) }),
            )
            .route(
                "/claim",
                post(move || {
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        Json(Value::Null)
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let local = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        for directory in [&local, &scratch, &state] {
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let image = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(image.path(), b"must not execute").unwrap();
        let hash = hex::encode(Sha256::digest(b"must not execute"));
        let worker = Worker::new(
            ControlClient::loopback_test(&format!("http://{address}"), "a".repeat(43)).unwrap(),
            VmImage::new(image.path().into(), hash).unwrap(),
            ArtifactStore::open(local.path().into(), MAX_OUTPUT).unwrap(),
            scratch.path().into(),
            Outbox::open(state.path().into()).unwrap(),
        )
        .unwrap()
        .with_evidence_signer(ed25519_dalek::SigningKey::from_bytes(&[1; 32]));
        assert_eq!(
            worker.build_once().await.unwrap_err().code,
            "artifact_format_unavailable"
        );
        assert_eq!(claims.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn pinned_guest_recipe_advertisement_and_source_limit_failure_are_durable() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        let claims = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let completions = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let seen = claims.clone();
        let completed = completions.clone();
        let router = Router::new()
            .route("/capabilities", post(|| async { Json(json!({"artifactFormats":[0,1,2],"buildRecipes":["dockerfile","dockerfile-frontend"]})) }))
            .route("/claim", post(move |Json(body): Json<Value>| {
                seen.lock().unwrap().push(body);
                async { Json(json!({"buildID":"100","customerID":"200","projectID":"300","sourceSHA":"a".repeat(40),"generation":1,"leaseUntil":(chrono::Utc::now()+chrono::Duration::minutes(27)).to_rfc3339(),"request":{"recipe":"dockerfile","rootDirectory":".","buildEnvironment":{},"limits":{"cpu":2,"memoryMiB":2048,"diskBytes":24000000000_u64,"durationSeconds":180}}})) }
            }))
            .route("/builds/100/archive", get(|| async { (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":"archive_limit"}))) }))
            .route("/builds/100/lease", get(|| async { Json(json!({"active":true})) }))
            .route("/complete", post(move |Json(body): Json<Value>| {
                completed.lock().unwrap().push(body);
                async { Json(json!({"status":"failed"})) }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        for frontend in [false, true] {
            let local = tempfile::tempdir().unwrap();
            let scratch = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            for dir in [&local, &scratch, &state] {
                std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
            let file = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(file.path(), b"never execute").unwrap();
            let mut image = VmImage::new(
                file.path().into(),
                hex::encode(Sha256::digest(b"never execute")),
            )
            .unwrap();
            if frontend {
                image = image.with_frontend_output();
            }
            let worker = Worker::new(
                ControlClient::loopback_test(&format!("http://{address}"), "a".repeat(43)).unwrap(),
                image,
                ArtifactStore::open(local.path().into(), MAX_OUTPUT).unwrap(),
                scratch.path().into(),
                Outbox::open(state.path().into()).unwrap(),
            )
            .unwrap();
            assert!(worker.build_once().await.unwrap());
            assert!(!worker.outbox.has_pending());
            assert!(scratch.path().read_dir().unwrap().next().is_none());
        }
        assert_eq!(
            *claims.lock().unwrap(),
            vec![
                json!({}),
                json!({"recipes":["dockerfile","dockerfile-frontend"]})
            ]
        );
        let completed = completions.lock().unwrap();
        assert_eq!(completed.len(), 2);
        for body in completed.iter() {
            assert_eq!(body["status"], "failed");
            assert_eq!(body["failureCode"], "BUILD_FAILED");
            assert!(body.get("artifact").is_none());
        }
        server.abort();
    }

    struct Cancelled(Arc<AtomicBool>);
    impl Drop for Cancelled {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn revocation_during_a_transfer_cancels_it_without_a_completion() {
        let revoked = Arc::new(AtomicBool::new(false));
        let state = revoked.clone();
        let allowed = Arc::new(AtomicUsize::new(0));
        let checks = allowed.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum::Router::new().route(
            "/builds/100/lease",
            axum::routing::get(move || {
                let state = state.clone();
                let checks = checks.clone();
                async move {
                    if state.load(Ordering::SeqCst) {
                        (axum::http::StatusCode::FORBIDDEN, axum::Json(json!({})))
                    } else {
                        checks.fetch_add(1, Ordering::SeqCst);
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(json!({"active":true})),
                        )
                    }
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let image = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(image.path(), b"not executed by this transport test").unwrap();
        let hash = hex::encode(Sha256::digest(std::fs::read(image.path()).unwrap()));
        let local = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        for directory in [&local, &scratch, &state] {
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let worker = Worker::new(
            ControlClient::loopback_test(&format!("http://{address}"), "a".repeat(43)).unwrap(),
            VmImage::new(image.path().into(), hash).unwrap(),
            ArtifactStore::open(local.path().into(), MAX_OUTPUT).unwrap(),
            scratch.path().into(),
            Outbox::open(state.path().into()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            worker
                .transfer_with_lease("builds", "100", 1, async { Ok(7) })
                .await
                .unwrap(),
            7
        );
        allowed.store(0, Ordering::SeqCst);
        let (started, received) = tokio::sync::oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let transfer = async {
            let _cancelled = Cancelled(flag);
            started.send(()).unwrap();
            std::future::pending::<Result<()>>().await
        };
        let revoke = tokio::spawn(async move {
            received.await.unwrap();
            while allowed.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            revoked.store(true, Ordering::SeqCst);
        });
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            worker.transfer_with_lease("builds", "100", 1, transfer),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "worker_revoked");
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(!worker.outbox.has_pending());
        revoke.await.unwrap();
        server.abort();
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;
    #[test]
    fn secret_configuration_rejects_public_files_symlinks_and_hardlinks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials");
        std::fs::write(&path, b"private fixture").unwrap();
        let owner = path.metadata().unwrap().uid();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            configured_file_as(&path, 256, false, owner).is_ok(),
            "non-secret readonly operator configuration remains supported"
        );
        assert!(configured_file_as(&path, 256, true, owner).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            configured_file_as(&path, 256, true, owner).unwrap(),
            b"private fixture"
        );
        assert!(configured_file_as(&path, 2, true, owner).is_err());
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(configured_file_as(&link, 256, true, owner).is_err());
        std::fs::hard_link(&path, directory.path().join("hardlink")).unwrap();
        assert!(configured_file_as(&path, 256, true, owner).is_err());
    }
}

#[cfg(test)]
mod discarded_outbox_tests {
    use super::*;
    #[test]
    fn only_exact_abandoned_completion_can_be_removed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let outbox = Outbox::open(root.path().to_path_buf()).unwrap();
        let body = json!({"buildID":"123","generation":1,"status":"failed","failureCode":"CANCELLED","logs":"owned"});
        outbox.put("complete", body.clone()).unwrap();
        assert!(
            outbox
                .discard_exact("complete", &json!({"buildID":"124"}))
                .is_err()
        );
        assert!(outbox.has_pending());
        assert!(outbox.discard_exact("release-complete", &body).is_err());
        assert!(outbox.has_pending());
        outbox.discard_exact("complete", &body).unwrap();
        assert!(!outbox.has_pending());
        outbox.discard_exact("complete", &body).unwrap();
    }
}

#[cfg(all(test, feature = "local-registry-test"))]
mod publication_only_tests {
    use super::*;
    #[tokio::test]
    async fn publication_idle_needs_no_vm_or_build_executor() {
        use axum::{Json, Router, routing::post};
        let app = Router::new().route("/release-claim", post(|| async { Json(Value::Null) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        for path in [root.path(), out.path()] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store = ArtifactStore::open(root.path().to_path_buf(), MAX_OUTPUT).unwrap();
        let outbox = Outbox::open(out.path().to_path_buf()).unwrap();
        let client =
            ControlClient::loopback_test(&format!("http://{address}"), "a".repeat(43)).unwrap();
        assert!(
            !publish_once_without_builder(
                &client,
                &store,
                &outbox,
                &BTreeMap::new(),
                &BTreeMap::new()
            )
            .await
            .unwrap()
        );
        task.abort();
    }
}

/// Registry publication and Hosting rollout observation without any VM/build execution dependency.
pub async fn publish_once_without_builder(
    control: &ControlClient,
    store: &ArtifactStore,
    outbox: &Outbox,
    publishers: &BTreeMap<String, (String, Arc<RegistryPublisher>)>,
    remote_stores: &BTreeMap<String, crate::remote::RemoteArtifacts>,
) -> Result<bool> {
    PublicationContext {
        control,
        store,
        outbox,
        publishers,
        remote_stores,
    }
    .publish_once()
    .await
}
struct PublicationContext<'a> {
    control: &'a ControlClient,
    store: &'a ArtifactStore,
    outbox: &'a Outbox,
    publishers: &'a BTreeMap<String, (String, Arc<RegistryPublisher>)>,
    remote_stores: &'a BTreeMap<String, crate::remote::RemoteArtifacts>,
}
impl PublicationContext<'_> {
    async fn transfer_with_lease<T>(
        &self,
        kind: &str,
        id: &str,
        generation: i32,
        transfer: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::pin!(transfer);
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {result=&mut transfer=>{self.control.guard(kind,id,generation).await?;return result;},_=interval.tick()=>self.control.guard(kind,id,generation).await?}
        }
    }
    async fn flush_publication(&self) -> Result<Option<Value>> {
        match self.outbox.flush(self.control).await {
            Ok(value) => Ok(value),
            Err(failure) if matches!(failure.code, "lease_changed" | "worker_revoked") => {
                let Some((endpoint, body)) = self.outbox.pending_publication()? else {
                    return Err(failure);
                };
                let id = string(&body, "releaseID")?;
                let generation = generation(&body)?;
                let guard = self.control.guard_release(id, generation).await;
                let Err(fence) = guard else {
                    return Err(failure);
                };
                if !matches!(fence.code, "lease_changed" | "worker_revoked") {
                    return Err(fence);
                }
                // This is authoritative loss of promotion authority, not a transport retry.
                self.outbox
                    .record_abandoned_publication(&endpoint, &body, fence.code)?;
                self.outbox.discard_exact(&endpoint, &body)?;
                if fence.code == "worker_revoked" {
                    return Err(fence);
                }
                Ok(Some(json!({"abandoned":true})))
            }
            Err(failure) => Err(failure),
        }
    }
    pub async fn publish_once(&self) -> Result<bool> {
        self.flush_publication().await?;
        let bindings: Vec<_> = self.remote_stores.keys().collect();
        let projects: BTreeMap<_, _> = self
            .publishers
            .iter()
            .map(|(project, (repository, _))| (project, repository))
            .collect();
        let release = self
            .control
            .post(
                "release-claim",
                &json!({"artifactStoreBindings":bindings,"publicationProjects":projects,"artifactFormats":crate::artifact::SUPPORTED_FORMATS}),
            )
            .await?;
        if release.is_null() {
            return Ok(false);
        }
        let id = string(&release, "releaseID")?.to_owned();
        let generation = generation(&release)?;
        let base = json!({"releaseID":id,"generation":generation});
        let publish = async {
            let prepared = self.control.post("release-prepare", &base).await?;
            if prepared["kind"] != "oci" {
                return Err(error("publication_kind_unavailable"));
            }
            let project = string(&release, "projectID")?;
            let customer = string(&release, "customerID")?;
            let (policy, publisher) = self
                .publishers
                .get(project)
                .ok_or_else(|| error("registry_policy_unavailable"))?;
            if policy != string(&prepared, "repository")? {
                return Err(error("registry_scope_mismatch"));
            }
            let record = &release["artifact"];
            let size = record["sizeBytes"]
                .as_u64()
                .filter(|s| *s <= MAX_OUTPUT)
                .ok_or_else(|| error("invalid_artifact"))?;
            let key = string(record, "storageKey")?.to_owned();
            let digest = string(record, "digest")?.to_owned();
            let image_digest = string(record, "imageDigest")?.to_owned();
            let artifact = if record["remoteStorage"].is_null() {
                self.store.load(key, digest, size, image_digest)?
            } else {
                let remote: webdock_object_storage::ObjectReceipt =
                    serde_json::from_value(record["remoteStorage"].clone())
                        .map_err(|_| error("invalid_artifact"))?;
                let backend = self
                    .remote_stores
                    .get(&remote.binding)
                    .ok_or_else(|| error("artifact_backend_mismatch"))?;
                let artifact = crate::artifact::StoredArtifact {
                    key,
                    digest,
                    size,
                    image_digest,
                    remote: Some(remote),
                };
                self.control.guard_release(&id, generation).await?;
                self.transfer_with_lease(
                    "releases",
                    &id,
                    generation,
                    backend.restore(self.store, &artifact),
                )
                .await?
            };
            struct Guard<'a> {
                client: &'a ControlClient,
                id: &'a str,
                generation: i32,
            }
            #[async_trait::async_trait]
            impl PublicationGuard for Guard<'_> {
                async fn check(&self) -> Result<()> {
                    self.client.guard_release(self.id, self.generation).await
                }
            }
            let image = publisher
                .publish(
                    customer,
                    project,
                    &id,
                    self.store,
                    &artifact,
                    &Guard {
                        client: self.control,
                        id: &id,
                        generation,
                    },
                )
                .await?;
            self.outbox.put(
                "container-publish",
                json!({"releaseID":id,"generation":generation,"image":image}),
            )?;
            if self
                .flush_publication()
                .await?
                .is_some_and(|v| v["abandoned"] == true)
            {
                return Ok::<(), crate::Error>(());
            }
            let expires = deadline(&release)?.saturating_sub(30);
            while chrono::Utc::now().timestamp() < expires {
                let observed = self.control.post("observe", &base).await?;
                if observed["status"] != "deploying" {
                    return Ok::<(), crate::Error>(());
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(error("publication_uncertain"))
        }
        .await;
        if let Err(e) = publish {
            if self.outbox.has_pending() {
                return Err(e);
            }
            self.outbox.put(
                "release-complete",
                json!({"releaseID":id,"generation":generation,"status":"needs-reconciliation"}),
            )?;
            self.flush_publication().await?;
            return Err(e);
        }
        Ok(true)
    }
}

#[cfg(all(test, feature = "local-registry-test"))]
mod fenced_publication_tests {
    use super::*;
    #[tokio::test]
    async fn authoritative_cancel_does_not_poison_other_projects_publisher() {
        use axum::{
            Json, Router,
            http::StatusCode,
            routing::{get, post},
        };
        let app = Router::new()
            .route(
                "/container-publish",
                post(|| async {
                    (
                        StatusCode::CONFLICT,
                        Json(json!({"error":"build_publication_cancelled"})),
                    )
                }),
            )
            .route(
                "/releases/123/lease",
                get(|| async {
                    (
                        StatusCode::CONFLICT,
                        Json(json!({"error":"build_publication_cancelled"})),
                    )
                }),
            )
            .route("/release-claim", post(|| async { Json(Value::Null) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        for path in [root.path(), out.path()] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store = ArtifactStore::open(root.path().into(), MAX_OUTPUT).unwrap();
        let outbox = Outbox::open(out.path().into()).unwrap();
        outbox.put("container-publish",json!({"releaseID":"123","generation":1,"image":format!("registry.example/owned@sha256:{}","a".repeat(64))})).unwrap();
        let control =
            ControlClient::loopback_test(&format!("http://{addr}"), "a".repeat(43)).unwrap();
        assert!(
            !publish_once_without_builder(
                &control,
                &store,
                &outbox,
                &BTreeMap::new(),
                &BTreeMap::new()
            )
            .await
            .unwrap()
        );
        assert!(!outbox.has_pending());
        assert!(std::fs::read_dir(out.path()).unwrap().any(|p| {
            p.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("abandoned-publication-")
        }));
        task.abort();
    }
    #[tokio::test]
    async fn transport_uncertainty_keeps_exact_callback_until_accepted() {
        use axum::{Json, Router, http::StatusCode, routing::post};
        use std::sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        };
        fn callback_reply(
            body: Value,
            first: &AtomicBool,
            seen: &Mutex<Vec<Value>>,
        ) -> (StatusCode, Json<Value>) {
            seen.lock().unwrap().push(body);
            if first.swap(false, Ordering::SeqCst) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":"uncertain"})),
                );
            }
            (
                StatusCode::OK,
                Json(json!({"replayed":true,"operationID":"321"})),
            )
        }
        let first = Arc::new(AtomicBool::new(true));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/container-publish",
                post({
                    let first = first.clone();
                    let seen = seen.clone();
                    move |Json(body): Json<Value>| {
                        let first = first.clone();
                        let seen = seen.clone();
                        async move { callback_reply(body, &first, &seen) }
                    }
                }),
            )
            .route("/release-claim", post(|| async { Json(Value::Null) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let root = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        for path in [root.path(), out.path()] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store = ArtifactStore::open(root.path().into(), MAX_OUTPUT).unwrap();
        let outbox = Outbox::open(out.path().into()).unwrap();
        let body = json!({"releaseID":"123","generation":1,"image":format!("registry.example/owned@sha256:{}","a".repeat(64))});
        outbox.put("container-publish", body.clone()).unwrap();
        let control =
            ControlClient::loopback_test(&format!("http://{address}"), "a".repeat(43)).unwrap();
        assert!(
            publish_once_without_builder(
                &control,
                &store,
                &outbox,
                &BTreeMap::new(),
                &BTreeMap::new()
            )
            .await
            .is_err()
        );
        assert!(outbox.has_pending());
        assert!(
            !publish_once_without_builder(
                &control,
                &store,
                &outbox,
                &BTreeMap::new(),
                &BTreeMap::new()
            )
            .await
            .unwrap()
        );
        assert!(!outbox.has_pending());
        assert_eq!(*seen.lock().unwrap(), vec![body.clone(), body]);
        server.abort();
    }
}
