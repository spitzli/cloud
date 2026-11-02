//! Two explicit recovery profiles, carried only on the enrolled native agent channel.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub mod custody;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryProfile {
    MailInstallationV1,
    NtfySqliteOfflineV1,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecoveryAction {
    Backup,
    Restore,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoverySpec {
    pub version: u8,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    #[serde(rename = "policyID")]
    pub policy_id: String,
    pub policy_revision: u64,
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    pub profile: RecoveryProfile,
    pub action: RecoveryAction,
    pub source: serde_json::Value,
    pub target: Option<serde_json::Value>,
    #[serde(rename = "recoverySetID")]
    pub recovery_set_id: Option<String>,
    pub epoch: String,
    pub capacity_bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryLease {
    pub generation: u64,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}
impl std::fmt::Debug for RecoveryLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryLease([private])")
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryClaim {
    pub spec: RecoverySpec,
    pub lease: RecoveryLease,
    pub checkpoint: Option<serde_json::Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryCompletion {
    pub generation: u64,
    pub token: String,
    pub state: String,
    pub proof: serde_json::Value,
    pub error_code: Option<String>,
}
fn id(s: &str) -> bool {
    !s.starts_with('0')
        && s.len() <= 19
        && s.bytes().all(|c| c.is_ascii_digit())
        && s.parse::<i64>().is_ok_and(|n| n > 0)
}
fn uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s && !u.is_nil())
}
impl RecoverySpec {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1
            || ![&self.operation_id, &self.policy_id, &self.cluster_id]
                .into_iter()
                .all(|s| id(s))
            || !(1..=2147483646).contains(&self.policy_revision)
            || !uuid(&self.epoch)
            || !(67108864..=64 * 1024 * 1024 * 1024).contains(&self.capacity_bytes)
            || !self.source.is_object()
            || self.source.to_string().len() > 32768
            || self
                .target
                .as_ref()
                .is_some_and(|v| !v.is_object() || v.to_string().len() > 32768)
            || (self.action == RecoveryAction::Restore)
                != (self.target.is_some() && self.recovery_set_id.as_deref().is_some_and(id))
            || (self.action == RecoveryAction::Backup
                && (self.target.is_some() || self.recovery_set_id.is_some()))
        {
            return Err("invalid recovery operation");
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String, &'static str> {
        self.validate()?;
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|_| "recovery encoding")?)
        ))
    }
}
impl RecoveryClaim {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.spec.validate()?;
        let remaining = self.lease.expires_at - Utc::now();
        self.lease.validate_shape()?;
        if remaining <= chrono::Duration::zero() || remaining > chrono::Duration::seconds(300) {
            return Err("invalid recovery lease");
        }
        Ok(())
    }
}
impl RecoveryLease {
    pub fn validate_shape(&self) -> Result<(), &'static str> {
        if !(1..=2147483646).contains(&self.generation) || !uuid(&self.token) {
            return Err("invalid recovery lease identity");
        }
        Ok(())
    }
}
