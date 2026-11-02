//! Product-private signing/deletion. Shared transport remains the verified byte path.
use crate::{MAX_BYTES, Result};
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region, retry::RetryConfig, timeout::TimeoutConfig},
    presigning::PresigningConfig,
    types::ExpirationStatus,
};
use serde::Deserialize;
use std::time::Duration;
use webdock_object_storage::{ObjectReceipt, S3Config, Store};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendConfig {
    pub config: S3Config,
    pub credentials: Keys,
    #[cfg(feature = "local-native-fixture")]
    #[serde(default)]
    pub fixture_profile: Option<crate::fixture::NativeProfile>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Keys {
    pub access_key_id: String,
    pub secret_access_key: String,
}
pub struct Backend {
    pub(crate) bytes: Store,
    s3: Client,
    bucket: String,
    namespace: String,
    local: bool,
    fixture_deadline: Option<i64>,
}
impl Backend {
    pub async fn new(c: BackendConfig) -> Result<Self> {
        Self::configured(c, false).await
    }
    #[cfg(feature = "local-test")]
    pub async fn local_test(c: BackendConfig) -> Result<Self> {
        Self::configured(c, true).await
    }
    async fn configured(c: BackendConfig, local: bool) -> Result<Self> {
        crate::initialize_crypto();
        #[cfg(feature = "local-native-fixture")]
        let fixture_deadline = if let Some(profile) = &c.fixture_profile {
            profile.validate(&c.config, crate::now())?;
            profile.verify_ca()?;
            Some(profile.expires_at)
        } else {
            None
        };
        #[cfg(not(feature = "local-native-fixture"))]
        let fixture_deadline: Option<i64> = None;
        let credentials = webdock_object_storage::Credentials::new(
            c.credentials.access_key_id.clone(),
            c.credentials.secret_access_key.clone(),
        )
        .map_err(|_| "Invalid storage credentials")?;
        let bytes = if local {
            #[cfg(feature = "local-test")]
            {
                Store::loopback_test(c.config.clone(), credentials)
            }
            #[cfg(not(feature = "local-test"))]
            {
                return Err("Local storage disabled");
            }
        } else {
            Store::s3(c.config.clone(), credentials)
        }
        .map_err(|_| "Invalid storage configuration")?;
        let s3 = Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new(c.config.region.clone()))
                .endpoint_url(&c.config.endpoint)
                .credentials_provider(Credentials::new(
                    c.credentials.access_key_id,
                    c.credentials.secret_access_key,
                    None,
                    None,
                    "archives-private-config",
                ))
                .force_path_style(true)
                .retry_config(RetryConfig::disabled())
                .timeout_config(
                    TimeoutConfig::builder()
                        .operation_timeout(Duration::from_secs(10))
                        .operation_attempt_timeout(Duration::from_secs(10))
                        .build(),
                )
                .build(),
        );
        let backend = Self {
            bytes,
            s3,
            bucket: c.config.bucket,
            namespace: c.config.namespace,
            local: local || fixture_deadline.is_some(),
            fixture_deadline,
        };
        backend.retention().await?;
        Ok(backend)
    }
    pub(crate) fn active(&self) -> Result<()> {
        if self
            .fixture_deadline
            .is_some_and(|until| until <= crate::now())
        {
            return Err("Archive fixture expired");
        }
        Ok(())
    }
    async fn retention(&self) -> Result<()> {
        self.active()?;
        match self
            .s3
            .get_bucket_policy()
            .bucket(&self.bucket)
            .send()
            .await
        {
            Err(e)
                if e.as_service_error()
                    .is_some_and(|e| e.meta().code() == Some("NoSuchBucketPolicy")) => {}
            _ => return Err("Archive bucket policy must be private and absent"),
        }
        // Local MinIO does not implement the GetBucketAcl IAM action; it authorizes through policies.
        if !self.local {
            let acl = self
                .s3
                .get_bucket_acl()
                .bucket(&self.bucket)
                .send()
                .await
                .map_err(|_| "Private archive ACL unavailable")?;
            if !private_acl(&acl) {
                return Err("Private archive ACL required");
            }
        }

        let v = self
            .s3
            .get_bucket_versioning()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|_| "Storage retention unavailable")?;
        if v.status().is_some() {
            return Err("Archive bucket must never be versioned");
        }
        let lifecycle = self
            .s3
            .get_bucket_lifecycle_configuration()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|_| "Archive retention policy unavailable")?;
        let r = lifecycle.rules();
        if r.len() != 1
            || r[0].id() != Some("webdock-archives-retention")
            || r[0].status() != &ExpirationStatus::Enabled
            || r[0]
                .expiration()
                .and_then(|v| v.days())
                .is_none_or(|d| !(1..=7).contains(&d))
            || r[0]
                .abort_incomplete_multipart_upload()
                .and_then(|v| v.days_after_initiation())
                .map_or(!self.local, |d| d != 1)
            || r[0]
                .filter()
                .and_then(|f| f.prefix())
                .is_none_or(|p| p != format!("{}/", self.namespace))
        {
            return Err("Bounded archive retention required");
        }
        Ok(())
    }
    pub(crate) async fn upload(
        &self,
        scope: &str,
        source: impl tokio::io::AsyncRead + Unpin,
    ) -> Result<ObjectReceipt> {
        self.retention().await?;
        self.bytes
            .upload(scope, source, MAX_BYTES)
            .await
            .map_err(|_| "Archive upload unavailable")
    }
    pub(crate) async fn sign(
        &self,
        scope: &str,
        r: &ObjectReceipt,
        seconds: u64,
    ) -> Result<(String, std::collections::BTreeMap<String, String>)> {
        self.active()?;
        self.bytes
            .validate_receipt(scope, r, MAX_BYTES)
            .map_err(|_| "Archive receipt damaged")?;
        let head = self
            .s3
            .head_object()
            .bucket(&self.bucket)
            .key(&r.object_key)
            .send()
            .await
            .map_err(|_| "Archive unavailable")?;
        if head.content_length() != Some(r.size_bytes as i64) {
            return Err("Archive integrity mismatch");
        }
        let etag = head.e_tag().ok_or("Archive receipt unavailable")?;
        let p = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(&r.object_key)
            .if_match(etag)
            .presigned(
                PresigningConfig::expires_in(Duration::from_secs(seconds))
                    .map_err(|_| "Archive signing unavailable")?,
            )
            .await
            .map_err(|_| "Archive signing unavailable")?;
        let headers = p
            .headers()
            .filter(|(k, _)| *k != "host")
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        Ok((p.uri().into(), headers))
    }
    pub(crate) async fn delete(&self, scope: &str, r: &ObjectReceipt) -> Result<()> {
        self.active()?;
        self.bytes
            .validate_receipt(scope, r, MAX_BYTES)
            .map_err(|_| "Archive receipt damaged")?;
        self.retention().await?;
        self.s3
            .delete_object()
            .bucket(&self.bucket)
            .key(&r.object_key)
            .send()
            .await
            .map_err(|_| "Archive deletion unavailable")?;
        match self
            .s3
            .head_object()
            .bucket(&self.bucket)
            .key(&r.object_key)
            .send()
            .await
        {
            Err(e) if e.as_service_error().is_some_and(|e| e.is_not_found()) => Ok(()),
            _ => Err("Archive deletion not observed"),
        }
    }
}

