//! Explicit private controller identity; no token is serialized into execution inputs.
use crate::{Result, error};
use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const NAMESPACE: &str = "webdock-build-controller";
const SERVICE_ACCOUNT: &str = "webdock-build-controller";
const REVOCATION: &str = "webdock-controller-revocation";
const REVIEW: &str = "/apis/authentication.k8s.io/v1/selfsubjectreviews";
const REQUEST: &str =
    "/api/v1/namespaces/webdock-build-controller/serviceaccounts/webdock-build-controller/token";
const AUTHORITY_FLOOR: u64 = 35 * 60;
const MAX_TOKEN: usize = 16384;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KubernetesIdentityConfig {
    #[serde(rename = "serviceAccountUID")]
    service_account_uid: String,
    #[serde(rename = "revocationSecretUID")]
    revocation_secret_uid: String,
    audience: String,
}
impl KubernetesIdentityConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        for uid in [&self.service_account_uid, &self.revocation_secret_uid] {
            if uid.is_empty()
                || uid.len() > 128
                || uid
                    .bytes()
                    .any(|c| !c.is_ascii_alphanumeric() && !b"_-".contains(&c))
            {
                return Err(error("invalid_kubernetes_identity_scope"));
            }
        }
        let audience = url::Url::parse(&self.audience)
            .map_err(|_| error("invalid_kubernetes_identity_scope"))?;
        if self.audience.len() > 512
            || audience.scheme() != "https"
            || audience.host_str().is_none()
            || audience.host_str().is_some_and(|host| host.contains('*'))
            || !audience.username().is_empty()
            || audience.password().is_some()
            || audience.query().is_some()
            || audience.fragment().is_some()
            || self.audience.chars().any(char::is_control)
        {
            return Err(error("invalid_kubernetes_identity_scope"));
        }
        Ok(())
    }
    fn subject(&self) -> String {
        format!("system:serviceaccount:{NAMESPACE}:{SERVICE_ACCOUNT}")
    }
    fn token_request(&self) -> Value {
        json!({"apiVersion":"authentication.k8s.io/v1","kind":"TokenRequest","spec":{"audiences":[self.audience],"expirationSeconds":3600,"boundObjectRef":{"kind":"Secret","apiVersion":"v1","name":REVOCATION,"uid":self.revocation_secret_uid}}})
    }
}

fn private_token(path: &Path) -> Result<String> {
    let bad = || error("kubernetes_identity_file_untrusted");
    if !path.is_absolute()
        || path.components().any(|part| {
            !matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(bad());
    }
    let parent = path.parent().ok_or_else(bad)?;
    let metadata = parent.symlink_metadata().map_err(|_| bad())?;
    // SAFETY: geteuid has no arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.mode() & 0o7777 != 0o700
        || parent.canonicalize().map_err(|_| bad())? != parent
    {
        return Err(bad());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| bad())?;
    let metadata = file.metadata().map_err(|_| bad())?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > MAX_TOKEN as u64
    {
        return Err(bad());
    }
    let mut bytes = Vec::new();
    file.take(MAX_TOKEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| bad())?;
    if bytes.len() > MAX_TOKEN
        || bytes.is_empty()
        || bytes
            .iter()
            .any(|b| !b.is_ascii_alphanumeric() && !b"._-".contains(b))
    {
        return Err(bad());
    }
    let token = String::from_utf8(bytes).map_err(|_| bad())?;
    if token.split('.').count() != 3 || token.split('.').any(str::is_empty) {
        return Err(bad());
    }
    Ok(token)
}

struct TokenFile {
    path: PathBuf,
    directory: std::fs::File,
    directory_id: (u64, u64),
}
impl TokenFile {
    fn open(path: PathBuf) -> Result<Self> {
        private_token(&path)?;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(
                path.parent()
                    .ok_or_else(|| error("kubernetes_identity_file_untrusted"))?,
            )
            .map_err(|_| error("kubernetes_identity_file_untrusted"))?;
        // SAFETY: directory owns this live descriptor; flock retains no pointers.
        if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(error("kubernetes_identity_already_running"));
        }
        let metadata = directory
            .metadata()
            .map_err(|_| error("kubernetes_identity_file_untrusted"))?;
        let guard = Self {
            path,
            directory,
            directory_id: (metadata.dev(), metadata.ino()),
        };
        guard.read()?;
        Ok(guard)
    }
    fn read(&self) -> Result<String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| error("kubernetes_identity_file_untrusted"))?;
        let metadata = parent
            .symlink_metadata()
            .map_err(|_| error("kubernetes_identity_file_untrusted"))?;
        if (metadata.dev(), metadata.ino()) != self.directory_id {
            return Err(error("kubernetes_identity_directory_changed"));
        }
        private_token(&self.path)
    }
    fn replace(&self, token: &str, expected: &[u8; 32]) -> Result<()> {
        let unchanged = || -> Result<()> {
            if <[u8; 32]>::from(Sha256::digest(self.read()?.as_bytes())) != *expected {
                return Err(error("kubernetes_identity_file_changed"));
            }
            Ok(())
        };
        unchanged()?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| error("kubernetes_identity_file_untrusted"))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| error("kubernetes_identity_persist_failed"))?;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| error("kubernetes_identity_persist_failed"))?;
        temporary
            .write_all(token.as_bytes())
            .map_err(|_| error("kubernetes_identity_persist_failed"))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| error("kubernetes_identity_persist_failed"))?;
        unchanged()?;
        temporary
            .persist(&self.path)
            .map_err(|_| error("kubernetes_identity_persist_failed"))?;
        self.directory
            .sync_all()
            .map_err(|_| error("kubernetes_identity_persist_failed"))
    }
}

