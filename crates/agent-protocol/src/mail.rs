//! Fixed incoming Mail lifecycle envelopes. Material is ephemeral and must not be journaled.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy)]
pub struct InvalidMail;
type Result<T> = std::result::Result<T, InvalidMail>;
fn id(value: &str) -> bool {
    !value.starts_with('0')
        && !value.is_empty()
        && value.len() <= 19
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<i64>().is_ok()
}
fn token(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|uuid| uuid.to_string() == value)
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn secret(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
}
pub fn fingerprint(value: &impl Serialize) -> Result<String> {
    Ok(hex::encode(Sha256::digest(
        serde_json::to_vec(value).map_err(|_| InvalidMail)?,
    )))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailAction {
    Ensure,
    Suspend,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailResources {
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
    pub volume_bytes: u64,
    pub ephemeral_bytes: u64,
}
impl MailResources {
    pub const FIXED: Self = Self {
        cpu_millicores: 500,
        memory_bytes: 1073741824,
        volume_bytes: 10737418240,
        ephemeral_bytes: 268435456,
    };
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailSpec {
    pub version: u8,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    pub revision: i64,
    pub agent_generation: i32,
    pub instance_key: String,
    pub hostname: String,
    pub image: String,
    pub resources: MailResources,
    pub material_sha256: String,
    pub action: MailAction,
}
impl MailSpec {
    pub fn validate(&self) -> Result<()> {
        let hostname_prefix = format!("mail-{}.", self.customer_id);
        let domain = self
            .hostname
            .strip_prefix(&hostname_prefix)
            .ok_or(InvalidMail)?;
        let labels = domain.split('.').collect::<Vec<_>>();
        let domain_valid = labels.len() >= 2
            && labels.iter().all(|part| {
                !part.is_empty()
                    && part.len() <= 63
                    && !part.starts_with('-')
                    && !part.ends_with('-')
                    && part.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            });
        let image = self
            .image
            .split_once("@sha256:")
            .is_some_and(|(name, digest)| {
                name.contains('/')
                    && !name.starts_with('/')
                    && name.len() <= 440
                    && hash(digest)
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-/:".contains(&byte))
            });
        if self.version != 1
            || ![&self.operation_id, &self.customer_id, &self.cluster_id]
                .into_iter()
                .all(|v| id(v))
            || !(1..=i64::from(i32::MAX)).contains(&self.revision)
            || self.agent_generation < 1
            || self.instance_key != format!("mail-{}", self.customer_id)
            || self.hostname.len() > 253
            || !domain_valid
            || !image
            || self.resources != MailResources::FIXED
            || !hash(&self.material_sha256)
        {
            return Err(InvalidMail);
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum MailMaterial {
    Unallocated {},
    Credentials {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bootstrap_password: Option<String>,
        username: String,
        password: String,
    },
}
impl MailMaterial {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Unallocated {} => Ok(()),
            Self::Credentials {
                bootstrap_password,
                username,
                password,
            } if secret(username)
                && secret(password)
                && !username.contains(':')
                && bootstrap_password.as_deref().is_none_or(secret) =>
            {
                Ok(())
            }
            Self::Credentials { .. } => Err(InvalidMail),
        }
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailLease {
    pub lease_token: String,
}
impl MailLease {
    pub fn validate(&self) -> Result<()> {
        if !token(&self.lease_token) {
            return Err(InvalidMail);
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct MailClaim {
    pub spec: MailSpec,
    pub lease: MailLease,
    pub lease_until: DateTime<Utc>,
    pub material: MailMaterial,
    pub checkpoint: Option<MailCheckpoint>,
}
impl MailClaim {
    pub fn validate(&self, now: DateTime<Utc>) -> Result<()> {
        self.spec.validate()?;
        self.lease.validate()?;
        self.material.validate()?;
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.validate()?;
            if checkpoint.material_sha256 != self.spec.material_sha256
                || checkpoint.lease.lease_token != self.lease.lease_token
                || self.spec.action != MailAction::Ensure
            {
                return Err(InvalidMail);
            }
        }
        if self.lease_until <= now
            || (self.spec.action == MailAction::Ensure
                && matches!(self.material, MailMaterial::Unallocated {}))
            || self.material.fingerprint()? != self.spec.material_sha256
        {
            return Err(InvalidMail);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailCheckpoint {
    pub lease: MailLease,
    #[serde(rename = "runtimeID")]
    pub runtime_id: String,
    pub material_sha256: String,
}
fn runtime_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}
impl MailCheckpoint {
    pub fn validate(&self) -> Result<()> {
        self.lease.validate()?;
        if !runtime_id(&self.runtime_id) || !hash(&self.material_sha256) {
            return Err(InvalidMail);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailOutcome {
    Succeeded,
    NeedsReview,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailEdition {
    Community,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailProof {
    #[serde(rename = "runtimeID")]
    pub runtime_id: String,
    pub namespace: String,
    pub revision: i64,
    pub running: bool,
    pub edition: MailEdition,
    pub recovery_disabled: bool,
    pub submission_certificate_sha256: Option<String>,
    /// Used only by the explicitly enabled owned local Docker integration runtime.
    pub private_ipv4: Option<std::net::Ipv4Addr>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailCompletion {
    pub lease: MailLease,
    pub outcome: MailOutcome,
    pub proof: Option<MailProof>,
}
impl MailCompletion {
    pub fn validate(&self) -> Result<()> {
        self.lease.validate()?;
        match (self.outcome, &self.proof) {
            (MailOutcome::NeedsReview, None) => Ok(()),
            (MailOutcome::Succeeded, Some(proof))
                if runtime_id(&proof.runtime_id)
                    && proof.recovery_disabled
                    && proof.revision > 0
                    && proof.namespace.len() <= 63
                    && proof.namespace.starts_with("wd-mail-")
                    && (!proof.running || proof.submission_certificate_sha256.is_some())
                    && proof
                        .submission_certificate_sha256
                        .as_deref()
                        .is_none_or(hash)
                    && proof.private_ipv4.is_none_or(|ip| ip.is_private()) =>
            {
                Ok(())
            }
            _ => Err(InvalidMail),
        }
    }
}
