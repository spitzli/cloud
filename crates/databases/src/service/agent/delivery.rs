//! Receipt custody is separate from permission to perform another runtime mutation.
use super::*;
struct Delivery<'a> {
    spec: &'a DatabaseJobSpec,
    lease: &'a DatabaseLease,
    slot: &'static str,
    hash: String,
}
impl Delivery<'_> {
    fn current(&self, op: &Value) -> bool {
        op["generation"] == self.spec.operation_generation
            && op["database_revision"] == self.spec.revision
            && op["backend_id"] == self.spec.backend.id
            && op["lease_token_hash"] == digest(&self.lease.lease_token)
    }
    fn recorded(&self, op: &Value, agent: &AgentIdentity) -> Result<bool> {
        let Some(record) = op["desired"]["nativeDatabaseRetiredDeliveries"]
            .as_array()
            .and_then(|rows| {
                rows.iter().find(|r| {
                    r["slot"] == self.slot
                        && r["tokenHash"] == digest(&self.lease.lease_token)
                        && r["agentGeneration"] == agent.generation()
                        && r["operationGeneration"] == self.spec.operation_generation
                })
            })
        else {
            return Ok(false);
        };
        if record["requestHash"] != self.hash || record["fingerprint"] != self.spec.fingerprint()? {
            return Err(lease_conflict());
        }
        Ok(true)
    }
}
impl DatabaseService {
    /// A known obsolete callback is durably acknowledged without installing its checkpoint.
    pub async fn receive_database_checkpoint(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &DatabaseJobSpec,
        data: DatabaseCheckpoint,
    ) -> Result<()> {
        data.validate()?;
        let op = self
            .issued_database_callback(&tx, agent, spec, &data.lease)
            .await?;
        if !checkpoint_phase_allowed(spec.phase, data.phase) {
            return Err(lease_conflict());
        }
        let delivery = Delivery {
            spec,
            lease: &data.lease,
            slot: match data.phase {
                DatabaseCheckpointPhase::Applied => "checkpoint-applied",
                DatabaseCheckpointPhase::Configuring => "checkpoint-configuring",
                DatabaseCheckpointPhase::Observing => "checkpoint-observing",
            },
            hash: checkpoint_digest(&data)?,
        };
        if delivery.recorded(&op, agent)? {
            tx.commit().await?;
            return Ok(());
        }
        if checkpoint_recorded(&op, agent, &data)? {
            return self.checkpoint_database_operation(tx, agent, data).await;
        }
        let rank = |phase: &str| match phase {
            "applied" => 1,
            "configuring" => 2,
            "observing" => 3,
            _ => 0,
        };
        let incoming = match data.phase {
            DatabaseCheckpointPhase::Applied => 1,
            DatabaseCheckpointPhase::Configuring => 2,
            DatabaseCheckpointPhase::Observing => 3,
        };
        if delivery.current(&op)
            && rank(op["checkpoint"]["phase"].as_str().unwrap_or("")) >= incoming
        {
            return Err(lease_conflict());
        }
        if !self
            .database_delivery_live(&tx, agent, &op, &delivery)
            .await
        {
            self.retire_database_delivery(&tx, agent, &op, &delivery)
                .await?;
            tx.commit().await?;
            return Ok(());
        }
        self.validate_database_checkpoint_in(&tx, agent, spec, &data)
            .await?;
        self.checkpoint_database_operation(tx, agent, data).await
    }
    /// Trust the issued envelope for failure/stale delivery, never for renewed execution or success.
    pub async fn receive_database_completion(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &DatabaseJobSpec,
        data: DatabaseCompletion,
    ) -> Result<Value> {
        data.validate()?;
        let op = self
            .issued_database_callback(&tx, agent, spec, &data.lease)
            .await?;
        if data.outcome == DatabaseCompletionOutcome::Succeeded
            && spec.phase != DatabaseExecutionPhase::Observe
        {
            return Err(lease_conflict());
        }
        let delivery = Delivery {
            spec,
            lease: &data.lease,
            slot: "completion",
            hash: completion_digest(&data)?,
        };
        if delivery.recorded(&op, agent)? {
            tx.commit().await?;
            return Ok(json!({"accepted":true}));
        }
        if let Some(record) = op["desired"]["nativeDatabaseCompletions"]
            .as_array()
            .and_then(|rows| {
                rows.iter().find(|r| {
                    r["tokenHash"] == digest(&data.lease.lease_token)
                        && r["agentGeneration"] == agent.generation()
                })
            })
        {
            if record["requestHash"] != delivery.hash {
                return Err(lease_conflict());
            }
            return self.complete_database_operation(tx, agent, data).await;
        }
        if op["status"] == "succeeded" && delivery.current(&op) {
            return self.complete_database_operation(tx, agent, data).await;
        }
        if data.outcome == DatabaseCompletionOutcome::NeedsReconciliation
            || !self
                .database_delivery_live(&tx, agent, &op, &delivery)
                .await
        {
            self.retire_database_delivery(&tx, agent, &op, &delivery)
                .await?;
            tx.commit().await?;
            return Ok(json!({"accepted":true}));
        }
        self.complete_database_operation(tx, agent, data).await
    }
    async fn database_delivery_live(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        op: &Value,
        delivery: &Delivery<'_>,
    ) -> bool {
        if !delivery.current(op) || op["status"] != "running" {
            return false;
        }
        // Any unavailable authority makes the outcome uncertain. A failed DB transaction still
        // cannot acknowledge: the classification write and commit below must both succeed.
        let authority = async {
            self.validate_database_callback_in(
                tx,
                agent,
                delivery.spec,
                delivery.lease,
                DatabaseCallback::Guard,
            )
            .await?;
            self.qualified_agent(tx, agent).await?;
            self.operation_authority(tx, op).await
        }
        .await;
        authority.is_ok()
    }
    async fn retire_database_delivery(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        op: &Value,
        delivery: &Delivery<'_>,
    ) -> Result<()> {
        let current = delivery.current(op) && op["status"] == "running";
        if current {
            self.mark_reconciliation(tx, op).await?;
        }
        let mut desired = op["desired"].as_object().cloned().unwrap_or_default();
        let mut receipts = desired
            .get("nativeDatabaseRetiredDeliveries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if receipts.len() >= 256 {
            receipts.remove(0);
        }
        receipts.push(json!({"slot":delivery.slot,"fingerprint":delivery.spec.fingerprint()?,"requestHash":delivery.hash,"tokenHash":digest(&delivery.lease.lease_token),"agentGeneration":agent.generation(),"operationGeneration":delivery.spec.operation_generation,"disposition":if current{"needs-reconciliation"}else{"obsolete"}}));
        desired.insert("nativeDatabaseRetiredDeliveries".into(), json!(receipts));
        exec(
            tx,
            "UPDATE webdock_auth.hosting_operation SET desired=$2 WHERE id=$1",
            vec![text(op, "id")?.into(), Value::Object(desired).into()],
        )
        .await
    }
}