#[async_trait]
trait IdentityApi: Send + Sync {
    async fn post(&self, token: &str, path: &str, body: Value) -> Result<Value>;
}
struct PinnedApi {
    client: reqwest::Client,
    endpoint: String,
}
impl PinnedApi {
    fn new(config: &kube::Config) -> Result<Self> {
        if !matches!(
            config.cluster_url.to_string().as_str(),
            "https://10.83.0.1:6443" | "https://10.83.0.1:6443/"
        ) || config.proxy_url.is_some()
        {
            return Err(error("invalid_kubernetes_identity_endpoint"));
        }
        let mut roots = rustls::RootCertStore::empty();
        for der in config
            .root_cert
            .as_ref()
            .filter(|roots| !roots.is_empty())
            .ok_or_else(|| error("invalid_kubernetes_identity_ca"))?
        {
            roots
                .add(rustls::pki_types::CertificateDer::from(der.clone()))
                .map_err(|_| error("invalid_kubernetes_identity_ca"))?;
        }
        // This constructor precedes kube-client initialization. Pin Ring here;
        // rustls-no-provider must not depend on another client starting first.
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| error("invalid_kubernetes_identity_ca"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let client = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| error("kubernetes_identity_unavailable"))?;
        Ok(Self {
            client,
            endpoint: "https://10.83.0.1:6443".into(),
        })
    }
}
#[async_trait]
impl IdentityApi for PinnedApi {
    async fn post(&self, token: &str, path: &str, body: Value) -> Result<Value> {
        let response = self
            .client
            .post(format!("{}{path}", self.endpoint))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|_| error("kubernetes_identity_unavailable"))?;
        if matches!(response.status().as_u16(), 401 | 403 | 404) {
            return Err(error("kubernetes_identity_revoked"));
        }
        if !response.status().is_success() {
            return Err(error("kubernetes_identity_unavailable"));
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| error("kubernetes_identity_unavailable"))?;
            if body.len().saturating_add(chunk.len()) > 65536 {
                return Err(error("invalid_kubernetes_identity_response"));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| error("invalid_kubernetes_identity_response"))
    }
}

struct VerifiedToken {
    token: String,
    digest: [u8; 32],
    expires_at: i64,
    deadline: Instant,
}
impl VerifiedToken {
    fn remaining(&self, now: i64, monotonic: Instant) -> u64 {
        let wall = self
            .expires_at
            .checked_sub(now)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(0);
        wall.min(self.deadline.saturating_duration_since(monotonic).as_secs())
    }
}

