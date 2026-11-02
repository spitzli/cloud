//! Short-lived staging capabilities. Use a separate store namespace from retained releases.
use super::*;
use object_store::{PutMode, PutOptions};
#[derive(Clone, Copy)]
pub enum StagingMethod {
    Get,
    Put,
}
impl Store {
    fn staging_key(&self, scope: &str, name: &str) -> Result<Path> {
        path(scope)?;
        path(name)?;
        if name.contains('/') {
            return Err(Error::Scope);
        }
        Ok(Path::from(format!("{}/{scope}/{name}", self.namespace)))
    }
    /// Source download only: never grant a build Pod provider write authority. Result ingress
    /// must use the revocable, streamed byte-bounded native build upload gateway.
    pub async fn presign_build_source(&self, scope: &str, expires: Duration) -> Result<url::Url> {
        let fields: Vec<_> = scope.split('/').collect();
        if fields.len() != 5
            || fields[0] != "build-attempts"
            || fields[1..4].iter().any(|s| {
                s.is_empty()
                    || s.len() > 63
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            })
            || fields[4].parse::<u64>().ok().filter(|n| *n > 0).is_none()
        {
            return Err(Error::Scope);
        }
        self.presign_staging(scope, "source.tar", StagingMethod::Get, expires)
            .await
    }
    /// The returned URL is a scoped credential: never Debug-print it or emit it to build logs.
    /// Only GET/PUT for this exact object are exposed; deletion/retention authority stays outside Jobs.
    /// A provider PUT signature is NOT a byte limit or revocable grant; untrusted build output
    /// must never use this legacy provider capability (even when a client claims a length).
    pub async fn presign_staging(
        &self,
        scope: &str,
        name: &str,
        method: StagingMethod,
        expires: Duration,
    ) -> Result<url::Url> {
        if expires.is_zero() || expires > Duration::from_secs(1800) {
            return Err(Error::Configuration);
        }
        let key = self.staging_key(scope, name)?;
        let method = match method {
            StagingMethod::Get => reqwest::Method::GET,
            StagingMethod::Put => reqwest::Method::PUT,
        };
        self.signer
            .as_ref()
            .ok_or(Error::Configuration)?
            .signed_url(method, &key, expires)
            .await
            .map_err(|_| Error::Unavailable)
    }
    /// Create once. A replay may only confirm identical source bytes, never overwrite another source.
    pub async fn write_staging(
        &self,
        scope: &str,
        name: &str,
        bytes: &[u8],
        limit: u64,
    ) -> Result<()> {
        if limit == 0 || limit > MAX_TRANSFER || bytes.len() as u64 > limit {
            return Err(Error::Limit);
        }
        let key = self.staging_key(scope, name)?;
        let options = PutOptions {
            mode: PutMode::Create,
            ..Default::default()
        };
        match self
            .inner
            .put_opts(&key, Bytes::copy_from_slice(bytes).into(), options)
            .await
        {
            Ok(_) => Ok(()),
            Err(object_store::Error::AlreadyExists { .. }) => {
                if self.read_staging(scope, name, limit).await? == bytes {
                    Ok(())
                } else {
                    Err(Error::Integrity)
                }
            }
            Err(_) => Err(Error::UploadUncertain),
        }
    }
    /// Delete only an exact transient key, never a bucket/prefix or retained release reference.
    pub async fn delete_staging(&self, scope: &str, name: &str) -> Result<()> {
        let key = self.staging_key(scope, name)?;
        match self.inner.delete(&key).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(_) => Err(Error::Unavailable),
        }
    }
    /// Caller independently validates the complete result identity and OCI graph before retaining it.
    pub async fn read_staging(&self, scope: &str, name: &str, limit: u64) -> Result<Vec<u8>> {
        if limit == 0 || limit > MAX_TRANSFER {
            return Err(Error::Limit);
        }
        let key = self.staging_key(scope, name)?;
        let result = self.inner.get(&key).await.map_err(|_| Error::Unavailable)?;
        if result.meta.size > limit {
            return Err(Error::Limit);
        }
        let mut stream = result.into_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.try_next().await.map_err(|_| Error::Stream)? {
            if bytes.len() as u64 + chunk.len() as u64 > limit {
                return Err(Error::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
