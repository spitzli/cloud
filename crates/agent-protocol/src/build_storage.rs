//! Public-verification storage requests. Signers stay in the trusted controller;
//! no request authorizes arbitrary paths, mounts, commands or customer execution.
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const AUDIENCE: &str = "webdock-build-storage-v1";
pub const MIN_VOLUME_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_VOLUME_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const DOMAIN: &[u8] = b"webdock-build-storage-v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildStorageAction {
    Reserve,
    Inspect,
    Retire,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildStorageTarget {
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    pub node: String,
    #[serde(rename = "poolID")]
    pub pool_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildStorageReservation {
    pub version: u8,
    pub audience: String,
    /// Independently allocated Snowflake storage identity; never an application ID.
    #[serde(rename = "volumeID")]
    pub volume_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "buildID")]
    pub build_id: String,
    pub generation: u64,
    pub target: BuildStorageTarget,
    pub volume_bytes: u64,
    pub action: BuildStorageAction,
    pub epoch: u64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedBuildStorageReservation {
    pub reservation: BuildStorageReservation,
    pub signature: String,
}

fn id(value: &str) -> bool {
    !value.starts_with('0')
        && !value.is_empty()
        && value.len() <= 19
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|n| n > 0)
}

impl BuildStorageTarget {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !id(&self.cluster_id)
            || self.node.is_empty()
            || self.node.len() > 253
            || !self.node.as_bytes()[0].is_ascii_alphanumeric()
            || !self
                .node
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
            || !uuid::Uuid::parse_str(&self.pool_id)
                .is_ok_and(|v| !v.is_nil() && v.to_string() == self.pool_id)
        {
            return Err("invalid build storage target");
        }
        Ok(())
    }
}

impl BuildStorageReservation {
    /// Shape validation also supports checking expired durable journal entries.
    pub fn validate_shape(&self) -> Result<(), &'static str> {
        self.target.validate()?;
        if self.version != 1
            || self.audience != AUDIENCE
            || ![
                &self.volume_id,
                &self.customer_id,
                &self.project_id,
                &self.build_id,
            ]
            .into_iter()
            .all(|s| id(s))
            || !(1..=2_147_483_646).contains(&self.generation)
            || !(1..=9_007_199_254_740_991).contains(&self.epoch)
            || !(MIN_VOLUME_BYTES..=MAX_VOLUME_BYTES).contains(&self.volume_bytes)
        {
            return Err("invalid build storage reservation");
        }
        Ok(())
    }

    pub fn validate_at(&self, now: DateTime<Utc>) -> Result<(), &'static str> {
        self.validate_shape()?;
        let remaining = self.expires_at - now;
        if remaining <= chrono::Duration::zero() || remaining > chrono::Duration::seconds(300) {
            return Err("expired or overlong build storage authority");
        }
        Ok(())
    }

    fn signing_bytes(&self) -> Result<Vec<u8>, &'static str> {
        self.validate_shape()?;
        let mut bytes = DOMAIN.to_vec();
        bytes.extend(serde_json::to_vec(self).map_err(|_| "build storage encoding")?);
        Ok(bytes)
    }

    pub fn fingerprint(&self) -> Result<String, &'static str> {
        Ok(hex::encode(Sha256::digest(self.signing_bytes()?)))
    }

    /// Stable allocation identity, excluding the action/epoch/expiry of a renewal.
    pub fn allocation_fingerprint(&self) -> Result<String, &'static str> {
        self.validate_shape()?;
        let mut allocation = self.clone();
        allocation.action = BuildStorageAction::Reserve;
        allocation.epoch = 1;
        allocation.expires_at = DateTime::UNIX_EPOCH;
        allocation.fingerprint()
    }
    pub fn job_name(&self) -> Result<String, &'static str> {
        self.validate_shape()?;
        Ok(format!(
            "wd-build-{}-{}",
            &hex::encode(Sha256::digest(self.build_id.as_bytes()))[..40],
            self.generation
        ))
    }
    pub fn queue_name(&self) -> Result<String, &'static str> {
        Ok(format!(
            "wd-vq-{}-{}-{}",
            self.volume_id,
            self.epoch,
            &self.fingerprint()?[..16]
        ))
    }
}

impl SignedBuildStorageReservation {
    pub fn sign(
        reservation: BuildStorageReservation,
        key: &SigningKey,
    ) -> Result<Self, &'static str> {
        reservation.validate_at(Utc::now())?;
        let signature = hex::encode(key.sign(&reservation.signing_bytes()?).to_bytes());
        Ok(Self {
            reservation,
            signature,
        })
    }

    pub fn verify(
        &self,
        key: &VerifyingKey,
        target: &BuildStorageTarget,
        now: DateTime<Utc>,
    ) -> Result<(), &'static str> {
        self.reservation.validate_at(now)?;
        target.validate()?;
        if &self.reservation.target != target || self.signature.len() != 128 {
            return Err("foreign build storage authority");
        }
        let bytes = hex::decode(&self.signature).map_err(|_| "invalid build storage signature")?;
        let signature =
            Signature::from_slice(&bytes).map_err(|_| "invalid build storage signature")?;
        key.verify_strict(&self.reservation.signing_bytes()?, &signature)
            .map_err(|_| "invalid build storage signature")
    }
}

