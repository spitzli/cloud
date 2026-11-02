//! Private S3 byte transport. Products retain ownership, authority, retention and billing.
//! Download targets must be private staging: bytes are usable only after verification succeeds.
use bytes::Bytes;
use futures_util::TryStreamExt;
use object_store::{ObjectStore, ObjectStoreExt, aws::AmazonS3Builder, path::Path};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const PART_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TRANSFER: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid_storage_configuration")]
    Configuration,
    #[error("invalid_storage_scope")]
    Scope,
    #[error("storage_transfer_limit")]
    Limit,
    #[error("storage_integrity_mismatch")]
    Integrity,
    #[error("storage_unavailable")]
    Unavailable,
    #[error("storage_upload_uncertain")]
    UploadUncertain,
    #[error("storage_stream_failed")]
    Stream,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct S3Config {
    pub id: String,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub namespace: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Credentials {
    access_key_id: String,
    secret_access_key: String,
}
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials([redacted])")
    }
}
impl Credentials {
    pub fn new(access_key_id: String, secret_access_key: String) -> Result<Self> {
        let value = Self {
            access_key_id,
            secret_access_key,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        for value in [&self.access_key_id, &self.secret_access_key] {
            if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
                return Err(Error::Configuration);
            }
        }
        Ok(())
    }
}

