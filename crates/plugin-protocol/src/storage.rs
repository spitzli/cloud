//! Private runtime plans never form part of a public tool result or transfer URL.
use crate::{PluginError, Result, validate_value};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_STORAGE_PLAN_BYTES: usize = 12 * 1024;
pub const MAX_TRANSFER_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_PLAN_SECONDS: i64 = 300;
pub const TRANSFER_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageDirection {
    Upload,
    Download,
}
impl StorageDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }
    pub fn method(self) -> &'static str {
        match self {
            Self::Upload => "PUT",
            Self::Download => "GET",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
}
/// Deliberately redacts URL and headers in diagnostics. Serialize only on the private RPC/ledger.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoragePlan {
    pub version: u8,
    pub binding: String,
    pub direction: StorageDirection,
    pub resource_id: String,
    /// Private producer epoch; rotate on deactivate/reactivate or any data-authority change.
    pub resource_epoch: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub expires_at: i64,
    pub exact_bytes: u64,
    pub expected_sha256: Option<String>,
    pub attachment: Attachment,
}
impl std::fmt::Debug for StoragePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoragePlan([private])")
    }
}
impl StoragePlan {
    pub fn validate(&self, direction: &str, resource: &str, now: i64) -> Result<()> {
        if self.version != 1
            || self.direction.as_str() != direction
            || self.resource_id != resource
            || resource.is_empty()
            || resource.len() > 256
            || self.resource_epoch.is_empty()
            || self.resource_epoch.len() > 128
            || !self
                .resource_epoch
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || self.binding.is_empty()
            || self.binding.len() > 64
            || !self
                .binding
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || self.expires_at <= now
            || self.expires_at.saturating_sub(now) > MAX_PLAN_SECONDS
            || self.exact_bytes > MAX_TRANSFER_BYTES
            || self.url.len() > 8192
            || self.headers.len() > 16
            || self
                .headers
                .iter()
                .any(|(k, v)| k.len() > 128 || v.len() > 2048)
            || self.expected_sha256.as_ref().is_some_and(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            || self.direction == StorageDirection::Upload && self.expected_sha256.is_none()
            || self.attachment.filename.is_empty()
            || self.attachment.filename.len() > 160
            || !self
                .attachment
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
            .contains(&self.attachment.content_type.as_str())
        {
            return Err(PluginError::denied());
        }
        let url = url::Url::parse(&self.url).map_err(|_| PluginError::denied())?;
        if !["https", "http"].contains(&url.scheme())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.host_str().is_none()
        {
            return Err(PluginError::denied());
        }
        if serde_json::to_vec(self)
            .map_err(|_| PluginError::denied())?
            .len()
            > MAX_STORAGE_PLAN_BYTES
        {
            return Err(PluginError::denied());
        }
        Ok(())
    }
}
/// Ordinary actions retain their manifest output validation. Storage has a separate private ABI;
/// Core alone creates and validates the public manifest output after intercepting this plan.
pub fn validate_storage_result(
    action: &Value,
    input: &Value,
    result: &Value,
    now: i64,
) -> Result<()> {
    if let Some(direction) = action.get("storage").and_then(Value::as_str) {
        let plan: StoragePlan =
            serde_json::from_value(result.clone()).map_err(|_| PluginError::denied())?;
        plan.validate(
            direction,
            input["resource_id"]
                .as_str()
                .ok_or_else(PluginError::denied)?,
            now,
        )
    } else {
        validate_value(action.get("output"), result)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferDescriptor {
    pub version: u8,
    pub id: String,
    pub path: String,
    pub method: String,
    pub expires_at: i64,
    pub exact_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
    pub attachment: Attachment,
}
impl TransferDescriptor {
    pub fn new(id: &str, plan: &StoragePlan) -> Result<Self> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(PluginError::denied());
        }
        Ok(Self {
            version: 1,
            id: id.into(),
            path: format!("/api/console/plugins/storage/{id}"),
            method: plan.direction.method().into(),
            expires_at: plan.expires_at,
            exact_bytes: plan.exact_bytes,
            expected_sha256: plan.expected_sha256.clone(),
            attachment: plan.attachment.clone(),
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageReceipt {
    pub bytes: u64,
    pub sha256: String,
    /// Opaque provider receipt, never a content digest.
    pub etag: Option<String>,
}

/// Side-effect-free private runtime callback, authenticated as Core on the existing pinned listener.
/// A producer must check private ownership and require active state, no pending lifecycle
/// transition, and the exact current epoch. Rotate the epoch on every deactivation/reactivation.
/// Successful presigning alone never answers this callback. Default runtime implementations deny.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageAuthorization {
    pub version: u8,
    pub transfer_id: String,
    pub source_invocation: String,
    pub plugin_id: String,
    pub generation: u64,
    pub action: String,
    pub customer_id: String,
    pub project_id: String,
    pub resource_id: String,
    pub resource_epoch: String,
    pub direction: StorageDirection,
    pub exact_bytes: u64,
    pub expected_sha256: Option<String>,
    pub expires_at: i64,
}
impl StorageAuthorization {
    pub fn validate(&self, plugin: &str, generation: u64, now: i64) -> Result<()> {
        if self.version != 1
            || self.plugin_id != plugin
            || self.generation != generation
            || uuid::Uuid::parse_str(&self.transfer_id).is_err()
            || uuid::Uuid::parse_str(&self.source_invocation).is_err()
            || self.action.is_empty()
            || self.action.len() > 64
            || self.resource_id.is_empty()
            || self.resource_id.len() > 256
            || self.resource_epoch.is_empty()
            || self.resource_epoch.len() > 128
            || [&self.customer_id, &self.project_id]
                .iter()
                .any(|s| s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()))
            || self.exact_bytes > MAX_TRANSFER_BYTES
            || self.expires_at <= now
            || self.expires_at.saturating_sub(now) > MAX_PLAN_SECONDS
            || self
                .expected_sha256
                .as_ref()
                .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            || self.direction == StorageDirection::Upload && self.expected_sha256.is_none()
        {
            return Err(PluginError::denied());
        }
        Ok(())
    }
}
