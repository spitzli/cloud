//! Provider-neutral signed-document bodies. Signing and key custody stay outside the protocol.
use super::RecoveryProfile;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedDocument {
    pub payload: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmittedRecoverySet {
    pub version: u8,
    #[serde(rename = "setID")]
    pub set_id: String,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    #[serde(rename = "policyID")]
    pub policy_id: String,
    pub policy_revision: u64,
    pub profile: RecoveryProfile,
    pub profile_id: String,
    pub approved_by: String,
    pub epoch: String,
    pub capacity_bytes: u64,
    pub proof: serde_json::Value,
    pub completed_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColdRecoveryCapsule {
    pub version: u8,
    pub admitted_set: SignedDocument,
    pub actor_id: String,
    pub target_profile_id: String,
    pub target_identity_sha256: String,
    pub target_nonce: String,
    pub capacity_bytes: u64,
    pub issued_at: i64,
    pub expires_at: i64,
    pub quarantine_only: bool,
}
