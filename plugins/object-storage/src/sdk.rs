//! Explicit provider administration. No aws-config, environment credentials or metadata discovery.
use crate::minio::Minio;
use crate::{model::*, provider::Provider};
use aws_sdk_iam::Client as Iam;
use aws_sdk_s3::{
    Client as S3,
    config::{BehaviorVersion, Credentials, Region, retry::RetryConfig, timeout::TimeoutConfig},
    types::*,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendConfig {
    pub id: String,
    pub endpoint: String,
    pub region: String,
    pub account: String,
    pub access_key: String,
    pub secret_key: String,
    pub kind: String,
    pub minio: Option<crate::minio::MinioConfig>,
}
pub struct Control {
    backend: String,
    endpoint: String,
    region: String,
    s3: S3,
    identity: Identity,
}
enum Identity {
    Wasabi(Iam),
    Minio(Minio),
}
fn unknown<T>(_: T) -> ProviderError {
    ProviderError::Unknown
}
fn object_checksum(head: &aws_sdk_s3::operation::head_object::HeadObjectOutput) -> Option<String> {
    use base64::Engine;
    if head.checksum_type() != Some(&ChecksumType::FullObject) {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(head.checksum_sha256()?)
        .ok()?;
    (bytes.len() == 32).then(|| hex::encode(bytes))
}
fn object_metadata(
    key: &str,
    head: &aws_sdk_s3::operation::head_object::HeadObjectOutput,
) -> ProviderResult<crate::data::ObjectMetadata> {
    let filename = key
        .rsplit('/')
        .next()
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 160
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._- ".contains(&b))
        })
        .unwrap_or("download.bin");
    let content_type = head
        .content_type()
        .filter(|kind| {
            matches!(
                *kind,
                "application/octet-stream"
                    | "application/pdf"
                    | "application/zip"
                    | "application/gzip"
                    | "text/plain"
                    | "text/csv"
            )
        })
        .unwrap_or("application/octet-stream");
    // Multipart/composite or unlabelled checksums are not advertised as full object SHA-256.
    let expected_sha256 = if head.checksum_type() == Some(&ChecksumType::FullObject) {
        object_checksum(head)
    } else {
        None
    };
    let metadata = crate::data::ObjectMetadata {
        key: key.into(),
        exact_bytes: u64::try_from(head.content_length().ok_or(ProviderError::Unavailable)?)
            .map_err(unknown)?,
        version_id: head.version_id().unwrap_or("null").into(),
        if_match: head.e_tag().ok_or(ProviderError::Unavailable)?.into(),
        filename: filename.into(),
        content_type: content_type.into(),
        expected_sha256,
    };
    metadata.validate(now() as i64).map_err(unknown)?;
    Ok(metadata)
}
#[cfg(feature = "local-native-fixture")]
fn native_fixture_endpoint(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            host.strip_suffix(".native-object-storage.test")
                .is_some_and(|run| {
                    run.len() == 12
                        && run
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
        })
}
fn marker(b: &Binding) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(b).expect("binding serialization"),
    ))
}
fn iam_path(b: &Binding) -> String {
    format!("/webdock/{}/", b.id.simple())
}
pub fn application_policy(b: &Binding) -> Value {
    json!({"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":["s3:ListBucket","s3:ListBucketVersions","s3:GetBucketLocation","s3:ListBucketMultipartUploads"],"Resource":format!("arn:aws:s3:::{}",b.bucket)},
        {"Effect":"Allow","Action":["s3:GetObject","s3:GetObjectVersion","s3:PutObject","s3:DeleteObject","s3:DeleteObjectVersion","s3:AbortMultipartUpload","s3:ListMultipartUploadParts"],"Resource":format!("arn:aws:s3:::{}/*",b.bucket)}]})
}
impl Control {
    pub async fn new(c: BackendConfig) -> Result<Self, &'static str> {
        // AWS and tonic enable two Rustls providers; select one before native mTLS is configured.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let u = url::Url::parse(&c.endpoint).map_err(|_| "Invalid storage endpoint")?;
        if !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
            || u.path() != "/"
            || c.region.is_empty()
            || c.id.is_empty()
            || c.account.is_empty()
            || c.access_key.is_empty()
            || c.secret_key.is_empty()
        {
            return Err("Explicit storage binding and credentials required");
        }
        match c.kind.as_str() {
            "wasabi"
                if u.scheme() == "https"
                    && u.host_str().is_some_and(|h| {
                        h == "s3.wasabisys.com"
                            || (h.starts_with("s3.") && h.ends_with(".wasabisys.com"))
                    })
                    && u.port().is_none()
                    && c.minio.is_none() => {}
            "minio-local"
                if matches!(u.host_str(), Some("127.0.0.1" | "[::1]"))
                    && matches!(u.scheme(), "http" | "https")
                    && c.minio.is_some() => {}
            #[cfg(feature = "local-native-fixture")]
            "minio-native-fixture" if native_fixture_endpoint(&u) && c.minio.is_some() => {}
            _ => return Err("Unsupported storage endpoint or backend"),
        }
        let credentials = Credentials::new(
            c.access_key.clone(),
            c.secret_key.clone(),
            None,
            None,
            "webdock-private-config",
        );
        let timeout = TimeoutConfig::builder()
            .operation_timeout(Duration::from_secs(10))
            .operation_attempt_timeout(Duration::from_secs(10))
            .build();
        let s3 = S3::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new(c.region.clone()))
                .endpoint_url(&c.endpoint)
                .credentials_provider(credentials.clone())
                .force_path_style(true)
                .retry_config(RetryConfig::disabled())
                .timeout_config(timeout.clone())
                .build(),
        );
        let identity = if let Some(local) = c.minio {
            #[cfg(feature = "local-native-fixture")]
            if c.kind == "minio-native-fixture" {
                crate::minio::prepare_native_fixture(
                    &local,
                    &c.endpoint,
                    &c.access_key,
                    &c.secret_key,
                )?;
            }
            Identity::Minio(Minio::new(local, &c.endpoint, &c.access_key, &c.secret_key).await?)
        } else {
            Identity::Wasabi(Iam::from_conf(
                aws_sdk_iam::Config::builder()
                    .behavior_version(BehaviorVersion::latest())
                    .region(Region::new("us-east-1"))
                    .endpoint_url("https://iam.wasabisys.com")
                    .credentials_provider(credentials)
                    .retry_config(RetryConfig::disabled())
                    .timeout_config(timeout)
                    .build(),
            ))
        };
        let binding = json!([c.id, c.endpoint, c.region, c.account, c.kind]);
        let backend = format!(
            "{}:{}",
            c.id,
            hex::encode(Sha256::digest(binding.to_string()))
        );
        Ok(Self {
            backend,
            endpoint: c.endpoint,
            region: c.region,
            s3,
            identity,
        })
    }
    async fn identity_exists(&self, b: &Binding) -> ProviderResult<bool> {
        match &self.identity {
            Identity::Minio(m) => m.exists(b).await,
            Identity::Wasabi(i) => match i.get_user().user_name(&b.user).send().await {
                Ok(v) => {
                    if v.user().is_none_or(|u| u.path() != iam_path(b)) {
                        return Err(ProviderError::Ownership);
                    }
                    Ok(true)
                }
                Err(e)
                    if e.as_service_error()
                        .is_some_and(|e| e.meta().code() == Some("NoSuchEntity")) =>
                {
                    Ok(false)
                }
                Err(_) => Err(ProviderError::Unknown),
            },
        }
    }
    async fn bucket_exists(&self, b: &Binding) -> ProviderResult<bool> {
        match self.s3.head_bucket().bucket(&b.bucket).send().await {
            Ok(_) => Ok(true),
            Err(e) if e.raw_response().is_some_and(|r| r.status().as_u16() == 404) => Ok(false),
            Err(_) => Err(ProviderError::Unknown),
        }
    }
    async fn ensure_owned(&self, b: &Binding) -> ProviderResult<()> {
        let o = self.observe(b).await?;
        if !o.exists || !o.owned {
            return Err(ProviderError::Ownership);
        }
        Ok(())
    }
    async fn batch(&self, b: &Binding, mut batch: UploadBatch) -> ProviderResult<InventoryPage> {
        if batch.pending.is_empty() {
            if batch.after.is_some() {
                return Err(ProviderError::Unknown);
            }
            return Ok(InventoryPage::default());
        }
        let item = batch.pending.remove(0);
        self.parts(b, &item.key, &item.upload, None, batch).await
    }
    async fn parts(
        &self,
        b: &Binding,
        key: &str,
        upload: &str,
        part: Option<String>,
        batch: UploadBatch,
    ) -> ProviderResult<InventoryPage> {
        let v = self
            .s3
            .list_parts()
            .bucket(&b.bucket)
            .key(key)
            .upload_id(upload)
            .set_part_number_marker(part.clone())
            .max_parts(100)
            .send()
            .await
            .map_err(|error| {
                if error
                    .as_service_error()
                    .is_some_and(|e| e.meta().code() == Some("NoSuchUpload"))
                {
                    ProviderError::ScanInvalidated
                } else {
                    ProviderError::Unknown
                }
            })?;
        let mut page = InventoryPage {
            multipart_uploads: u64::from(part.is_none()),
            ..Default::default()
        };
        for p in v.parts() {
            page.multipart_bytes = page
                .multipart_bytes
                .checked_add(
                    u64::try_from(p.size().ok_or(ProviderError::Unknown)?).map_err(unknown)?,
                )
                .ok_or(ProviderError::Unknown)?;
        }
        page.next = if v.is_truncated() == Some(true) {
            let next = v
                .next_part_number_marker()
                .filter(|s| Some(*s) != part.as_deref())
                .ok_or(ProviderError::Unknown)?;
            Some(Cursor::Parts {
                key: key.into(),
                upload: upload.into(),
                part: Some(next.into()),
                batch,
            })
        } else {
            batch_next(batch)
        };

        Ok(page)
    }
}
#[async_trait::async_trait]
impl Provider for Control {
    async fn head_object(
        &self,
        b: &Binding,
        key: &str,
    ) -> ProviderResult<crate::data::ObjectMetadata> {
        if b.backend != self.backend || !crate::data::valid_key(key) {
            return Err(ProviderError::Ownership);
        }
        let head = self
            .s3
            .head_object()
            .bucket(&b.bucket)
            .key(key)
            .checksum_mode(ChecksumMode::Enabled)
            .send()
            .await
            .map_err(unknown)?;
        object_metadata(key, &head)
    }
    fn backend(&self) -> &str {
        &self.backend
    }
    fn endpoint(&self) -> &str {
        &self.endpoint
    }
    fn validate_lifecycle(&self, policy: &Lifecycle) -> ProviderResult<()> {
        if matches!(self.identity, Identity::Minio(_)) && policy.abort_multipart_days.is_some() {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
    async fn presign(
        &self,
        b: &Binding,
        direction: webdock_plugin_protocol::storage::StorageDirection,
        request: &crate::data::DataRequest,
    ) -> ProviderResult<crate::data::PresignedObject> {
        use base64::Engine;
        use webdock_plugin_protocol::storage::StorageDirection;
        request.validate(direction, now() as i64).map_err(unknown)?;
        if b.backend != self.backend || b.id != request.resource_id {
            return Err(ProviderError::Ownership);
        }
        let started = now();
        let seconds = u64::try_from(request.expires_at)
            .map_err(unknown)?
            .checked_sub(started)
            .ok_or(ProviderError::Unavailable)?;
        let config = aws_sdk_s3::presigning::PresigningConfig::builder()
            .start_time(std::time::UNIX_EPOCH + Duration::from_secs(started))
            .expires_in(Duration::from_secs(seconds))
            .build()
            .map_err(unknown)?;
        let signed = match direction {
            StorageDirection::Upload => {
                let checksum = base64::engine::general_purpose::STANDARD.encode(
                    hex::decode(
                        request
                            .expected_sha256
                            .as_deref()
                            .ok_or(ProviderError::Unavailable)?,
                    )
                    .map_err(unknown)?,
                );
                self.s3
                    .put_object()
                    .bucket(&b.bucket)
                    .key(&request.key)
                    .content_length(i64::try_from(request.exact_bytes).map_err(unknown)?)
                    .content_type(&request.content_type)
                    .checksum_sha256(&checksum)
                    .if_none_match("*")
                    .presigned(config)
                    .await
                    .map_err(unknown)?
            }
            StorageDirection::Download => {
                let version = request
                    .version_id
                    .as_deref()
                    .ok_or(ProviderError::Unavailable)?;
                let tag = request
                    .if_match
                    .as_deref()
                    .ok_or(ProviderError::Unavailable)?;
                let head = self
                    .s3
                    .head_object()
                    .bucket(&b.bucket)
                    .key(&request.key)
                    .version_id(version)
                    .if_match(tag)
                    .checksum_mode(ChecksumMode::Enabled)
                    .send()
                    .await
                    .map_err(unknown)?;
                if head.content_length()
                    != Some(i64::try_from(request.exact_bytes).map_err(unknown)?)
                    || head.e_tag() != Some(tag)
                    || head.version_id().unwrap_or("null") != version
                    // A known full-object checksum must not become a hashless Core plan.
                    || object_checksum(&head).as_ref().is_some_and(|actual| {
                        request.expected_sha256.as_ref() != Some(actual)
                    })
                {
                    return Err(ProviderError::Unavailable);
                }
                self.s3
                    .get_object()
                    .bucket(&b.bucket)
                    .key(&request.key)
                    .version_id(version)
                    .if_match(tag)
                    .presigned(config)
                    .await
                    .map_err(unknown)?
            }
        };
        if signed.method() != direction.method() {
            return Err(ProviderError::Unavailable);
        }
        // Host/content-length are supplied by Core's pinned transport; retain every other signed header.
        let headers = signed
            .headers()
            .filter(|(k, _)| !["host", "content-length"].contains(k))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        Ok(crate::data::PresignedObject {
            url: signed.uri().into(),
            headers,
        })
    }
    async fn provision(&self, b: &Binding) -> ProviderResult<()> {
        if b.backend != self.backend {
            return Err(ProviderError::Ownership);
        }
        if self.bucket_exists(b).await? || self.identity_exists(b).await? {
            return Err(ProviderError::Collision);
        }
        let mut create = self.s3.create_bucket().bucket(&b.bucket);
        if self.region != "us-east-1" {
            create = create.create_bucket_configuration(
                CreateBucketConfiguration::builder()
                    .location_constraint(BucketLocationConstraint::from(self.region.as_str()))
                    .build(),
            );
        }
        create.send().await.map_err(unknown)?;
        self.s3
            .put_bucket_tagging()
            .bucket(&b.bucket)
            .tagging(
                Tagging::builder()
                    .tag_set(
                        Tag::builder()
                            .key("webdock-binding")
                            .value(marker(b))
                            .build()
                            .map_err(unknown)?,
                    )
                    .build()
                    .map_err(unknown)?,
            )
            .send()
            .await
            .map_err(unknown)?;
        match &self.identity {
            Identity::Minio(m) => m.provision(b, &application_policy(b)).await?,
            Identity::Wasabi(i) => {
                i.create_user()
                    .user_name(&b.user)
                    .path(iam_path(b))
                    .send()
                    .await
                    .map_err(unknown)?;
                i.put_user_policy()
                    .user_name(&b.user)
                    .policy_name("webdock-bucket")
                    .policy_document(application_policy(b).to_string())
                    .send()
                    .await
                    .map_err(unknown)?;
            }
        }
        self.ensure_owned(b).await
    }
    async fn observe(&self, b: &Binding) -> ProviderResult<Observation> {
        if b.backend != self.backend {
            return Err(ProviderError::Ownership);
        }
        if !self.bucket_exists(b).await? {
            return Ok(Observation {
                exists: false,
                owned: false,
                versioning: false,
                lifecycle: None,
                observed_at: now(),
            });
        }
        let tags = self
            .s3
            .get_bucket_tagging()
            .bucket(&b.bucket)
            .send()
            .await
            .map_err(unknown)?;
        let mut owned = tags
            .tag_set()
            .iter()
            .any(|t| t.key() == "webdock-binding" && t.value() == marker(b));
        if !owned {
            return Ok(Observation {
                exists: true,
                owned: false,
                versioning: false,
                lifecycle: None,
                observed_at: now(),
            });
        }
        owned = self.identity_exists(b).await?;
        let acl = self
            .s3
            .get_bucket_acl()
            .bucket(&b.bucket)
            .send()
            .await
            .map_err(unknown)?;
        // Reject public groups and grants to any other canonical account.
        let owner = acl.owner().and_then(|o| o.id());
        // Local MinIO returns an owner-only canonical placeholder without a grantee ID.
        let local = matches!(self.identity, Identity::Minio(_));
        if owner.is_none()
            || acl.grants().iter().any(|g| {
                g.grantee().is_none_or(|v| {
                    v.uri().is_some()
                        || v.r#type() != &Type::CanonicalUser
                        || match v.id() {
                            Some(id) => Some(id) != owner,
                            None => !local,
                        }
                })
            })
        {
            owned = false;
        }
        match self.s3.get_bucket_policy().bucket(&b.bucket).send().await {
            Ok(_) => owned = false,
            Err(e)
                if e.as_service_error()
                    .is_some_and(|e| e.meta().code() == Some("NoSuchBucketPolicy")) => {}
            Err(_) => return Err(ProviderError::Unknown),
        }
        let versioning = self
            .s3
            .get_bucket_versioning()
            .bucket(&b.bucket)
            .send()
            .await
            .map_err(unknown)?
            .status()
            == Some(&BucketVersioningStatus::Enabled);
        let lifecycle = match self
            .s3
            .get_bucket_lifecycle_configuration()
            .bucket(&b.bucket)
            .send()
            .await
        {
            Ok(v) => {
                let rules = v.rules();
                if rules.len() != 1
                    || rules[0].id() != Some("webdock-explicit")
                    || rules[0].status() != &ExpirationStatus::Enabled
                {
                    return Err(ProviderError::Ownership);
                }
                let r = &rules[0];
                Some(Lifecycle {
                    expire_current_days: r.expiration().and_then(|e| e.days()),
                    expire_noncurrent_days: r
                        .noncurrent_version_expiration()
                        .and_then(|e| e.noncurrent_days()),
                    abort_multipart_days: r
                        .abort_incomplete_multipart_upload()
                        .and_then(|e| e.days_after_initiation()),
                })
            }
            Err(e)
                if e.as_service_error()
                    .is_some_and(|e| e.meta().code() == Some("NoSuchLifecycleConfiguration")) =>
            {
                None
            }
            Err(_) => return Err(ProviderError::Unknown),
        };
        Ok(Observation {
            exists: true,
            owned,
            versioning,
            lifecycle,
            observed_at: now(),
        })
    }
    async fn keys(&self, b: &Binding) -> ProviderResult<Vec<String>> {
        if !self.identity_exists(b).await? {
            return Err(ProviderError::Ownership);
        }
        match &self.identity {
            Identity::Minio(m) => m.keys(b).await,
            Identity::Wasabi(i) => {
                let v = i
                    .list_access_keys()
                    .user_name(&b.user)
                    .max_items(3)
                    .send()
                    .await
                    .map_err(unknown)?;
                if v.is_truncated() {
                    return Err(ProviderError::Unknown);
                }
                v.access_key_metadata()
                    .iter()
                    .map(|k| {
                        k.access_key_id()
                            .map(str::to_owned)
                            .ok_or(ProviderError::Unknown)
                    })
                    .collect()
            }
        }
    }
    async fn create_key(&self, b: &Binding) -> ProviderResult<SecretKey> {
        if self.keys(b).await?.len() >= 2 {
            return Err(ProviderError::Unavailable);
        }
        match &self.identity {
            Identity::Minio(m) => m.create_key(b).await,
            Identity::Wasabi(i) => {
                let v = i
                    .create_access_key()
                    .user_name(&b.user)
                    .send()
                    .await
                    .map_err(unknown)?;
                let k = v.access_key().ok_or(ProviderError::Unknown)?;
                Ok(SecretKey {
                    access_key: k.access_key_id().into(),
                    secret_key: k.secret_access_key().into(),
                })
            }
        }
    }
    async fn revoke_key(&self, b: &Binding, key: &str) -> ProviderResult<()> {
        if !self.keys(b).await?.iter().any(|v| v == key) {
            return Ok(());
        }
        match &self.identity {
            Identity::Minio(m) => m.revoke_key(key).await,
            Identity::Wasabi(i) => {
                i.delete_access_key()
                    .user_name(&b.user)
                    .access_key_id(key)
                    .send()
                    .await
                    .map_err(unknown)?;
                Ok(())
            }
        }
    }
    async fn set_versioning(&self, b: &Binding, enabled: bool) -> ProviderResult<()> {
        self.s3
            .put_bucket_versioning()
            .bucket(&b.bucket)
            .versioning_configuration(
                VersioningConfiguration::builder()
                    .status(if enabled {
                        BucketVersioningStatus::Enabled
                    } else {
                        BucketVersioningStatus::Suspended
                    })
                    .build(),
            )
            .send()
            .await
            .map_err(unknown)?;
        Ok(())
    }
    async fn set_lifecycle(&self, b: &Binding, p: Option<&Lifecycle>) -> ProviderResult<()> {
        let Some(p) = p else {
            self.s3
                .delete_bucket_lifecycle()
                .bucket(&b.bucket)
                .send()
                .await
                .map_err(unknown)?;
            return Ok(());
        };
        self.validate_lifecycle(p)?;
        let mut rule = LifecycleRule::builder()
            .id("webdock-explicit")
            .status(ExpirationStatus::Enabled)
            .filter(LifecycleRuleFilter::builder().prefix("").build());
        if let Some(days) = p.expire_current_days {
            rule = rule.expiration(LifecycleExpiration::builder().days(days).build());
        }
        if let Some(days) = p.expire_noncurrent_days {
            rule = rule.noncurrent_version_expiration(
                NoncurrentVersionExpiration::builder()
                    .noncurrent_days(days)
                    .build(),
            );
        }
        if let Some(days) = p.abort_multipart_days {
            rule = rule.abort_incomplete_multipart_upload(
                AbortIncompleteMultipartUpload::builder()
                    .days_after_initiation(days)
                    .build(),
            );
        }
        self.s3
            .put_bucket_lifecycle_configuration()
            .bucket(&b.bucket)
            .lifecycle_configuration(
                BucketLifecycleConfiguration::builder()
                    .rules(rule.build().map_err(unknown)?)
                    .build()
                    .map_err(unknown)?,
            )
            .send()
            .await
            .map_err(unknown)?;
        Ok(())
    }
    async fn inventory(
        &self,
        b: &Binding,
        cursor: Option<&Cursor>,
    ) -> ProviderResult<InventoryPage> {
        match cursor {
            None | Some(Cursor::Versions { .. }) => {
                let (key, version) = match cursor {
                    Some(Cursor::Versions { key, version }) => (key.clone(), version.clone()),
                    _ => (None, None),
                };
                let v = self
                    .s3
                    .list_object_versions()
                    .bucket(&b.bucket)
                    .set_key_marker(key.clone())
                    .set_version_id_marker(version.clone())
                    .max_keys(100)
                    .send()
                    .await
                    .map_err(|error| {
                        if cursor.is_some()
                            && error.as_service_error().is_some_and(|e| {
                                matches!(e.meta().code(), Some("InvalidArgument" | "NoSuchVersion"))
                            })
                        {
                            ProviderError::ScanInvalidated
                        } else {
                            ProviderError::Unknown
                        }
                    })?;
                let mut page = InventoryPage::default();
                for o in v.versions() {
                    let size =
                        u64::try_from(o.size().ok_or(ProviderError::Unknown)?).map_err(unknown)?;
                    if o.is_latest() == Some(true) {
                        page.current_objects += 1;
                        page.current_bytes = page
                            .current_bytes
                            .checked_add(size)
                            .ok_or(ProviderError::Unknown)?;
                    } else {
                        page.noncurrent_versions += 1;
                        page.noncurrent_bytes = page
                            .noncurrent_bytes
                            .checked_add(size)
                            .ok_or(ProviderError::Unknown)?;
                    }
                }
                page.delete_markers = u64::try_from(v.delete_markers().len()).map_err(unknown)?;
                page.next = Some(if v.is_truncated() == Some(true) {
                    let next = v.next_key_marker().ok_or(ProviderError::Unknown)?;
                    if Some(next) == key.as_deref()
                        && v.next_version_id_marker() == version.as_deref()
                    {
                        return Err(ProviderError::Unknown);
                    }
                    Cursor::Versions {
                        key: Some(next.into()),
                        version: v.next_version_id_marker().map(str::to_owned),
                    }
                } else {
                    Cursor::Uploads {
                        key: None,
                        upload: None,
                    }
                });
                Ok(page)
            }
            Some(Cursor::Uploads { key, upload }) => {
                let v = self
                    .s3
                    .list_multipart_uploads()
                    .bucket(&b.bucket)
                    .set_key_marker(key.clone())
                    .set_upload_id_marker(upload.clone())
                    .max_uploads(100)
                    .send()
                    .await
                    .map_err(unknown)?;
                // MinIO's bucket-wide listing ignores max-uploads/markers. Keep its
                // local-only buffer bounded; Wasabi must honor our requested page bound.
                let limit = if matches!(self.identity, Identity::Minio(_)) {
                    1000
                } else {
                    100
                };
                if v.uploads().len() > limit {
                    return Err(ProviderError::Unavailable);
                }
                let after = if v.is_truncated() == Some(true) {
                    let next = v.next_key_marker().ok_or(ProviderError::Unknown)?;
                    let next_upload = v.next_upload_id_marker().map(str::to_owned);
                    if Some(next) == key.as_deref() && next_upload == *upload {
                        return Err(ProviderError::Unknown);
                    }
                    Some(UploadCursor {
                        key: next.into(),
                        upload: next_upload,
                    })
                } else {
                    None
                };
                let mut pending = Vec::new();
                for u in v.uploads() {
                    pending.push(UploadItem {
                        key: u.key().ok_or(ProviderError::Unknown)?.into(),
                        upload: u.upload_id().ok_or(ProviderError::Unknown)?.into(),
                    });
                }
                self.batch(b, UploadBatch { pending, after }).await
            }
            Some(Cursor::Batch(batch)) => self.batch(b, batch.clone()).await,
            Some(Cursor::Parts {
                key,
                upload,
                part,
                batch,
            }) => {
                self.parts(b, key, upload, part.clone(), batch.clone())
                    .await
            }
        }
    }
    async fn purge(&self, b: &Binding) -> ProviderResult<bool> {
        let versions = self
            .s3
            .list_object_versions()
            .bucket(&b.bucket)
            .max_keys(100)
            .send()
            .await
            .map_err(unknown)?;
        let mut objects = vec![];
        for (key, version) in versions
            .versions()
            .iter()
            .map(|v| (v.key(), v.version_id()))
            .chain(
                versions
                    .delete_markers()
                    .iter()
                    .map(|v| (v.key(), v.version_id())),
            )
        {
            objects.push(
                ObjectIdentifier::builder()
                    .key(key.ok_or(ProviderError::Unknown)?)
                    .version_id(version.ok_or(ProviderError::Unknown)?)
                    .build()
                    .map_err(unknown)?,
            );
        }
        if !objects.is_empty() {
            let result = self
                .s3
                .delete_objects()
                .bucket(&b.bucket)
                .delete(
                    Delete::builder()
                        .set_objects(Some(objects))
                        .build()
                        .map_err(unknown)?,
                )
                .send()
                .await
                .map_err(unknown)?;
            if !result.errors().is_empty() {
                return Err(ProviderError::Unknown);
            }
            return Ok(false);
        }
        let uploads = self
            .s3
            .list_multipart_uploads()
            .bucket(&b.bucket)
            .max_uploads(10)
            .send()
            .await
            .map_err(unknown)?;
        for u in uploads.uploads().iter().take(10) {
            self.s3
                .abort_multipart_upload()
                .bucket(&b.bucket)
                .key(u.key().ok_or(ProviderError::Unknown)?)
                .upload_id(u.upload_id().ok_or(ProviderError::Unknown)?)
                .send()
                .await
                .map_err(unknown)?;
        }
        Ok(uploads.uploads().is_empty() && uploads.is_truncated() != Some(true))
    }
    async fn delete_empty(&self, b: &Binding) -> ProviderResult<()> {
        self.ensure_owned(b).await?;
        // Walk the two empty metadata categories; one S3 page always advances categories.
        let versions = self.inventory(b, None).await?;
        let uploads = self
            .inventory(
                b,
                Some(&Cursor::Uploads {
                    key: None,
                    upload: None,
                }),
            )
            .await?;
        if versions.current_objects > 0
            || versions.noncurrent_versions > 0
            || versions.delete_markers > 0
            || !uploads.empty()
        {
            return Err(ProviderError::Unavailable);
        }
        if !self.keys(b).await?.is_empty() {
            return Err(ProviderError::Unavailable);
        }
        self.s3
            .delete_bucket()
            .bucket(&b.bucket)
            .send()
            .await
            .map_err(unknown)?;
        match &self.identity {
            Identity::Minio(m) => m.delete(b).await?,
            Identity::Wasabi(i) => {
                i.delete_user_policy()
                    .user_name(&b.user)
                    .policy_name("webdock-bucket")
                    .send()
                    .await
                    .map_err(unknown)?;
                i.delete_user()
                    .user_name(&b.user)
                    .send()
                    .await
                    .map_err(unknown)?;
            }
        }
        if self.bucket_exists(b).await? || self.identity_exists(b).await? {
            return Err(ProviderError::Unknown);
        }
        Ok(())
    }
}

fn batch_next(batch: UploadBatch) -> Option<Cursor> {
    if !batch.pending.is_empty() {
        return Some(Cursor::Batch(batch));
    }
    batch.after.map(|c| Cursor::Uploads {
        key: Some(c.key),
        upload: c.upload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observed_metadata_has_no_targets_and_never_confuses_composite_or_unknown_hashes() {
        use base64::Engine;
        let checksum = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let head = aws_sdk_s3::operation::head_object::HeadObjectOutput::builder()
            .content_length(3)
            .e_tag("\"abc123\"")
            .content_type("text/html")
            .checksum_sha256(&checksum)
            .checksum_type(ChecksumType::FullObject)
            .build();
        let metadata = object_metadata("reports/proof.txt", &head).unwrap();
        assert_eq!(metadata.version_id, "null");
        assert_eq!(metadata.content_type, "application/octet-stream");
        assert_eq!(metadata.expected_sha256, Some(hex::encode([7u8; 32])));
        let value = serde_json::to_value(metadata).unwrap();
        for key in [
            "url",
            "headers",
            "bucket",
            "access_key",
            "secret_key",
            "endpoint",
        ] {
            assert!(value.get(key).is_none());
        }
        let mut composite = head.clone();
        composite.checksum_type = Some(ChecksumType::Composite);
        assert!(
            object_metadata("reports/proof.txt", &composite)
                .unwrap()
                .expected_sha256
                .is_none()
        );
        assert!(object_checksum(&composite).is_none());
        let mut unknown = head.clone();
        unknown.checksum_type = None;
        assert!(
            object_metadata("reports/proof.txt", &unknown)
                .unwrap()
                .expected_sha256
                .is_none()
        );
        let mut too_large = head;
        too_large.content_length = Some(536870913);
        assert!(object_metadata("reports/proof.txt", &too_large).is_err());
    }
    #[cfg(not(feature = "local-native-fixture"))]
    #[tokio::test]
    async fn production_build_rejects_native_fixture_profile() {
        let c = serde_json::from_value(serde_json::json!({
            "id":"test", "kind":"minio-native-fixture", "endpoint":"https://abcdef123456.native-object-storage.test:19443",
            "region":"us-east-1", "account":"owned-test", "accessKey":"test", "secretKey":"test",
            "minio":{"executable":"/nonexistent/mc", "configDir":"/nonexistent/config"}
        })).unwrap();
        assert!(matches!(
            Control::new(c).await,
            Err("Unsupported storage endpoint or backend")
        ));
    }
    #[cfg(feature = "local-native-fixture")]
    #[test]
    fn native_fixture_transport_requires_named_https_test_target() {
        for endpoint in [
            "http://abcdef123456.native-object-storage.test",
            "https://example.com",
            "https://127.0.0.1",
            "https://abcdef123456.native-object-storage.test.evil.test",
        ] {
            assert!(!native_fixture_endpoint(
                &url::Url::parse(endpoint).unwrap()
            ));
        }
        assert!(native_fixture_endpoint(
            &url::Url::parse("https://abcdef123456.native-object-storage.test:9443").unwrap()
        ));
    }
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = vec![];
        loop {
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
            let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let header = String::from_utf8_lossy(&request[..end]);
            let length: usize = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse().ok())
                })
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                return String::from_utf8(request).unwrap();
            }
        }
    }

    // Exercise SDK deserialization and actual emitted requests without any account/backend.
    fn scripted(
        responses: Vec<(u16, String)>,
    ) -> (
        Control,
        Arc<Mutex<Vec<String>>>,
        std::thread::JoinHandle<()>,
    ) {
        scripted_headers(
            responses
                .into_iter()
                .map(|(status, body)| (status, String::new(), body))
                .collect(),
        )
    }
    fn scripted_headers(
        responses: Vec<(u16, String, String)>,
    ) -> (
        Control,
        Arc<Mutex<Vec<String>>>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let captured = requests.clone();
        let worker = std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline, "expected SDK request");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => panic!("fixture listener failed"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                captured.lock().unwrap().push(read_request(&mut stream));
                write!(stream, "HTTP/1.1 {status} Fixture\r\n{headers}Content-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let credentials = Credentials::new("fixture", "fixture-secret", None, None, "test");
        let s3 = S3::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new("us-east-1"))
                .endpoint_url(&endpoint)
                .force_path_style(true)
                .credentials_provider(credentials.clone())
                .retry_config(RetryConfig::disabled())
                .build(),
        );
        let iam = Iam::from_conf(
            aws_sdk_iam::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new("us-east-1"))
                .endpoint_url(&endpoint)
                .credentials_provider(credentials)
                .retry_config(RetryConfig::disabled())
                .build(),
        );
        (
            Control {
                backend: "fixture".into(),
                endpoint,
                region: "us-east-1".into(),
                s3,
                identity: Identity::Wasabi(iam),
            },
            requests,
            worker,
        )
    }
    fn binding() -> Binding {
        Binding {
            id: uuid::Uuid::new_v4(),
            customer: "fixture".into(),
            project: "fixture".into(),
            environment: "fixture".into(),
            backend: "fixture".into(),
            bucket: "fixture".into(),
            user: "fixture".into(),
        }
    }
    // This is the SDK HTTP boundary, not a claim that local MinIO labels its checksums FullObject.
    #[tokio::test]
    async fn full_object_checksum_cannot_be_downgraded_by_omitting_the_caller_hash() {
        use base64::Engine;
        use webdock_plugin_protocol::storage::StorageDirection;
        let hash = hex::encode([7u8; 32]);
        for (kind, expected, allowed) in [
            (Some("FULL_OBJECT"), None, false),
            (Some("FULL_OBJECT"), Some(hash.clone()), true),
            (Some("FULL_OBJECT"), Some("0".repeat(64)), false),
            (Some("COMPOSITE"), None, true),
            (None, None, true),
            (None, Some("0".repeat(64)), true),
        ] {
            let checksum = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
            let headers = format!(
                "ETag: \"abc123\"\r\nx-amz-version-id: version-1\r\nx-amz-checksum-sha256: {checksum}\r\n{}",
                kind.map(|value| format!("x-amz-checksum-type: {value}\r\n"))
                    .unwrap_or_default()
            );
            let (provider, requests, worker) =
                scripted_headers(vec![(200, headers, String::new())]);
            let b = binding();
            let request = crate::data::DataRequest {
                resource_id: b.id,
                key: "proof.txt".into(),
                exact_bytes: 0,
                expected_sha256: expected.clone(),
                filename: "proof.txt".into(),
                content_type: "text/plain".into(),
                expires_at: now() as i64 + 60,
                version_id: Some("version-1".into()),
                if_match: Some("\"abc123\"".into()),
            };
            let result = provider
                .presign(&b, StorageDirection::Download, &request)
                .await;
            worker.join().unwrap();
            assert_eq!(
                result.is_ok(),
                allowed,
                "kind={kind:?}, caller_hash={expected:?}"
            );
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].starts_with("HEAD /fixture/proof.txt?versionId=version-1 "));
            assert!(
                requests[0]
                    .to_ascii_lowercase()
                    .contains("if-match: \"abc123\"")
            );
            assert!(
                requests[0]
                    .to_ascii_lowercase()
                    .contains("x-amz-checksum-mode: enabled")
            );
        }
    }
    #[tokio::test]
    async fn completed_or_aborted_multipart_cursor_is_scan_invalidation() {
        let (provider, _, worker) = scripted(vec![(
            404,
            "<Error><Code>NoSuchUpload</Code><Message>Upload no longer exists</Message></Error>"
                .into(),
        )]);
        let result = provider
            .inventory(
                &binding(),
                Some(&Cursor::Parts {
                    key: "object".into(),
                    upload: "gone".into(),
                    part: Some("100".into()),
                    batch: UploadBatch {
                        pending: vec![],
                        after: None,
                    },
                }),
            )
            .await;
        worker.join().unwrap();
        assert!(matches!(result, Err(ProviderError::ScanInvalidated)));
    }
    #[tokio::test]
    async fn concurrently_removed_version_marker_is_scan_invalidation() {
        let (provider, _, worker) = scripted(vec![(
            400,
            "<Error><Code>InvalidArgument</Code><Message>Invalid version marker</Message></Error>"
                .into(),
        )]);
        let result = provider
            .inventory(
                &binding(),
                Some(&Cursor::Versions {
                    key: Some("removed".into()),
                    version: Some("gone".into()),
                }),
            )
            .await;
        worker.join().unwrap();
        assert!(matches!(result, Err(ProviderError::ScanInvalidated)));
    }
    #[tokio::test]
    async fn partial_purge_errors_never_report_complete_and_preserve_null_versions_and_markers() {
        let (provider, requests, worker) = scripted(vec![
            (
                200,
                "<ListVersionsResult><IsTruncated>false</IsTruncated><Version><Key>plain</Key><VersionId>null</VersionId><IsLatest>true</IsLatest><Size>5</Size></Version><DeleteMarker><Key>deleted</Key><VersionId>marker-id</VersionId><IsLatest>true</IsLatest></DeleteMarker></ListVersionsResult>".into(),
            ),
            (
                200,
                "<DeleteResult><Deleted><Key>plain</Key><VersionId>null</VersionId></Deleted><Error><Key>deleted</Key><VersionId>marker-id</VersionId><Code>AccessDenied</Code></Error></DeleteResult>".into(),
            ),
        ]);
        assert!(matches!(
            provider.purge(&binding()).await,
            Err(ProviderError::Unknown)
        ));
        worker.join().unwrap();
        let requests = requests.lock().unwrap();
        assert!(requests[1].contains("<VersionId>null</VersionId>"));
        assert!(requests[1].contains("<VersionId>marker-id</VersionId>"));
    }
    #[tokio::test]
    async fn standard_upload_markers_cross_one_hundred_uploads_without_duplicates() {
        let mut xml = "<ListMultipartUploadsResult><IsTruncated>true</IsTruncated><NextKeyMarker>key099</NextKeyMarker><NextUploadIdMarker>id099</NextUploadIdMarker>".to_owned();
        for n in 0..100 {
            xml.push_str(&format!(
                "<Upload><Key>key{n:03}</Key><UploadId>id{n:03}</UploadId></Upload>"
            ));
        }
        xml.push_str("</ListMultipartUploadsResult>");
        let parts = "<ListPartsResult><IsTruncated>false</IsTruncated><Part><PartNumber>1</PartNumber><Size>1</Size></Part></ListPartsResult>";
        let mut replies = vec![(200, xml)];
        replies.extend((0..100).map(|_| (200, parts.into())));
        replies.push((200, "<ListMultipartUploadsResult><IsTruncated>false</IsTruncated><Upload><Key>key100</Key><UploadId>id100</UploadId></Upload></ListMultipartUploadsResult>".into()));
        replies.push((200, parts.into()));
        let (provider, requests, worker) = scripted(replies);
        let mut cursor = Some(Cursor::Uploads {
            key: None,
            upload: None,
        });
        let mut count = 0;
        let mut bytes = 0;
        for _ in 0..103 {
            let page = provider
                .inventory(&binding(), cursor.as_ref())
                .await
                .unwrap();
            count += page.multipart_uploads;
            bytes += page.multipart_bytes;
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        worker.join().unwrap();
        assert!(cursor.is_none());
        assert_eq!((count, bytes), (101, 101));
        assert!(requests.lock().unwrap()[101].contains("key-marker=key099"));
        assert!(requests.lock().unwrap()[101].contains("upload-id-marker=id099"));
    }
    #[tokio::test]
    async fn purge_never_aborts_more_than_ten_even_when_local_server_ignores_page_size() {
        let mut xml = String::from("<ListMultipartUploadsResult><IsTruncated>false</IsTruncated>");
        for n in 0..11 {
            xml.push_str(&format!(
                "<Upload><Key>key{n}</Key><UploadId>id{n}</UploadId></Upload>"
            ));
        }
        xml.push_str("</ListMultipartUploadsResult>");
        let mut replies = vec![
            (
                200,
                "<ListVersionsResult><IsTruncated>false</IsTruncated></ListVersionsResult>".into(),
            ),
            (200, xml),
        ];
        replies.extend((0..10).map(|_| (204, String::new())));
        let (provider, requests, worker) = scripted(replies);
        let result = provider.purge(&binding()).await;
        worker.join().unwrap();
        assert_eq!(result, Ok(false));
        assert_eq!(requests.lock().unwrap().len(), 12);
    }
}