pub(crate) struct OwnedKubernetesIdentity {
    config: KubernetesIdentityConfig,
    file: TokenFile,
    api: Arc<dyn IdentityApi>,
    current: Mutex<VerifiedToken>,
}
impl OwnedKubernetesIdentity {
    pub(crate) async fn prepare(
        config: &mut kube::Config,
        path: PathBuf,
        identity: KubernetesIdentityConfig,
    ) -> Result<Arc<Self>> {
        let api = Arc::new(PinnedApi::new(config)?);
        let ready = Self::prepare_api(path, identity, api).await?;
        ready.configure_client(config)?;
        Ok(ready)
    }
    fn configure_client(&self, config: &mut kube::Config) -> Result<()> {
        self.audit()?;
        config.auth_info.token = None;
        config.auth_info.token_file = Some(
            self.file
                .path
                .to_str()
                .ok_or_else(|| error("kubernetes_identity_file_untrusted"))?
                .into(),
        );
        Ok(())
    }
    async fn prepare_api(
        path: PathBuf,
        config: KubernetesIdentityConfig,
        api: Arc<dyn IdentityApi>,
    ) -> Result<Arc<Self>> {
        config.validate()?;
        let file = TokenFile::open(path)?;
        // Bootstrap authority is used only to obtain a fresh candidate. No
        // executor/client is exposed until renewability and full margin pass.
        let bootstrap = authenticate(&*api, &config, file.read()?, None, 0).await?;
        let current = request_renewed(&*api, &config, &bootstrap.token).await?;
        file.replace(&current.token, &bootstrap.digest)?;
        let identity = Arc::new(Self {
            config,
            file,
            api,
            current: Mutex::new(current),
        });
        identity.audit()?;
        Ok(identity)
    }
    pub(crate) fn audit(&self) -> Result<()> {
        let current = self
            .current
            .lock()
            .map_err(|_| error("kubernetes_identity_state_unavailable"))?;
        self.audit_current(&current, chrono::Utc::now().timestamp(), Instant::now())
    }
    fn audit_current(&self, current: &VerifiedToken, now: i64, monotonic: Instant) -> Result<()> {
        let token = self.file.read()?;
        if <[u8; 32]>::from(Sha256::digest(token.as_bytes())) != current.digest {
            return Err(error("kubernetes_identity_file_changed"));
        }
        if current.remaining(now, monotonic) <= AUTHORITY_FLOOR {
            return Err(error("kubernetes_identity_expiring"));
        }
        Ok(())
    }
    async fn renew_once(&self) -> Result<()> {
        let (bearer, digest) = {
            let current = self
                .current
                .lock()
                .map_err(|_| error("kubernetes_identity_state_unavailable"))?;
            self.audit_current(&current, chrono::Utc::now().timestamp(), Instant::now())?;
            (current.token.clone(), current.digest)
        };
        let candidate = request_renewed(&*self.api, &self.config, &bearer).await?;
        // File and in-memory state change under one lock. Readers cannot mistake
        // our own atomic rename for an unknown replacement. No sidecar expiry
        // file is needed: startup reauthenticates the sole persisted token.
        let mut current = self
            .current
            .lock()
            .map_err(|_| error("kubernetes_identity_state_unavailable"))?;
        if current.digest != digest {
            return Err(error("kubernetes_identity_state_changed"));
        }
        self.audit_current(&current, chrono::Utc::now().timestamp(), Instant::now())?;
        self.file.replace(&candidate.token, &digest)?;
        *current = candidate;
        Ok(())
    }
    pub(crate) async fn run(self: Arc<Self>) -> Result<()> {
        let mut audit = tokio::time::interval(Duration::from_secs(1));
        let mut next = tokio::time::Instant::now();
        loop {
            let wakeup = next;
            let renewal = async {
                tokio::time::sleep_until(wakeup).await;
                self.renew_once().await
            };
            tokio::pin!(renewal);
            loop {
                tokio::select! {
                    biased;
                    _ = audit.tick() => self.audit()?,
                    outcome = &mut renewal => {
                        next = tokio::time::Instant::now() + match outcome {
                            Ok(()) => Duration::from_secs(300),
                            Err(failure) if failure.code == "kubernetes_identity_unavailable" => Duration::from_secs(15),
                            Err(failure) => return Err(failure),
                        };
                        break;
                    }
                }
            }
        }
    }
}

