//! Tenant-scoped, one-attempt Mail provider protocol. No ORM, project or vendor dependency.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[allow(
    clippy::allow_attributes_without_reason,
    clippy::excessive_nesting,
    reason = "Tonic owns the nesting and lint attributes in this generated protocol module."
)]
pub mod wire {
    tonic::include_proto!("webdock.mail.v1");
}
pub mod runtime;
pub const FRAME_BYTES: usize = 32 * 1024;
pub const MAX_MIME_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_METADATA_BYTES: usize = 32 * 1024;
const DOMAIN: &[u8] = b"webdock.mail.send.lease.v1\0";
#[derive(Debug, thiserror::Error)]
#[error("Mail provider authority denied")]
pub struct Denied;
pub type Result<T> = std::result::Result<T, Denied>;
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn id(value: &str) -> bool {
    !value.starts_with('0')
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|v| v > 0)
}
fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaseClaims {
    pub v: u8,
    pub purpose: String,
    pub audience: String,
    pub id: String,
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    pub generation: u64,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "jobID")]
    pub job_id: String,
    #[serde(rename = "sourceID")]
    pub source_id: String,
    pub transport_revision: u64,
    pub grant_revision: u64,
    pub configuration_revision: u64,
    pub descriptor_sha256: String,
    pub binding_sha256: String,
    #[serde(rename = "workerID")]
    pub worker_id: String,
    pub worker_generation: u64,
    pub worker_credential_sha256: String,
    pub metadata_sha256: String,
    pub mime_sha256: String,
    pub mime_bytes: u64,
    pub iat: i64,
    pub exp: i64,
}
impl LeaseClaims {
    pub fn validate(&self, now: i64) -> Result<()> {
        let source = self
            .source_id
            .strip_prefix("key:")
            .or_else(|| self.source_id.strip_prefix("webmail:"));
        if self.v != 1
            || self.purpose != "mail-send"
            || self.audience != "webdock.mail.runtime.v1"
            || !slug(&self.plugin_id)
            || !slug(&self.worker_id)
            || !id(&self.customer_id)
            || !id(&self.job_id)
            || !(source.is_some_and(id)
                || self
                    .source_id
                    .strip_prefix("action:")
                    .is_some_and(valid_digest))
            || !uuid::Uuid::parse_str(&self.id).is_ok_and(|v| {
                v.to_string() == self.id
                    && v.get_version_num() == 4
                    && v.get_variant() == uuid::Variant::RFC4122
            })
            || [
                self.generation,
                self.worker_generation,
                self.transport_revision,
                self.grant_revision,
            ]
            .iter()
            .any(|v| *v == 0 || *v > i64::MAX as u64)
            || self.configuration_revision > i64::MAX as u64
            || [
                &self.descriptor_sha256,
                &self.binding_sha256,
                &self.worker_credential_sha256,
                &self.metadata_sha256,
                &self.mime_sha256,
            ]
            .iter()
            .any(|s| !valid_digest(s))
            || self.mime_bytes == 0
            || self.mime_bytes > MAX_MIME_BYTES
            || self.iat < 0
            || self.iat > now.saturating_add(5)
            || self.exp <= now
            || self.exp <= self.iat
            || self.exp.saturating_sub(self.iat) > 60
        {
            return Err(Denied);
        }
        Ok(())
    }
    pub fn sign(&self, key: &SigningKey) -> Result<String> {
        self.validate(self.iat)?;
        let bytes = serde_json::to_vec(self).map_err(|_| Denied)?;
        let body = URL_SAFE_NO_PAD.encode(bytes);
        let mut message = DOMAIN.to_vec();
        message.extend_from_slice(body.as_bytes());
        Ok(format!(
            "{body}.{}",
            URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())
        ))
    }
}
pub fn verify_lease(
    token: &str,
    key: &VerifyingKey,
    plugin: &str,
    generation: u64,
    now: i64,
) -> Result<LeaseClaims> {
    if token.len() > 16384 {
        return Err(Denied);
    }
    let (body, signature) = token.split_once('.').ok_or(Denied)?;
    let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(signature).map_err(|_| Denied)?)
        .map_err(|_| Denied)?;
    let mut message = DOMAIN.to_vec();
    message.extend_from_slice(body.as_bytes());
    key.verify_strict(&message, &signature)
        .map_err(|_| Denied)?;
    let claims: LeaseClaims =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).map_err(|_| Denied)?)
            .map_err(|_| Denied)?;
    claims.validate(now)?;
    if claims.plugin_id != plugin || claims.generation != generation {
        return Err(Denied);
    }
    Ok(claims)
}

pub mod outbox;
pub use outbox::{CleanupItem, PayloadFormat, PayloadLease, Preparation, Receipt, SendLease};
pub mod native_control;
