use super::*;
impl DatabaseService {
    pub async fn complete_database_operation(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        data: DatabaseCompletion,
    ) -> Result<Value> {
        data.validate()?;
        self.agent_scope(&tx, agent).await?;
        self.cluster_lock(&tx, agent.cluster_id()).await?;
        let op = one(
            &tx,
            &format!("{JOB} WHERE o.id=$1 AND o.cluster_id=$2 FOR UPDATE OF o,j"),
            vec![
                data.lease.operation_id.clone().into(),
                agent.cluster_id().into(),
            ],
        )
        .await?;
        if data.lease.generation != agent.generation() {
            return Err(lease_conflict());
        }
        let request_hash = completion_digest(&data)?;
        let token_hash = digest(&data.lease.lease_token);
        if let Some(receipt) = op["desired"]["nativeDatabaseCompletions"]
            .as_array()
            .and_then(|rows| {
                rows.iter().find(|r| {
                    r["requestHash"] == request_hash
                        && r["tokenHash"] == token_hash
                        && r["agentGeneration"] == agent.generation()
                })
            })
        {
            let response = receipt["response"].clone();
            tx.commit().await?;
            return Ok(response);
        }
        if op["status"] == "succeeded" {
            self.verify_job_lease(&op, agent, &data.lease, true)?;
            let old: DatabaseCompletionProof =
                serde_json::from_value(op["result"].clone()).map_err(|_| lease_conflict())?;
            if data.outcome != DatabaseCompletionOutcome::Succeeded
                || data.proof.as_ref() != Some(&old)
            {
                return Err(lease_conflict());
            }
            tx.commit().await?;
            return Ok(json!({"accepted":true}));
        }
        self.qualified_agent(&tx, agent).await?;
        self.verify_job_lease(&op, agent, &data.lease, false)?;
        let guard = self.operation_authority(&tx, &op).await?;
        if data.outcome == DatabaseCompletionOutcome::NeedsReconciliation {
            self.mark_reconciliation(&tx, &op).await?;
            let response = json!({"accepted":true});
            self.record_database_completion(&tx, &op, agent, &data, &response)
                .await?;
            tx.commit().await?;
            guard.commit().await?;
            return Ok(response);
        }
        let proof = data.proof.as_ref().ok_or_else(lease_conflict)?;
        let backend=one(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1 FOR UPDATE",vec![text(&op,"backend_id")?.into()]).await?;
        let action = text(&op, "database_action")?;
        if !op["backup_policy_id"].is_null() {
            self.complete_backup_proof(&tx, &op, proof, &backend)
                .await?;
            let response = json!({"accepted":true});
            exec(&tx,"UPDATE webdock_auth.hosting_operation SET status='succeeded',result=$2 WHERE id=$1",vec![text(&op,"id")?.into(),serde_json::to_value(proof).map_err(|_|invalid())?.into()]).await?;
            self.record_database_completion(&tx, &op, agent, &data, &response)
                .await?;
            tx.commit().await?;
            guard.commit().await?;
            return Ok(response);
        }
        let expected = match action {
            "pause" => DatabaseProofState::Paused,
            "delete" => DatabaseProofState::Retained,
            "purge" | "purgeSource" => DatabaseProofState::Purged,
            _ => DatabaseProofState::Ready,
        };
        let database = if action == "prepareMigration" {
            None
        } else {
            op["database_id"].as_str()
        };
        if op["checkpoint"]["phase"] != "observing"
            || proof.installation_id != text(&op, "backend_id")?
            || proof.database_id.as_deref() != database
            || Some(proof.cnpg_uid.as_str()) != backend["cnpg_uid"].as_str()
            || proof.revision != number(&op, "database_revision")?
            || proof.state != expected
            || (expected == DatabaseProofState::Ready && !proof.endpoint_ready)
        {
            return Err(lease_conflict());
        }
        if action == "restore"
            && proof.copy_sha256.as_deref() != op["recovery"]["verificationSHA256"].as_str()
        {
            return Err(lease_conflict());
        }
        let proof_value = serde_json::to_value(proof).map_err(|_| invalid())?;
        let state = match expected {
            DatabaseProofState::Ready => "ready",
            DatabaseProofState::Paused => "paused",
            DatabaseProofState::Retained => "retained",
            DatabaseProofState::Purged => "purged",
        };
        let response = if action == "purgeSource" {
            let changed=optional(&tx,"UPDATE webdock_auth.hosting_database_migration SET state='purged',source_credentials_encrypted='',transfer_credentials_encrypted='' WHERE id=$1 AND source_backend_id=$2 AND state='retained' RETURNING jsonb_build_object('id',id) AS value",vec![text(&op,"migration_id")?.into(),text(&op,"backend_id")?.into()]).await?;
            if changed.is_none() {
                return Err(lease_conflict());
            }
            if backend["kind"] == "dedicated" {
                exec(&tx,"UPDATE webdock_auth.hosting_database_backend SET state='purged',endpoint_ready=false,observed_at=now() WHERE id=$1",vec![text(&backend,"id")?.into()]).await?;
                exec(
                    &tx,
                    "DELETE FROM webdock_auth.hosting_database_port WHERE backend_id=$1",
                    vec![text(&backend, "id")?.into()],
                )
                .await?;
            }
            exec(&tx,"UPDATE webdock_auth.hosting_operation SET status='succeeded',result=$2 WHERE id=$1",vec![text(&op,"id")?.into(),proof_value.into()]).await?;
            json!({"accepted":true})
        } else if action == "prepareMigration" {
            one(&tx,"SELECT jsonb_build_object('id',id) AS value FROM webdock_auth.hosting_database WHERE id=$1 AND pending_backend_id=$2 AND revision=$3",vec![text(&op,"database_id")?.into(),text(&backend,"id")?.into(),number(&op,"database_revision")?.into()]).await?;
            exec(&tx,"UPDATE webdock_auth.hosting_database_backend SET state='ready',observed_revision=revision,observed_at=now(),endpoint_ready=true WHERE id=$1",vec![text(&backend,"id")?.into()]).await?;
            exec(&tx,"UPDATE webdock_auth.hosting_database_job SET action='migrate',checkpoint=NULL,lease_token_hash=NULL WHERE operation_id=$1",vec![text(&op,"id")?.into()]).await?;
            exec(&tx,"UPDATE webdock_auth.hosting_database_migration SET state='copying' WHERE database_id=$1 AND target_backend_id=$2",vec![text(&op,"database_id")?.into(),text(&backend,"id")?.into()]).await?;
            exec(&tx,"UPDATE webdock_auth.hosting_operation SET status='queued',action='databases.migrate',lease_until=NULL WHERE id=$1",vec![text(&op,"id")?.into()]).await?;
            json!({"accepted":true,"pending":true})
        } else {
            if let Some(database) = database {
                if action == "migrate" {
                    let migration=one(&tx,"SELECT to_jsonb(m) AS value FROM webdock_auth.hosting_database_migration m WHERE database_id=$1 AND target_backend_id=$2 FOR UPDATE",vec![database.into(),text(&backend,"id")?.into()]).await?;
                    if migration["job_uid"].as_str().is_none()
                        || migration["copy_sha256"].as_str().is_none()
                        || proof.copy_sha256.as_deref() != migration["copy_sha256"].as_str()
                    {
                        return Err(lease_conflict());
                    }
                    exec(&tx,"UPDATE webdock_auth.hosting_database_migration SET state='retained',retained_until=now()+interval '7 days' WHERE id=$1",vec![text(&migration,"id")?.into()]).await?;
                    exec(&tx,"UPDATE webdock_auth.hosting_database SET backend_id=pending_backend_id,pending_backend_id=NULL,kind='dedicated' WHERE id=$1 AND pending_backend_id=$2",vec![database.into(),text(&backend,"id")?.into()]).await?;
                    exec(&tx,"UPDATE webdock_auth.hosting_database_backend SET state='retained',endpoint_ready=false WHERE id=$1 AND kind='dedicated'",vec![text(&migration,"source_backend_id")?.into()]).await?;
                }
                if ["resize", "migrate"].contains(&action) {
                    let target=one(&tx,"SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1 FOR UPDATE",vec![database.into()]).await?;
                    if target["pending_order_id"].is_null()
                        || target["pending_plan_revision_id"].is_null()
                    {
                        return Err(lease_conflict());
                    }
                    exec(&tx,"UPDATE webdock_auth.hosting_database_order SET ended_at=coalesce(ended_at,now()) WHERE id=$1",vec![text(&target,"order_id")?.into()]).await?;
                    exec(&tx,"UPDATE webdock_auth.hosting_database SET order_id=pending_order_id,plan_revision_id=pending_plan_revision_id,pending_order_id=NULL,pending_plan_revision_id=NULL WHERE id=$1",vec![database.into()]).await?;
                }
                let target=optional(&tx,"UPDATE webdock_auth.hosting_database SET state=$3,observed_revision=revision,observed_at=now(),confirmed_credential_revision=CASE WHEN $3='ready' THEN credential_revision ELSE confirmed_credential_revision END,confirmed_credentials_encrypted=CASE WHEN $3='ready' THEN credentials_encrypted ELSE confirmed_credentials_encrypted END,retained_until=CASE WHEN $3='retained' THEN now()+interval '7 days' ELSE retained_until END WHERE id=$1 AND revision=$2 RETURNING jsonb_build_object('orderID',order_id) AS value",vec![database.into(),number(&op,"database_revision")?.into(),state.into()]).await?.ok_or_else(lease_conflict)?;
                if state == "ready" {
                    self.retain_backup_credentials(&tx, database).await?;
                    exec(&tx,"UPDATE webdock_auth.hosting_database_order SET billing_started_at=coalesce(billing_started_at,now()) WHERE id=$1",vec![text(&target,"orderID")?.into()]).await?;
                }
                if state == "retained" {
                    exec(&tx,"UPDATE webdock_auth.hosting_database_order SET ended_at=coalesce(ended_at,now()) WHERE id=$1",vec![text(&target,"orderID")?.into()]).await?;
                }
            }
            if backend["kind"] == "dedicated" || database.is_none() {
                exec(&tx,"UPDATE webdock_auth.hosting_database_backend SET state=$2,observed_revision=revision,observed_at=now(),endpoint_ready=$3 WHERE id=$1",vec![text(&backend,"id")?.into(),state.into(),(proof.endpoint_ready&&state=="ready").into()]).await?;
            } else if state == "ready" {
                exec(&tx,"UPDATE webdock_auth.hosting_database_backend SET endpoint_ready=true,observed_at=now() WHERE id=$1",vec![text(&backend,"id")?.into()]).await?;
            }
            if state == "purged" && backend["kind"] == "dedicated" {
                exec(
                    &tx,
                    "DELETE FROM webdock_auth.hosting_database_port WHERE backend_id=$1",
                    vec![text(&backend, "id")?.into()],
                )
                .await?;
            }
            exec(&tx,"UPDATE webdock_auth.hosting_operation SET status='succeeded',result=$2 WHERE id=$1",vec![text(&op,"id")?.into(),proof_value.into()]).await?;
            json!({"accepted":true})
        };
        self.record_database_completion(&tx, &op, agent, &data, &response)
            .await?;
        tx.commit().await?;
        guard.commit().await?;
        Ok(response)
    }
    async fn record_database_completion(
        &self,
        tx: &DatabaseTransaction,
        op: &Value,
        agent: &AgentIdentity,
        data: &DatabaseCompletion,
        response: &Value,
    ) -> Result<()> {
        let mut desired = op["desired"].as_object().cloned().unwrap_or_default();
        let mut receipts = desired
            .get("nativeDatabaseCompletions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if receipts.len() >= 32 {
            receipts.remove(0);
        }
        receipts.push(json!({"tokenHash":digest(&data.lease.lease_token),"requestHash":completion_digest(data)?,"agentGeneration":agent.generation(),"operationGeneration":op["generation"],"response":response}));
        desired.insert("nativeDatabaseCompletions".into(), json!(receipts));
        exec(
            tx,
            "UPDATE webdock_auth.hosting_operation SET desired=$2 WHERE id=$1",
            vec![text(op, "id")?.into(), Value::Object(desired).into()],
        )
        .await
    }
}