/// A location, never an authorization token. The product must obtain it from its own ledger.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectReceipt {
    pub backend_id: String,
    pub binding: String,
    pub object_key: String,
    pub sha256: String,
    pub size_bytes: u64,
}
impl ObjectReceipt {
    /// Structural validation only. Authority still comes from the product ledger.
    pub fn validate(&self, scope: &str, limit: u64) -> Result<()> {
        path(scope)?;
        path(&self.backend_id)?;
        path(&self.object_key)?;
        let marker = format!("/{scope}/");
        let (namespace, suffix) = self.object_key.rsplit_once(&marker).ok_or(Error::Scope)?;
        let uuid = uuid::Uuid::parse_str(suffix).map_err(|_| Error::Scope)?;
        if namespace.is_empty()
            || self.backend_id.len() > 128
            || self.backend_id.contains('/')
            || uuid.to_string() != suffix
            || [&self.binding, &self.sha256].into_iter().any(|hash| {
                hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(Error::Scope);
        }
        if limit == 0 || limit > MAX_TRANSFER || self.size_bytes > limit {
            return Err(Error::Limit);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Store {
    id: String,
    binding: String,
    namespace: String,
    inner: Arc<dyn ObjectStore>,
    signer: Option<Arc<dyn object_store::signer::Signer>>,
}
impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn path(value: &str) -> Result<()> {
    if value.len() > 512
        || value.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return Err(Error::Scope);
    }
    Ok(())
}
fn binding(parts: &[&str]) -> String {
    hex::encode(Sha256::digest(parts.join("\0").as_bytes()))
}

#[derive(Debug)]
struct Transport(reqwest::Client);
impl object_store::client::HttpConnector for Transport {
    fn connect(
        &self,
        _: &object_store::ClientOptions,
    ) -> object_store::Result<object_store::client::HttpClient> {
        Ok(object_store::client::HttpClient::new(self.0.clone()))
    }
}

impl Store {
    pub fn s3(config: S3Config, credentials: Credentials) -> Result<Self> {
        Self::configured(config, credentials, false, None)
    }
    #[cfg(feature = "local-test")]
    pub fn loopback_test(config: S3Config, credentials: Credentials) -> Result<Self> {
        Self::configured(config, credentials, true, None)
    }
    /// Operator-supplied private CA supplements normal TLS validation; insecure TLS is never enabled.
    pub fn s3_with_ca(config: S3Config, credentials: Credentials, pem: &[u8]) -> Result<Self> {
        Self::configured(config, credentials, false, Some(pem))
    }
    fn configured(
        config: S3Config,
        credentials: Credentials,
        loopback: bool,
        ca: Option<&[u8]>,
    ) -> Result<Self> {
        path(&config.id)?;
        path(&config.namespace)?;
        let endpoint = url::Url::parse(&config.endpoint).map_err(|_| Error::Configuration)?;
        let local = endpoint
            .host_str()
            .and_then(|h| h.parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if config.id.contains('/')
            || config.id.len() > 128
            || endpoint.host_str().is_none()
            || (endpoint.scheme() != "https" && !(loopback && local && endpoint.scheme() == "http"))
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.path() != "/"
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || config.region.is_empty()
            || config.region.len() > 64
            || !config
                .region
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !(3..=63).contains(&config.bucket.len())
            || !config
                .bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || config.bucket.starts_with('-')
            || config.bucket.ends_with('-')
        {
            return Err(Error::Configuration);
        }
        credentials.validate()?;
        let binding = binding(&[
            &config.id,
            endpoint.as_str(),
            &config.region,
            &config.bucket,
            &config.namespace,
        ]);
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let mut client = reqwest::Client::builder();
        if let Some(pem) = ca {
            use rustls::pki_types::pem::PemObject;
            if pem.is_empty() || pem.len() > 65536 {
                return Err(Error::Configuration);
            }
            let mut roots = rustls::RootCertStore::empty();
            let mut count = 0;
            for certificate in rustls::pki_types::CertificateDer::pem_slice_iter(pem) {
                roots
                    .add(certificate.map_err(|_| Error::Configuration)?)
                    .map_err(|_| Error::Configuration)?;
                count += 1;
                if count > 8 {
                    return Err(Error::Configuration);
                }
            }
            if count == 0 {
                return Err(Error::Configuration);
            }
            client = client.add_root_certificate(
                reqwest::Certificate::from_pem(pem).map_err(|_| Error::Configuration)?,
            );
        }
        let client = client
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .https_only(!loopback)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .build()
            .map_err(|_| Error::Configuration)?;
        let store = AmazonS3Builder::new()
            .with_endpoint(config.endpoint)
            .with_region(config.region)
            .with_bucket_name(config.bucket)
            .with_access_key_id(credentials.access_key_id)
            .with_secret_access_key(credentials.secret_access_key)
            .with_allow_http(loopback)
            .with_virtual_hosted_style_request(false)
            .with_http_connector(Transport(client))
            .build()
            .map_err(|_| Error::Configuration)?;
        Ok(Self {
            id: config.id,
            binding,
            namespace: config.namespace,
            inner: Arc::new(store.clone()),
            signer: Some(Arc::new(store)),
        })
    }
    #[cfg(feature = "local-test")]
    pub fn memory(id: &str, namespace: &str) -> Result<Self> {
        path(id)?;
        path(namespace)?;
        Ok(Self {
            id: id.into(),
            binding: binding(&["memory", id, namespace]),
            namespace: namespace.into(),
            inner: Arc::new(object_store::memory::InMemory::new()),
            signer: None,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn binding(&self) -> &str {
        &self.binding
    }
    pub fn validate_receipt(&self, scope: &str, receipt: &ObjectReceipt, limit: u64) -> Result<()> {
        receipt.validate(scope, limit)?;
        let prefix = format!("{}/{scope}/", self.namespace);
        let suffix = receipt
            .object_key
            .strip_prefix(&prefix)
            .ok_or(Error::Scope)?;
        let uuid = uuid::Uuid::parse_str(suffix).map_err(|_| Error::Scope)?;
        if receipt.backend_id != self.id
            || receipt.binding != self.binding
            || uuid.to_string() != suffix
            || receipt.sha256.len() != 64
            || !receipt
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Scope);
        }
        if limit == 0 || limit > MAX_TRANSFER || receipt.size_bytes > limit {
            return Err(Error::Limit);
        }
        Ok(())
    }
    /// Every attempt uses a fresh object name. Retries cannot overwrite retained bytes.
    /// Successful completion includes full readback; an ETag is never treated as a SHA-256.
    pub async fn upload(
        &self,
        scope: &str,
        mut source: impl AsyncRead + Unpin,
        limit: u64,
    ) -> Result<ObjectReceipt> {
        path(scope)?;
        if limit == 0 || limit > MAX_TRANSFER {
            return Err(Error::Limit);
        }
        let key = format!("{}/{scope}/{}", self.namespace, uuid::Uuid::new_v4());
        path(&key)?;
        let location = Path::from(key.as_str());
        let mut upload = self
            .inner
            .put_multipart(&location)
            .await
            .map_err(|_| Error::Unavailable)?;
        let sent = async {
            let mut hash = Sha256::new();
            let mut size = 0u64;
            loop {
                let mut part =
                    vec![0u8; (limit.saturating_sub(size) + 1).min(PART_BYTES as u64) as usize];
                let mut filled = 0;
                while filled < part.len() {
                    let n = source
                        .read(&mut part[filled..])
                        .await
                        .map_err(|_| Error::Stream)?;
                    if n == 0 {
                        break;
                    }
                    filled += n;
                }
                part.truncate(filled);
                size += filled as u64;
                if size > limit {
                    return Err(Error::Limit);
                }
                if filled == 0 {
                    break;
                }
                hash.update(&part);
                upload
                    .put_part(Bytes::from(part).into())
                    .await
                    .map_err(|_| Error::Unavailable)?;
            }
            Ok((size, hex::encode(hash.finalize())))
        }
        .await;
        let (size_bytes, sha256) = match sent {
            Ok(value) => value,
            Err(error) => {
                let _ = tokio::time::timeout(Duration::from_secs(10), upload.abort()).await;
                return Err(error);
            }
        };
        let completed = upload.complete().await.is_ok();
        let receipt = ObjectReceipt {
            backend_id: self.id.clone(),
            binding: self.binding.clone(),
            object_key: key,
            sha256,
            size_bytes,
        };
        // A lost CompleteMultipartUpload response is reconciled by reading this exact unique key.
        match self
            .download(scope, &receipt, &mut tokio::io::sink(), limit)
            .await
        {
            Ok(()) => Ok(receipt),
            Err(error) if completed => Err(error),
            Err(_) => {
                let _ = tokio::time::timeout(Duration::from_secs(10), upload.abort()).await;
                Err(Error::UploadUncertain)
            }
        }
    }
    pub async fn download(
        &self,
        scope: &str,
        receipt: &ObjectReceipt,
        target: &mut (impl AsyncWrite + Unpin),
        limit: u64,
    ) -> Result<()> {
        self.validate_receipt(scope, receipt, limit)?;
        let result = self
            .inner
            .get(&Path::from(receipt.object_key.as_str()))
            .await
            .map_err(|_| Error::Unavailable)?;
        if result.meta.size != receipt.size_bytes {
            return Err(Error::Integrity);
        }
        let mut stream = result.into_stream();
        let mut hash = Sha256::new();
        let mut size = 0u64;
        while let Some(chunk) = stream.try_next().await.map_err(|_| Error::Unavailable)? {
            size = size.checked_add(chunk.len() as u64).ok_or(Error::Limit)?;
            if size > receipt.size_bytes || size > limit {
                return Err(Error::Integrity);
            }
            hash.update(&chunk);
            target.write_all(&chunk).await.map_err(|_| Error::Stream)?;
        }
        if size != receipt.size_bytes || hex::encode(hash.finalize()) != receipt.sha256 {
            return Err(Error::Integrity);
        }
        target.flush().await.map_err(|_| Error::Stream)?;
        Ok(())
    }
}

mod staging;
pub use staging::StagingMethod;
