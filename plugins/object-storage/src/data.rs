//! Private metadata-only producers. Core owns public descriptors and every byte transfer.
use crate::{engine::Engine, model::*, store::Result};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use webdock_plugin_protocol::storage::{
    Attachment, MAX_PLAN_SECONDS, MAX_TRANSFER_BYTES, StorageAuthorization, StorageDirection,
    StoragePlan,
};

pub const STORAGE_BINDING: &str = "managed-objects";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRequest {
    pub resource_id: uuid::Uuid,
    pub key: String,
    pub exact_bytes: u64,
    pub expected_sha256: Option<String>,
    pub filename: String,
    pub content_type: String,
    pub expires_at: i64,
    pub version_id: Option<String>,
    pub if_match: Option<String>,
}
impl DataRequest {
    pub fn validate(&self, direction: StorageDirection, now: i64) -> Result<()> {
        if !valid_key(&self.key)
            || self
                .expected_sha256
                .as_deref()
                .is_some_and(|hash| !valid_hash(hash))
            || direction == StorageDirection::Upload && self.expected_sha256.is_none()
            || self.exact_bytes > MAX_TRANSFER_BYTES
            || self.expires_at <= now
            || self.expires_at.saturating_sub(now) > MAX_PLAN_SECONDS
            || self.filename.is_empty()
            || self.filename.len() > 160
            || !self
                .filename
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._- ".contains(&b))
            || ![
                "application/octet-stream",
                "application/pdf",
                "application/zip",
                "application/gzip",
                "text/plain",
                "text/csv",
            ]
            .contains(&self.content_type.as_str())
        {
            return Err("Invalid data plan");
        }
        match direction {
            StorageDirection::Upload if self.version_id.is_none() && self.if_match.is_none() => {
                Ok(())
            }
            StorageDirection::Download => {
                let version = self
                    .version_id
                    .as_deref()
                    .ok_or("Exact object version required")?;
                let tag = self
                    .if_match
                    .as_deref()
                    .ok_or("Exact object ETag required")?;
                if version.is_empty()
                    || version.len() > 512
                    || !version
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/+=._-".contains(&b))
                    || tag.len() < 3
                    || tag.len() > 130
                    || !tag.starts_with('"')
                    || !tag.ends_with('"')
                    || !tag.as_bytes()[1..tag.len() - 1]
                        .iter()
                        .all(|b| b.is_ascii_hexdigit() || *b == b'-')
                {
                    return Err("Invalid object version");
                }
                Ok(())
            }
            StorageDirection::Upload => Err("Upload is create-only"),
        }
    }
}
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 512
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
        && key
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveRequest {
    resource_id: uuid::Uuid,
    key: String,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectMetadata {
    pub key: String,
    pub exact_bytes: u64,
    pub version_id: String,
    pub if_match: String,
    pub filename: String,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
}
impl ObjectMetadata {
    pub fn validate(&self, now: i64) -> Result<()> {
        DataRequest {
            resource_id: uuid::Uuid::nil(),
            key: self.key.clone(),
            exact_bytes: self.exact_bytes,
            expected_sha256: self.expected_sha256.clone(),
            filename: self.filename.clone(),
            content_type: self.content_type.clone(),
            expires_at: now + 60,
            version_id: Some(self.version_id.clone()),
            if_match: Some(self.if_match.clone()),
        }
        .validate(StorageDirection::Download, now)
    }
}
// No Debug: URLs and signed headers are private RPC material.
pub struct PresignedObject {
    pub url: String,
    pub headers: BTreeMap<String, String>,
}
fn epoch(r: &Resource) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(&(&r.binding, r.revision)).expect("binding serialization"),
    ))
}
fn active(
    r: &Resource,
    id: &uuid::Uuid,
    customer: &str,
    project: &str,
    backend: &str,
) -> Result<()> {
    if r.binding.id != *id
        || r.binding.customer != customer
        || r.binding.project != project
        || r.binding.backend != backend
        || r.binding.bucket != format!("wd-{}", r.binding.id.simple())
        || r.binding.user != format!("wd-{}", r.binding.id.simple())
        || r.state != "active"
        || !r.provisioned
        || r.pending.is_some()
        || r.executing
    {
        return Err("Data access unavailable");
    }
    Ok(())
}
impl Engine {
    pub async fn observe_object(
        &self,
        customer: &str,
        project: &str,
        input: Value,
    ) -> Result<ObjectMetadata> {
        let request: ObserveRequest =
            serde_json::from_value(input).map_err(|_| "Invalid object input")?;
        if !valid_key(&request.key) {
            return Err("Invalid object key");
        }
        let r = self
            .store
            .read(&request.resource_id)
            .await?
            .ok_or("Data access unavailable")?;
        active(
            &r,
            &request.resource_id,
            customer,
            project,
            self.provider.backend(),
        )?;
        let owned = self
            .provider
            .observe(&r.binding)
            .await
            .map_err(|_| "Data ownership unavailable")?;
        if !owned.exists || !owned.owned {
            return Err("Data access unavailable");
        }
        let metadata = self
            .provider
            .head_object(&r.binding, &request.key)
            .await
            .map_err(|_| "Object metadata unavailable")?;
        metadata.validate(now() as i64)?;
        if metadata.key != request.key {
            return Err("Object identity changed");
        }
        let fresh = self
            .store
            .read(&request.resource_id)
            .await?
            .ok_or("Data access unavailable")?;
        active(
            &fresh,
            &request.resource_id,
            customer,
            project,
            self.provider.backend(),
        )?;
        if epoch(&fresh) != epoch(&r) {
            return Err("Data authority changed");
        }
        Ok(metadata)
    }
    /// Never acquire the global mutation fence or persist an expiring plan in operation results.
    pub async fn prepare_data(
        &self,
        customer: &str,
        project: &str,
        direction: StorageDirection,
        input: Value,
    ) -> Result<StoragePlan> {
        let request: DataRequest =
            serde_json::from_value(input).map_err(|_| "Invalid data input")?;
        request.validate(direction, now() as i64)?;
        let r = self
            .store
            .read(&request.resource_id)
            .await?
            .ok_or("Data access unavailable")?;
        active(
            &r,
            &request.resource_id,
            customer,
            project,
            self.provider.backend(),
        )?;
        let observed = self
            .provider
            .observe(&r.binding)
            .await
            .map_err(|_| "Data ownership unavailable")?;
        if !observed.exists || !observed.owned {
            return Err("Data access unavailable");
        }
        let signed = self
            .provider
            .presign(&r.binding, direction, &request)
            .await
            .map_err(|_| "Data plan unavailable")?;
        // A concurrent deactivation commits pending intent before touching the provider.
        let fresh = self
            .store
            .read(&request.resource_id)
            .await?
            .ok_or("Data access unavailable")?;
        active(
            &fresh,
            &request.resource_id,
            customer,
            project,
            self.provider.backend(),
        )?;
        if epoch(&fresh) != epoch(&r) {
            return Err("Data authority changed");
        }
        let plan = StoragePlan {
            version: 1,
            binding: STORAGE_BINDING.into(),
            direction,
            resource_id: r.binding.id.to_string(),
            resource_epoch: epoch(&r),
            url: signed.url,
            headers: signed.headers,
            expires_at: request.expires_at,
            exact_bytes: request.exact_bytes,
            expected_sha256: request.expected_sha256,
            attachment: Attachment {
                filename: request.filename,
                content_type: request.content_type,
            },
        };
        plan.validate(direction.as_str(), &r.binding.id.to_string(), now() as i64)
            .map_err(|_| "Invalid provider plan")?;
        Ok(plan)
    }
    /// Called repeatedly by the authenticated Core runtime, including before final delivery.
    pub async fn authorize_data(&self, a: &StorageAuthorization) -> Result<()> {
        a.validate("object-storage", a.generation, now() as i64)
            .map_err(|_| "Data access unavailable")?;
        if a.action
            != match a.direction {
                StorageDirection::Upload => "objects.upload",
                StorageDirection::Download => "objects.download",
            }
        {
            return Err("Data action mismatch");
        }
        let id = a
            .resource_id
            .parse()
            .map_err(|_| "Data access unavailable")?;
        let r = self
            .store
            .read(&id)
            .await?
            .ok_or("Data access unavailable")?;
        active(
            &r,
            &id,
            &a.customer_id,
            &a.project_id,
            self.provider.backend(),
        )?;
        if epoch(&r) != a.resource_epoch {
            return Err("Data authority changed");
        }
        // Binding/state/epoch are private authority. Do not wait behind a lifecycle operation's fence.
        Ok(())
    }
}