fn private_acl(acl: &aws_sdk_s3::operation::get_bucket_acl::GetBucketAclOutput) -> bool {
    use aws_sdk_s3::types::{Permission, Type};
    let Some(owner) = acl.owner().and_then(|o| o.id()).filter(|s| !s.is_empty()) else {
        return false;
    };
    if acl.grants().len() != 1 {
        return false;
    }
    let grant = &acl.grants()[0];
    grant.permission() == Some(&Permission::FullControl)
        && grant.grantee().is_some_and(|g| {
            g.r#type() == &Type::CanonicalUser && g.id() == Some(owner) && g.uri().is_none()
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_s3::types::{Grant, Grantee, Owner, Permission, Type};
    #[test]
    fn anonymous_or_foreign_acl_is_never_accepted_as_protected_storage() {
        let owner = Owner::builder().id("owned").build();
        let grantee = Grantee::builder()
            .r#type(Type::CanonicalUser)
            .id("owned")
            .build()
            .unwrap();
        let grant = Grant::builder()
            .grantee(grantee)
            .permission(Permission::FullControl)
            .build();
        let acl = aws_sdk_s3::operation::get_bucket_acl::GetBucketAclOutput::builder()
            .owner(owner.clone())
            .grants(grant.clone())
            .build();
        assert!(private_acl(&acl));
        let public = Grantee::builder()
            .r#type(Type::Group)
            .uri("http://acs.amazonaws.com/groups/global/AllUsers")
            .build()
            .unwrap();
        let public = Grant::builder()
            .grantee(public)
            .permission(Permission::Read)
            .build();
        let acl = aws_sdk_s3::operation::get_bucket_acl::GetBucketAclOutput::builder()
            .owner(owner)
            .grants(grant)
            .grants(public)
            .build();
        assert!(!private_acl(&acl));
        assert!(!private_acl(
            &aws_sdk_s3::operation::get_bucket_acl::GetBucketAclOutput::builder().build()
        ));
    }
}
