//! Custody of an issued stale receipt is not authorization to apply its claimed outcome.
use super::*;
use subtle::ConstantTimeEq;
use webdock_supervisor::{Command, ReadCommand, RuntimeSpec};

/// Trusted transport integration: Core first loads this exact persisted dispatch for the
/// current enrollment and structurally validates the matching receipt, before calling here.
#[derive(Clone, Copy)]
pub enum DeliveryCommand<'a> {
    Mutation(&'a Command),
    Read(&'a ReadCommand),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeliveryDisposition {
    Live,
    Retired,
    Completed,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeliveryRecord {
    agent_generation: i32,
    operation_generation: u64,
    request_sha256: String,
    receipt_sha256: String,
    lease_until: u64,
    disposition: RetiredDisposition,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RetiredDisposition {
    Accepted,
    NeedsReconciliation,
    Obsolete,
}
fn hash_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn stored_deliveries(op: &Value, field: &str) -> Result<Vec<DeliveryRecord>> {
    let desired = op["desired"]
        .as_object()
        .ok_or_else(|| error(503, "Stored runtime dispatch is invalid."))?;
    let records: Vec<DeliveryRecord> = match desired.get(field) {
        None => vec![],
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_| error(503, "Stored runtime receipt custody is invalid."))?,
    };
    if records.len() > 256
        || records.iter().any(|record| {
            (matches!(record.disposition, RetiredDisposition::Accepted)
                != (field == "nativeHostingAcceptedDeliveries"))
                || record.agent_generation <= 0
                || record.operation_generation == 0
                || record.lease_until == 0
                || !hash_valid(&record.request_sha256)
                || !hash_valid(&record.receipt_sha256)
        })
    {
        return Err(error(503, "Stored runtime receipt custody is invalid."));
    }
    Ok(records)
}
struct Envelope<'a> {
    id: &'a str,
    generation: u64,
    lease_until: u64,
    spec: &'a RuntimeSpec,
    authorization: &'a str,
    expected_runtime_id: Option<&'a str>,
    fingerprint: String,
    read: bool,
}
impl DeliveryCommand<'_> {
    fn envelope(&self) -> Result<Envelope<'_>> {
        let invalid = |_| error(400, "Invalid issued runtime command.");
        match self {
            Self::Mutation(command) => {
                command.validate().map_err(invalid)?;
                Ok(Envelope {
                    id: &command.operation_id,
                    generation: command.generation,
                    lease_until: command.lease_until,
                    spec: &command.spec,
                    authorization: &command.authorization,
                    expected_runtime_id: command.expected_runtime_id.as_deref(),
                    fingerprint: command.fingerprint().map_err(invalid)?,
                    read: false,
                })
            }
            Self::Read(command) => {
                command.validate().map_err(invalid)?;
                Ok(Envelope {
                    id: &command.operation_id,
                    generation: command.generation,
                    lease_until: command.lease_until,
                    spec: &command.spec,
                    authorization: &command.authorization,
                    expected_runtime_id: command.expected_runtime_id.as_deref(),
                    fingerprint: command.fingerprint().map_err(invalid)?,
                    read: true,
                })
            }
        }
    }
}
impl HostingService {
    /// Persist exact transport receipt identity in the same transaction as a native outcome.
    /// Older direct-domain callers keep their existing semantic receipt comparison.
    pub(super) async fn record_accepted_delivery(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        command: DeliveryCommand<'_>,
        receipt_sha256: Option<&str>,
    ) -> Result<()> {
        let Some(receipt_sha256) = receipt_sha256 else {
            return Ok(());
        };
        if !hash_valid(receipt_sha256) {
            return Err(error(400, "Invalid runtime receipt hash."));
        }
        let envelope = command.envelope()?;
        let op = Self::operation(tx, agent, envelope.id, true).await?;
        let mut records = stored_deliveries(&op, "nativeHostingAcceptedDeliveries")?;
        if let Some(record) = records.iter().find(|record| {
            record.agent_generation == agent.generation
                && record.operation_generation == envelope.generation
        }) {
            if record.request_sha256 != envelope.fingerprint
                || record.receipt_sha256 != receipt_sha256
                || record.lease_until != envelope.lease_until
            {
                return Err(error(409, "Accepted runtime receipt changed."));
            }
            return Ok(());
        }
        if records.len() + stored_deliveries(&op, "nativeHostingRetiredDeliveries")?.len() >= 256 {
            return Err(error(
                409,
                "Runtime receipt custody requires operator review.",
            ));
        }
        records.push(DeliveryRecord {
            agent_generation: agent.generation,
            operation_generation: envelope.generation,
            request_sha256: envelope.fingerprint,
            receipt_sha256: receipt_sha256.into(),
            lease_until: envelope.lease_until,
            disposition: RetiredDisposition::Accepted,
        });
        execute(tx,"UPDATE webdock_auth.hosting_operation SET desired=jsonb_set(desired,'{nativeHostingAcceptedDeliveries}',$2) WHERE id=$1",vec![envelope.id.into(),json!(records).into()]).await?;
        Ok(())
    }
    /// Records only receipt custody and, when still bound to the same running revision,
    /// the need for operator review. No observed state, lease or allocation is settled.
    pub async fn retire_stale_delivery(
        &self,
        agent: &AgentIdentity,
        command: DeliveryCommand<'_>,
        receipt_sha256: &str,
    ) -> Result<DeliveryDisposition> {
        if !hash_valid(receipt_sha256) {
            return Err(error(400, "Invalid runtime receipt hash."));
        }
        let envelope = command.envelope()?;
        let expected = match command {
            DeliveryCommand::Mutation(value) => self.runtime_mac(agent, value)?,
            DeliveryCommand::Read(value) => self.read_mac(agent, value)?,
        };
        if !bool::from(expected.as_bytes().ct_eq(envelope.authorization.as_bytes())) {
            return Err(error(
                403,
                "Issued runtime authority does not match enrollment.",
            ));
        }
        let tx = self.agent_tx(agent).await?;
        let op = Self::operation(&tx, agent, envelope.id, true).await?;
        if op["project_id"] != envelope.spec.project_id.to_string()
            || op["app_id"] != envelope.spec.resource_id.to_string()
            || numeric(&op, "generation")?
                < i64::try_from(envelope.generation)
                    .map_err(|_| error(400, "Invalid operation generation."))?
            || is_read_action(field(&op, "action")?) != envelope.read
        {
            return Err(error(409, "Issued runtime operation changed."));
        }
        let accepted = stored_deliveries(&op, "nativeHostingAcceptedDeliveries")?;
        if let Some(record) = accepted.iter().find(|record| {
            record.agent_generation == agent.generation
                && record.operation_generation == envelope.generation
        }) {
            if record.request_sha256 != envelope.fingerprint
                || record.receipt_sha256 != receipt_sha256
                || record.lease_until != envelope.lease_until
            {
                return Err(error(409, "Accepted runtime receipt changed."));
            }
            tx.commit().await.map_err(db_error)?;
            return Ok(DeliveryDisposition::Completed);
        }
        let mut records = stored_deliveries(&op, "nativeHostingRetiredDeliveries")?;
        if let Some(record) = records.iter().find(|record| {
            record.agent_generation == agent.generation
                && record.operation_generation == envelope.generation
        }) {
            if record.request_sha256 != envelope.fingerprint
                || record.receipt_sha256 != receipt_sha256
                || record.lease_until != envelope.lease_until
            {
                return Err(error(409, "Retired runtime receipt changed."));
            }
            tx.commit().await.map_err(db_error)?;
            return Ok(DeliveryDisposition::Retired);
        }
        if op["generation"] == envelope.generation {
            // A prior native domain result remains authoritative if its transport ACK was lost.
            // Delegate its comparison to the strict completion path; do not relabel it stale.
            let known = [
                "nativeReadReceipt",
                "runtimeReceipt",
                "nativeRuntimeReceipt",
            ]
            .iter()
            .any(|key| {
                op["result"]
                    .get(*key)
                    .is_some_and(|receipt| receipt["requestSha256"] == envelope.fingerprint)
            });
            if known
                && matches!(
                    op["status"].as_str(),
                    Some("succeeded" | "failed" | "needs-reconciliation")
                )
            {
                tx.commit().await.map_err(db_error)?;
                return Ok(DeliveryDisposition::Live);
            }
            if op["status"] == "succeeded" {
                return Err(error(
                    409,
                    "Successful runtime receipt provenance is unavailable.",
                ));
            }
        }
        let app = Self::app(&tx, field(&op, "app_id")?).await?;
        let generation_current = op["generation"] == envelope.generation;
        let revision_current = app["revision"] == op["target_revision"]
            && (envelope.read || op["target_revision"] == envelope.spec.revision);
        let binding_current = if envelope.read {
            app["observed_uid"].as_str() == envelope.expected_runtime_id
        } else {
            app["operation_id"] == envelope.id
        };
        let lease_current = op["status"] == "running"
            && epoch(&op["lease_until"]).is_some_and(|lease| {
                lease > now() && u64::try_from(lease).ok() == Some(envelope.lease_until)
            });
        let current = generation_current && revision_current && binding_current;
        // Known obsolete leases/generations need no renewed execution permission. If current,
        // distinguish explicit denial from persistence/configuration errors; never swallow outages.
        if current && lease_current && self.accepted_authority(&tx, &op).await? {
            tx.commit().await.map_err(db_error)?;
            return Ok(DeliveryDisposition::Live);
        }
        if records.len() + accepted.len() >= 256 {
            return Err(error(
                409,
                "Runtime receipt custody requires operator review.",
            ));
        }
        let review = generation_current
            && revision_current
            && (envelope.read || binding_current)
            && op["status"] == "running";
        records.push(DeliveryRecord {
            agent_generation: agent.generation,
            operation_generation: envelope.generation,
            request_sha256: envelope.fingerprint,
            receipt_sha256: receipt_sha256.into(),
            lease_until: envelope.lease_until,
            disposition: if review {
                RetiredDisposition::NeedsReconciliation
            } else {
                RetiredDisposition::Obsolete
            },
        });
        execute(&tx,"UPDATE webdock_auth.hosting_operation SET desired=jsonb_set(desired,'{nativeHostingRetiredDeliveries}',$2),status=CASE WHEN $3 THEN $4 ELSE status END WHERE id=$1",vec![envelope.id.into(),json!(records).into(),review.into(),if envelope.read{"failed"}else{"needs-reconciliation"}.into()]).await?;
        Self::verify_agent(&tx, agent).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(DeliveryDisposition::Retired)
    }
}
