//! Provider-neutral repository leases, separate from interactive plugin Invoke.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub use webdock_plugin_protocol::{ErrorCode, PluginError, Result};
pub const FRAME_BYTES: usize = 32 * 1024;
pub const JSON_BYTES: usize = 64 * 1024;
/// Retained build source ceiling, distinct from artifact output and upload allowances.
pub const SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const LEASE_DOMAIN: &[u8] = b"webdock.repository.lease.v1\0";
const DESCRIPTOR_DOMAIN: &[u8] = b"webdock.repository.descriptor.v1\0";
#[allow(
    clippy::allow_attributes_without_reason,
    clippy::excessive_nesting,
    reason = "Tonic owns the nesting and lint attributes in this generated protocol module."
)]
pub mod wire {
    tonic::include_proto!("webdock.repository.v1");
}
pub mod runtime;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Resolve,
    Archive,
    Check,
}
impl Action {
    pub fn manifest_name(self) -> &'static str {
        match self {
            Self::Resolve => "repository.resolve",
            Self::Archive => "repository.archive",
            Self::Check => "repository.check",
        }
    }
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
    pub action: Action,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "sourceID")]
    pub source_id: String,
    pub source_revision: u64,
    pub connection_generation: u64,
    pub grant_revision: u64,
    pub configuration_revision: u64,
    pub descriptor_sha256: String,
    pub request_sha256: String,
    pub configuration_sha256: String,
    pub iat: i64,
    pub exp: i64,
}
fn invalid(message: &str) -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, message)
}
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn record_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('0')
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|n| n > 0)
}
fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn sign(domain: &[u8], bytes: &[u8], key: &SigningKey) -> String {
    let body = URL_SAFE_NO_PAD.encode(bytes);
    let mut message = domain.to_vec();
    message.extend_from_slice(body.as_bytes());
    format!(
        "{body}.{}",
        URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())
    )
}
fn verify(domain: &[u8], token: &str, key: &VerifyingKey, max: usize) -> Result<Vec<u8>> {
    if token.len() > max {
        return Err(PluginError::denied());
    }
    let (body, signature) = token.split_once('.').ok_or_else(PluginError::denied)?;
    let signature = Signature::from_slice(
        &URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| PluginError::denied())?,
    )
    .map_err(|_| PluginError::denied())?;
    let mut message = domain.to_vec();
    message.extend_from_slice(body.as_bytes());
    key.verify_strict(&message, &signature)
        .map_err(|_| PluginError::denied())?;
    URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| PluginError::denied())
}
impl LeaseClaims {
    pub fn validate(&self, now: i64) -> Result<()> {
        if self.v != 1
            || self.purpose != "repository"
            || self.audience != "webdock.repository.runtime.v1"
            || !slug(&self.plugin_id)
            || !uuid::Uuid::parse_str(&self.id).is_ok_and(|id| {
                id.to_string() == self.id
                    && id.get_version_num() == 4
                    && id.get_variant() == uuid::Variant::RFC4122
            })
            || !record_id(&self.project_id)
            || !record_id(&self.customer_id)
            || !record_id(&self.source_id)
            || [
                self.generation,
                self.source_revision,
                self.connection_generation,
            ]
            .iter()
            .any(|n| *n == 0 || *n > 9_007_199_254_740_991)
            || self.grant_revision > 9_007_199_254_740_991
            || self.configuration_revision > 9_007_199_254_740_991
            || ![
                &self.descriptor_sha256,
                &self.request_sha256,
                &self.configuration_sha256,
            ]
            .iter()
            .all(|v| valid_digest(v))
            || self.iat > now
            || self.exp <= now
            || !self
                .exp
                .checked_sub(self.iat)
                .is_some_and(|seconds| (1..=60).contains(&seconds))
        {
            return Err(PluginError::denied());
        }
        Ok(())
    }
    pub fn sign(&self, key: &SigningKey) -> Result<String> {
        self.validate(self.iat)?;
        Ok(sign(
            LEASE_DOMAIN,
            &serde_json::to_vec(self).map_err(|_| invalid("Invalid lease"))?,
            key,
        ))
    }
}
pub fn verify_lease_issuer(token: &str, key: &VerifyingKey, now: i64) -> Result<LeaseClaims> {
    let bytes = verify(LEASE_DOMAIN, token, key, 16384)?;
    let claims: LeaseClaims = serde_json::from_slice(&bytes).map_err(|_| PluginError::denied())?;
    claims.validate(now)?;
    Ok(claims)
}
pub fn verify_lease(
    token: &str,
    key: &VerifyingKey,
    plugin: &str,
    generation: u64,
    now: i64,
) -> Result<LeaseClaims> {
    let claims = verify_lease_issuer(token, key, now)?;
    if claims.plugin_id != plugin || claims.generation != generation {
        return Err(PluginError::denied());
    }
    Ok(claims)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    pub v: u8,
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    pub version: String,
    pub manifest_sha256: String,
    pub image: String,
    pub protocol: String,
    pub signer_key_id: String,
}
impl Descriptor {
    pub fn validate(&self) -> Result<()> {
        if self.v != 1
            || !slug(&self.plugin_id)
            || self.version.is_empty()
            || self.version.len() > 40
            || !valid_digest(&self.manifest_sha256)
            || self.protocol != "webdock.repository.v1"
            || !slug(&self.signer_key_id)
            || !self
                .image
                .rsplit_once("@sha256:")
                .is_some_and(|(name, digest)| {
                    !name.is_empty() && name.len() <= 512 && valid_digest(digest)
                })
        {
            return Err(invalid("Invalid repository descriptor"));
        }
        Ok(())
    }
    pub fn sign(&self, key: &SigningKey) -> Result<String> {
        self.validate()?;
        Ok(sign(
            DESCRIPTOR_DOMAIN,
            &serde_json::to_vec(self).map_err(|_| invalid("Invalid descriptor"))?,
            key,
        ))
    }
}
pub fn verify_descriptor(token: &str, key: &VerifyingKey) -> Result<Descriptor> {
    let raw = verify(DESCRIPTOR_DOMAIN, token, key, 16384)?;
    let descriptor: Descriptor = serde_json::from_slice(&raw).map_err(|_| PluginError::denied())?;
    descriptor.validate()?;
    Ok(descriptor)
}