async fn request_renewed(
    api: &dyn IdentityApi,
    config: &KubernetesIdentityConfig,
    bearer: &str,
) -> Result<VerifiedToken> {
    let response = api.post(bearer, REQUEST, config.token_request()).await?;
    let token = response["status"]["token"]
        .as_str()
        .ok_or_else(|| error("invalid_kubernetes_identity_response"))?
        .to_owned();
    let expires = response["status"]["expirationTimestamp"]
        .as_str()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .ok_or_else(|| error("invalid_kubernetes_identity_response"))?
        .timestamp();
    authenticate(api, config, token, Some(expires), AUTHORITY_FLOOR).await
}

async fn authenticate(
    api: &dyn IdentityApi,
    config: &KubernetesIdentityConfig,
    token: String,
    expected_expiry: Option<i64>,
    minimum_remaining: u64,
) -> Result<VerifiedToken> {
    config.validate()?;
    if token.is_empty()
        || token.len() > MAX_TOKEN
        || token
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && !b"._-".contains(&b))
    {
        return Err(error("invalid_kubernetes_identity_token"));
    }
    let review = api
        .post(
            &token,
            REVIEW,
            json!({"apiVersion":"authentication.k8s.io/v1","kind":"SelfSubjectReview","spec":{}}),
        )
        .await?;
    let info = &review["status"]["userInfo"];
    let groups = info["groups"]
        .as_array()
        .ok_or_else(|| error("kubernetes_identity_scope_conflict"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| error("kubernetes_identity_scope_conflict"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let expected_groups = BTreeSet::from([
        "system:authenticated".to_string(),
        "system:serviceaccounts".to_string(),
        format!("system:serviceaccounts:{NAMESPACE}"),
    ]);
    if info["username"].as_str() != Some(config.subject().as_str())
        || info["uid"].as_str() != Some(config.service_account_uid.as_str())
        || groups != expected_groups.iter().map(String::as_str).collect()
    {
        return Err(error("kubernetes_identity_scope_conflict"));
    }
    // Decode only after the pinned API authenticates this exact bearer. JWT
    // text alone never authorizes a subject, scope, expiry or replacement.
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err(error("invalid_kubernetes_identity_token"));
    }
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| error("invalid_kubernetes_identity_token"))?;
    let claims: Value =
        serde_json::from_slice(&payload).map_err(|_| error("invalid_kubernetes_identity_token"))?;
    let now = chrono::Utc::now().timestamp();
    let issued = claims["iat"]
        .as_i64()
        .ok_or_else(|| error("invalid_kubernetes_identity_lifetime"))?;
    let not_before = claims["nbf"]
        .as_i64()
        .ok_or_else(|| error("invalid_kubernetes_identity_lifetime"))?;
    let expires = claims["exp"]
        .as_i64()
        .ok_or_else(|| error("invalid_kubernetes_identity_lifetime"))?;
    if issued > now + 30
        || not_before > now + 30
        || !expires
            .checked_sub(issued)
            .is_some_and(|seconds| (1..=3600).contains(&seconds))
        || expires > now + 3630
        || expires
            .checked_sub(now)
            .is_none_or(|seconds| seconds <= minimum_remaining as i64)
        || expected_expiry.is_some_and(|expected| expected.abs_diff(expires) > 1)
    {
        return Err(error("invalid_kubernetes_identity_lifetime"));
    }
    let scope = &claims["kubernetes.io"];
    if claims["sub"] != config.subject()
        || claims["aud"] != json!([config.audience])
        || scope["namespace"] != NAMESPACE
        || scope["serviceaccount"]
            != json!({"name":SERVICE_ACCOUNT,"uid":config.service_account_uid})
        || scope["secret"] != json!({"name":REVOCATION,"uid":config.revocation_secret_uid})
        || scope.get("pod").is_some()
        || scope.get("node").is_some()
    {
        return Err(error("kubernetes_identity_scope_conflict"));
    }
    let seconds =
        u64::try_from(expires - now).map_err(|_| error("invalid_kubernetes_identity_lifetime"))?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(seconds))
        .ok_or_else(|| error("invalid_kubernetes_identity_lifetime"))?;
    Ok(VerifiedToken {
        digest: Sha256::digest(token.as_bytes()).into(),
        token,
        expires_at: expires,
        deadline,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    use std::sync::atomic::{AtomicUsize, Ordering};

    fn identity_config() -> KubernetesIdentityConfig {
        serde_json::from_value(json!({"serviceAccountUID":"owned-service-account-uid","revocationSecretUID":"owned-revocation-uid","audience":"https://kubernetes.default.svc.cluster.local"})).unwrap()
    }
    fn token(config: &KubernetesIdentityConfig, serial: usize, overrides: Value) -> String {
        let now = chrono::Utc::now().timestamp();
        let mut claims = json!({"sub":config.subject(),"aud":[config.audience],"iat":now,"nbf":now,"exp":now+3600,"jti":format!("owned-test-{serial}"),"kubernetes.io":{"namespace":NAMESPACE,"serviceaccount":{"name":SERVICE_ACCOUNT,"uid":config.service_account_uid},"secret":{"name":REVOCATION,"uid":config.revocation_secret_uid}}});
        for (key, value) in overrides.as_object().unwrap() {
            claims[key] = value.clone();
        }
        format!(
            "e30.{}.ownedtestsignature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap())
        )
    }
    struct ControlledApi {
        config: KubernetesIdentityConfig,
        accepted: Mutex<BTreeSet<String>>,
        replacement_claims: Mutex<Value>,
        wrong_review: AtomicUsize,
        unavailable: AtomicUsize,
        deny_renewal: AtomicUsize,
        renewed: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl IdentityApi for ControlledApi {
        async fn post(&self, bearer: &str, path: &str, body: Value) -> Result<Value> {
            if self.unavailable.load(Ordering::SeqCst) != 0 {
                return Err(error("kubernetes_identity_unavailable"));
            }
            if !self.accepted.lock().unwrap().contains(bearer) {
                return Err(error("kubernetes_identity_revoked"));
            }
            if path == REVIEW {
                assert_eq!(body["kind"], "SelfSubjectReview");
                return Ok(
                    json!({"status":{"userInfo":{"username":if self.wrong_review.load(Ordering::SeqCst)==2 {"system:serviceaccount:kube-system:foreign".to_string()}else{self.config.subject()},"uid":if self.wrong_review.load(Ordering::SeqCst)==1 {"replacement-uid"}else{self.config.service_account_uid.as_str()},"groups":if self.wrong_review.load(Ordering::SeqCst)==3 {json!(["system:masters"])}else{json!(["system:serviceaccounts",format!("system:serviceaccounts:{NAMESPACE}"),"system:authenticated"])}}}}),
                );
            }
            assert_eq!(path, REQUEST);
            if self.deny_renewal.load(Ordering::SeqCst) != 0 {
                return Err(error("kubernetes_identity_revoked"));
            }
            assert_eq!(body, self.config.token_request());
            let serial = self.renewed.fetch_add(1, Ordering::SeqCst) + 1;
            let replacement = token(
                &self.config,
                serial,
                self.replacement_claims.lock().unwrap().clone(),
            );
            let parts = replacement.split('.').collect::<Vec<_>>();
            let claims: Value = serde_json::from_slice(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(parts[1])
                    .unwrap(),
            )
            .unwrap();
            let expiry = chrono::DateTime::from_timestamp(claims["exp"].as_i64().unwrap(), 0)
                .unwrap()
                .to_rfc3339();
            self.accepted.lock().unwrap().insert(replacement.clone());
            Ok(json!({"status":{"token":replacement,"expirationTimestamp":expiry}}))
        }
    }
    fn private_fixture() -> (tempfile::TempDir, PathBuf, Arc<ControlledApi>) {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = root.path().join("identity");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("token");
        let config = identity_config();
        let seed = token(&config, 0, json!({}));
        std::fs::write(&path, &seed).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let api = Arc::new(ControlledApi {
            config,
            accepted: Mutex::new(BTreeSet::from([seed])),
            replacement_claims: Mutex::new(json!({})),
            wrong_review: AtomicUsize::new(0),
            unavailable: AtomicUsize::new(0),
            deny_renewal: AtomicUsize::new(0),
            renewed: Arc::new(AtomicUsize::new(0)),
        });
        (root, path, api)
    }
    pub(crate) async fn verified_fixture() -> (
        tempfile::TempDir,
        Arc<OwnedKubernetesIdentity>,
        Arc<AtomicUsize>,
    ) {
        let (root, path, api) = private_fixture();
        let count = api.renewed.clone();
        let identity = OwnedKubernetesIdentity::prepare_api(path, identity_config(), api)
            .await
            .unwrap();
        (root, identity, count)
    }
    #[tokio::test]
    async fn startup_proves_renewability_before_exposing_a_managed_client() {
        let (_root, path, api) = private_fixture();
        let initial = std::fs::read(&path).unwrap();
        api.deny_renewal.store(1, Ordering::SeqCst);
        assert_eq!(
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api)
                .await
                .err()
                .unwrap()
                .code,
            "kubernetes_identity_revoked"
        );
        assert_eq!(std::fs::read(path).unwrap(), initial);
    }
    #[tokio::test]
    async fn startup_renews_valid_short_bootstrap_but_never_uses_expired_authority() {
        let (_root, path, api) = private_fixture();
        let now = chrono::Utc::now().timestamp();
        let bootstrap = token(&identity_config(), 77, json!({"exp":now+300}));
        api.accepted.lock().unwrap().insert(bootstrap.clone());
        std::fs::write(&path, &bootstrap).unwrap();
        let identity =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        identity.audit().unwrap();
        assert_ne!(std::fs::read_to_string(&path).unwrap(), bootstrap);
        drop(identity);
        let expired = token(&identity_config(), 78, json!({"exp":now-1}));
        api.accepted.lock().unwrap().insert(expired.clone());
        std::fs::write(&path, &expired).unwrap();
        assert_eq!(
            OwnedKubernetesIdentity::prepare_api(path, identity_config(), api)
                .await
                .err()
                .unwrap()
                .code,
            "invalid_kubernetes_identity_lifetime"
        );
    }

    #[tokio::test]
    async fn authenticated_api_identity_is_required_before_claims_are_used() {
        let (_root, path, api) = private_fixture();
        let identity =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        identity.audit().unwrap();
        drop(identity);
        for fault in 1..=3 {
            api.wrong_review.store(fault, Ordering::SeqCst);
            assert_eq!(
                OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                    .await
                    .err()
                    .unwrap()
                    .code,
                "kubernetes_identity_scope_conflict"
            );
        }
        api.wrong_review.store(0, Ordering::SeqCst);
        let forged = token(&identity_config(), 99, json!({}));
        std::fs::write(&path, forged).unwrap();
        assert_eq!(
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api)
                .await
                .err()
                .unwrap()
                .code,
            "kubernetes_identity_revoked"
        );
    }

    #[tokio::test]
    async fn renewal_authenticates_candidate_before_atomic_private_replacement() {
        let (_root, path, api) = private_fixture();
        let identity =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        let original = std::fs::read(&path).unwrap();
        identity.renew_once().await.unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
        identity.audit().unwrap();
        assert_eq!(api.renewed.load(Ordering::SeqCst), 2);
        // Restart derives expiry/scope from the single persisted token, not a
        // separately replaceable metadata file or stale kubeconfig token.
        drop(identity);
        OwnedKubernetesIdentity::prepare_api(path, identity_config(), api)
            .await
            .unwrap()
            .audit()
            .unwrap();
    }
    #[tokio::test]
    async fn wrong_candidate_uid_audience_expiry_or_binding_never_replaces_authority() {
        let now = chrono::Utc::now().timestamp();
        for wrong in [
            json!({"aud":["foreign"]}),
            json!({"exp":now+7200}),
            json!({"exp":now+30}),
            json!({"kubernetes.io":{"namespace":"foreign"}}),
            json!({"kubernetes.io":{"namespace":NAMESPACE,"serviceaccount":{"name":SERVICE_ACCOUNT,"uid":"replacement-sa"},"secret":{"name":REVOCATION,"uid":"owned-revocation-uid"}}}),
            json!({"kubernetes.io":{"namespace":NAMESPACE,"serviceaccount":{"name":SERVICE_ACCOUNT,"uid":"owned-service-account-uid"},"secret":{"name":REVOCATION,"uid":"replacement-secret"}}}),
        ] {
            let (_root, path, api) = private_fixture();
            let identity =
                OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                    .await
                    .unwrap();
            let original = std::fs::read(&path).unwrap();
            *api.replacement_claims.lock().unwrap() = wrong;
            let failure = identity.renew_once().await.unwrap_err();
            assert!(failure.logs.is_empty());
            assert!(!failure.code.contains("ownedtestsignature"));
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
        let (_root, path, api) = private_fixture();
        let identity =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        let original = std::fs::read(&path).unwrap();
        api.wrong_review.store(1, Ordering::SeqCst);
        assert_eq!(
            identity.renew_once().await.unwrap_err().code,
            "kubernetes_identity_scope_conflict"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[tokio::test]
    async fn transient_failure_preserves_authority_but_monotonic_and_wall_expiry_fence_it() {
        let (_root, path, api) = private_fixture();
        let identity =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        let original = std::fs::read(path).unwrap();
        api.unavailable.store(1, Ordering::SeqCst);
        assert_eq!(
            identity.renew_once().await.unwrap_err().code,
            "kubernetes_identity_unavailable"
        );
        identity.audit().unwrap();
        assert_eq!(identity.file.read().unwrap().as_bytes(), original);
        let current = identity.current.lock().unwrap();
        assert_eq!(
            identity
                .audit_current(
                    &current,
                    current.expires_at - AUTHORITY_FLOOR as i64,
                    Instant::now()
                )
                .unwrap_err()
                .code,
            "kubernetes_identity_expiring"
        );
        assert_eq!(
            identity
                .audit_current(
                    &current,
                    chrono::Utc::now().timestamp() - 7200,
                    current.deadline + Duration::from_secs(1)
                )
                .unwrap_err()
                .code,
            "kubernetes_identity_expiring"
        );
    }
    #[tokio::test]
    async fn unknown_file_replacement_missing_file_and_directory_reuse_fence_cached_auth() {
        let (root, path, api) = private_fixture();
        let identity =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, token(&identity_config(), 77, json!({}))).unwrap();
        assert_eq!(
            identity.audit().unwrap_err().code,
            "kubernetes_identity_file_changed"
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            identity.audit().unwrap_err().code,
            "kubernetes_identity_file_untrusted"
        );
        std::fs::write(&path, &original).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let parent = path.parent().unwrap();
        std::fs::rename(parent, root.path().join("previous-identity")).unwrap();
        std::fs::create_dir(parent).unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&path, original).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            identity.audit().unwrap_err().code,
            "kubernetes_identity_directory_changed"
        );
    }
    #[tokio::test]
    async fn revoked_api_authority_never_renews_or_replaces_a_token() {
        let (_root, path, api) = private_fixture();
        let identity =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        let original = std::fs::read(path).unwrap();
        api.accepted.lock().unwrap().clear();
        let failure = identity.renew_once().await.unwrap_err();
        assert_eq!(failure.code, "kubernetes_identity_revoked");
        assert!(failure.logs.is_empty());
        assert_eq!(identity.file.read().unwrap().as_bytes(), original);
        assert_eq!(api.renewed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn maintained_client_sends_rotated_file_bearer_without_restart_or_job_deletion() {
        use http_body_util::Full;
        use kube::client::{Body, ConfigExt};
        let (_root, path, api) = private_fixture();
        let identity =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        let mut config = kube::Config::new("https://10.83.0.1:6443".parse().unwrap());
        config.auth_info.token = Some("inline-value-must-not-shadow-file".into());
        identity.configure_client(&mut config).unwrap();
        assert!(config.auth_info.token.is_none());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let registry = api.clone();
        let service=tower::ServiceBuilder::new().option_layer(config.auth_layer().unwrap()).service(tower::service_fn(move |request:http::Request<Body>| {
            let recorded=recorded.clone();let registry=registry.clone();
            async move {
                assert_eq!(request.method(),http::Method::GET,"identity rotation must not delete a Job");
                let bearer=request.headers()["authorization"].to_str().unwrap().strip_prefix("Bearer ").unwrap().to_owned();
                assert!(registry.accepted.lock().unwrap().contains(&bearer));
                recorded.lock().unwrap().push(bearer);
                Ok::<_,Box<dyn std::error::Error+Send+Sync>>(http::Response::new(Full::new(serde_json::to_vec(&json!({"apiVersion":"batch/v1","kind":"Job","metadata":{"name":"owned","uid":"unchanged-job"},"status":{"active":1}})).unwrap().into())))
            }
        }));
        let client = kube::Client::new(service, "webdock-build-qualification");
        let jobs = kube::Api::<k8s_openapi::api::batch::v1::Job>::namespaced(
            client,
            "webdock-build-qualification",
        );
        assert_eq!(
            jobs.get("owned").await.unwrap().metadata.uid.as_deref(),
            Some("unchanged-job")
        );
        assert_eq!(requests.lock().unwrap()[0], original);
        identity.renew_once().await.unwrap();
        let replacement = std::fs::read_to_string(path).unwrap();
        assert_ne!(replacement, original);
        // kube-client uses real wall-clock cache time, not tokio's test clock.
        // Waiting once proves the maintained refresh path on the SAME client.
        tokio::time::sleep(Duration::from_secs(51)).await;
        assert_eq!(
            jobs.get("owned").await.unwrap().metadata.uid.as_deref(),
            Some("unchanged-job")
        );
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert_eq!(requests.lock().unwrap()[1], replacement);
    }

    #[tokio::test]
    async fn second_owner_cannot_compete_for_the_same_identity_directory() {
        let (_root, path, api) = private_fixture();
        let first =
            OwnedKubernetesIdentity::prepare_api(path.clone(), identity_config(), api.clone())
                .await
                .unwrap();
        assert_eq!(
            OwnedKubernetesIdentity::prepare_api(path, identity_config(), api)
                .await
                .err()
                .unwrap()
                .code,
            "kubernetes_identity_already_running"
        );
        first.audit().unwrap();
    }

    #[test]
    fn private_identity_config_cannot_override_target_or_use_wildcard_audience() {
        let mut value = json!({"serviceAccountUID":"owned-sa-uid","revocationSecretUID":"owned-secret-uid","audience":"https://kubernetes.default.svc.cluster.local"});
        value["namespace"] = json!("kube-system");
        assert!(serde_json::from_value::<KubernetesIdentityConfig>(value).is_err());
        let mut config = identity_config();
        config.audience = "https://*.example".into();
        assert!(config.validate().is_err());
        config = identity_config();
        config.service_account_uid = "foreign:account".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn identity_transport_requires_the_fixed_private_endpoint_and_pinned_roots() {
        use rustls::pki_types::pem::PemObject;
        let mut config = kube::Config::new("https://10.83.0.1:6443".parse().unwrap());
        assert!(PinnedApi::new(&config).is_err());
        let certificates = rustls::pki_types::CertificateDer::pem_slice_iter(include_bytes!(
            "../tests/fixtures/kubernetes-worker-ca.crt"
        ))
        .map(|value| value.unwrap().to_vec())
        .collect();
        config.root_cert = Some(certificates);
        assert!(PinnedApi::new(&config).is_ok());
        config.cluster_url = "https://167.86.75.144:6443".parse().unwrap();
        assert_eq!(
            PinnedApi::new(&config).err().unwrap().code,
            "invalid_kubernetes_identity_endpoint"
        );
    }

    #[test]
    fn missing_public_linked_and_nonprivate_parent_tokens_are_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("token");
        assert!(private_token(&path).is_err());
        std::fs::write(&path, b"header.payload.signature").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(private_token(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = root.path().join("linked");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(private_token(&link).is_err());
        std::fs::remove_file(&link).unwrap();
        std::fs::hard_link(&path, &link).unwrap();
        assert!(private_token(&path).is_err());
        std::fs::remove_file(&link).unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_token(&path).is_err());
    }

    #[test]
    fn explicit_owned0600_token_in_private0700_directory_is_readable() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("token");
        std::fs::write(&path, b"header.payload.signature").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(private_token(&path).unwrap(), "header.payload.signature");
    }
}