/// Infrastructure receipt, never a Kubernetes credential or a customer-selected PVC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildVolumeHandle {
    pub version: u8,
    pub reservation: BuildStorageReservation,
    pub namespace: String,
    pub pv_name: String,
    pub pv_uid: String,
    pub pvc_name: String,
    pub pvc_uid: String,
    pub filesystem_uuid: String,
}

fn resource_uid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}
fn namespace(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
impl BuildVolumeHandle {
    pub fn resource_name(request: &BuildStorageReservation) -> Result<String, &'static str> {
        request.validate_shape()?;
        Ok(format!(
            "wd-bv-{}-{}-{}",
            request.build_id, request.generation, request.volume_id
        ))
    }
    pub fn new(
        reservation: BuildStorageReservation,
        namespace: &str,
        pv_uid: &str,
        pvc_uid: &str,
        filesystem_uuid: &str,
    ) -> Result<Self, &'static str> {
        let name = Self::resource_name(&reservation)?;
        let handle = Self {
            version: 1,
            reservation,
            namespace: namespace.into(),
            pv_name: name.clone(),
            pv_uid: pv_uid.into(),
            pvc_name: name,
            pvc_uid: pvc_uid.into(),
            filesystem_uuid: filesystem_uuid.into(),
        };
        handle.validate_for(&handle.reservation)?;
        Ok(handle)
    }
    pub fn validate_for(&self, request: &BuildStorageReservation) -> Result<(), &'static str> {
        let name = Self::resource_name(request)?;
        if self.version != 1
            || !namespace(&self.namespace)
            || self.reservation.allocation_fingerprint()? != request.allocation_fingerprint()?
            || self.pv_name != name
            || self.pvc_name != name
            || !resource_uid(&self.pv_uid)
            || !resource_uid(&self.pvc_uid)
            || !uuid::Uuid::parse_str(&self.filesystem_uuid)
                .is_ok_and(|v| !v.is_nil() && v.to_string() == self.filesystem_uuid)
        {
            return Err("invalid build volume binding");
        }
        Ok(())
    }
    pub fn volume_name(&self) -> &str {
        &self.pv_name
    }
    pub fn claim_name(&self) -> &str {
        &self.pvc_name
    }
    pub fn job_name(&self) -> String {
        format!(
            "wd-build-{}-{}",
            &hex::encode(Sha256::digest(self.reservation.build_id.as_bytes()))[..40],
            self.reservation.generation
        )
    }
    pub fn fingerprint(&self) -> Result<String, &'static str> {
        self.validate_for(&self.reservation)?;
        let bytes = serde_json::to_vec(&(
            1,
            self.reservation.allocation_fingerprint()?,
            &self.namespace,
            &self.pv_name,
            &self.pv_uid,
            &self.pvc_name,
            &self.pvc_uid,
            &self.filesystem_uuid,
        ))
        .map_err(|_| "build volume encoding")?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildVolumePhase {
    Pending,
    Ready,
    Retiring,
    Retired,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildVolumeResult {
    pub phase: BuildVolumePhase,
    pub handle: Option<BuildVolumeHandle>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BuildVolumeFailure {
    InvalidAuthority,
    Expired,
    Fenced,
    StorageConflict,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildVolumeReply {
    pub version: u8,
    pub request_uid: String,
    pub request_fingerprint: String,
    pub result: Option<BuildVolumeResult>,
    pub failure_code: Option<BuildVolumeFailure>,
}
impl BuildVolumeReply {
    pub fn validate_for(
        &self,
        request: &BuildStorageReservation,
        request_uid: &str,
        namespace: &str,
    ) -> Result<(), &'static str> {
        request.validate_shape()?;
        if self.version != 1
            || !resource_uid(request_uid)
            || self.request_uid != request_uid
            || self.request_fingerprint != request.fingerprint()?
            || self.result.is_some() == self.failure_code.is_some()
        {
            return Err("foreign build volume reply");
        }
        if let Some(result) = &self.result {
            let allowed = match result.phase {
                BuildVolumePhase::Ready | BuildVolumePhase::Pending => {
                    request.action != BuildStorageAction::Retire
                }
                BuildVolumePhase::Retired => request.action == BuildStorageAction::Retire,
                BuildVolumePhase::Retiring => request.action != BuildStorageAction::Reserve,
            };
            if !allowed || (result.phase == BuildVolumePhase::Ready && result.handle.is_none()) {
                return Err("invalid build volume outcome");
            }
            if let Some(handle) = &result.handle {
                handle.validate_for(request)?;
                if handle.namespace != namespace {
                    return Err("foreign build volume namespace");
                }
            }
        }
        Ok(())
    }
}
