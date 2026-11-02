//! Pure reviewed-intent data, intentionally not linked to service admission yet.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use webdock_agent_protocol::database::DatabaseBackupPolicySpec;
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreSource {
    #[serde(rename = "databaseID")]
    pub database_id: String,
    pub revision: i32,
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    pub policy: DatabaseBackupPolicySpec,
    #[serde(rename = "restorePointID")]
    pub restore_point_id: String,
    #[serde(rename = "baseBackupID")]
    pub base_backup_id: String,
    pub completed_at: DateTime<Utc>,
    pub credential_revision: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreTarget {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    #[serde(rename = "planRevisionID")]
    pub plan_revision_id: String,
    pub name: String,
    pub plan: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreIntent {
    pub version: u8,
    pub purpose: String,
    #[serde(rename = "offerID")]
    pub offer_id: String,
    pub expires_at: DateTime<Utc>,
    pub source: RestoreSource,
    pub target: RestoreTarget,
    pub target_time: DateTime<Utc>,
    pub timeline: u32,
    #[serde(rename = "verificationSHA256")]
    pub verification_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewedPrice {
    pub monthly_price_cents: u64,
    pub currency: String,
    pub billing: String,
    pub billing_starts: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreAcceptance {
    pub action: String,
    #[serde(rename = "offerID")]
    pub offer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    pub intent_hash: String,
    pub confirm_price: bool,
    pub reviewed_price: ReviewedPrice,
    pub idempotency_key: String,
}
impl RestoreIntent {
    pub fn parse(value: Value, now: DateTime<Utc>) -> Result<Self> {
        let intent = Self::decode(value)?;
        intent.validate(now)?;
        Ok(intent)
    }
    /// Decode immutable storage for review/replay; this does not admit new work.
    pub fn decode(value: Value) -> Result<Self> {
        if serde_json::to_vec(&value)
            .map_err(|_| "Invalid restore intent")?
            .len()
            > 65536
        {
            return Err("Restore intent is too large");
        }
        serde_json::from_value(value).map_err(|_| "Invalid restore intent")
    }
    fn validate(&self, now: DateTime<Utc>) -> Result<()> {
        let policy = &self.source.policy;
        policy
            .validate()
            .map_err(|_| "Invalid source policy identity")?;
        if self.version != 1 || self.purpose != "restore" || self.expires_at <= now {
            return Err("Current restore-purpose review required");
        }
        if [
            &self.offer_id,
            &self.source.database_id,
            &self.source.cluster_id,
            &self.source.restore_point_id,
            &self.target.customer_id,
            &self.target.project_id,
            &self.target.cluster_id,
            &self.target.plan_revision_id,
        ]
        .iter()
        .any(|id| !valid_id(id))
            || self.source.revision < 1
            || self.source.credential_revision < 1
            || self.timeline == 0
            || !sha256(&self.verification_sha256)
            || self.source.base_backup_id.len() != 15
            || self
                .source
                .base_backup_id
                .bytes()
                .enumerate()
                .any(|(i, b)| {
                    if i == 8 {
                        b != b'T'
                    } else {
                        !b.is_ascii_digit()
                    }
                })
        {
            return Err("Exact source/base/time/credential/hash selection required");
        }
        if self.source.completed_at > self.target_time || self.target_time > now {
            return Err("Recovery time must follow the completed base and not be future");
        }
        if policy.customer_id != self.target.customer_id
            || policy.project_id != self.target.project_id
            || self.source.cluster_id != self.target.cluster_id
        {
            return Err("Recovery requires the same customer/project/cluster");
        }
        if self.target.name.is_empty()
            || self.target.name.len() > 160
            || self.target.name.trim() != self.target.name
        {
            return Err("Exact target name required");
        }
        crate::ManagedCommand::parse(serde_json::json!({"action":"databases.plans.create","plan":self.target.plan,"idempotencyKey":"restore-plan-validation-001"})).map_err(|_| "Invalid reviewed target plan")?;
        if self.target.plan["kind"] != "dedicated"
            || self
                .target
                .plan
                .get("sponsoredCustomerID")
                .is_some_and(|id| id != &self.target.customer_id)
        {
            return Err("A matching Dedicated target plan is required");
        }
        Ok(())
    }
    pub fn hash(&self) -> Result<String> {
        let value = serde_json::to_value(self).map_err(|_| "Invalid restore intent")?;
        let canonical = webdock_hosting::canonical_command(&value)
            .map_err(|_| "Invalid canonical restore intent")?;
        Ok(hex::encode(Sha256::digest(canonical.as_bytes())))
    }
    /// First acceptance only: a caller must authorize and replay any existing order
    /// before enforcing mutable/expiry checks. This helper neither allocates nor rebinds authority.
    pub fn validate_acceptance(
        &self,
        accept: &RestoreAcceptance,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.validate(now)?;
        self.validate_reviewed_acceptance(accept)
    }
    /// Exact immutable review only; caller must replay an existing order or also
    /// call validate_acceptance before allocating new work.
    pub fn validate_reviewed_acceptance(&self, accept: &RestoreAcceptance) -> Result<()> {
        if self.version != 1
            || self.purpose != "restore"
            || accept.offer_id != self.offer_id
            || accept.project_id != self.target.project_id
            || accept.intent_hash != self.hash()?
            || !accept.confirm_price
        {
            return Err("Exact reviewed restore offer and consent required");
        }
        let price = &accept.reviewed_price;
        let billing = if self.target.plan.get("sponsoredCustomerID").is_some() {
            "sponsored"
        } else {
            "monthly-net-fixed"
        };
        if Some(price.monthly_price_cents) != self.target.plan["monthlyPriceCents"].as_u64()
            || price.currency != self.target.plan["currency"]
            || price.billing != billing
            || price.billing_starts != "observed-ready"
        {
            return Err("Exact full reviewed price required");
        }
        Ok(())
    }
}
impl RestoreAcceptance {
    pub fn parse(value: Value) -> Result<Self> {
        let accept: Self =
            serde_json::from_value(value).map_err(|_| "Invalid restore acceptance")?;
        if accept.action != "databases.restoreOrder.accept"
            || !valid_id(&accept.offer_id)
            || !valid_id(&accept.project_id)
            || !sha256(&accept.intent_hash)
            || !accept.confirm_price
            || !idempotency_key(&accept.idempotency_key)
            || accept.reviewed_price.monthly_price_cents > 1000000
            || accept.reviewed_price.currency != "EUR"
            || !["sponsored", "monthly-net-fixed"].contains(&accept.reviewed_price.billing.as_str())
            || accept.reviewed_price.billing_starts != "observed-ready"
        {
            return Err("Exact restore acceptance and price confirmation required");
        }
        Ok(accept)
    }
}
pub fn require_ordinary_offer(snapshot: &Value) -> Result<()> {
    if !snapshot.is_object() || snapshot.get("purpose").is_some() {
        return Err("Ordinary acceptance cannot consume a purpose-specific offer");
    }
    Ok(())
}
fn valid_id(value: &str) -> bool {
    !value.starts_with('0')
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|id| id > 0)
}
fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn idempotency_key(value: &str) -> bool {
    (16..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
