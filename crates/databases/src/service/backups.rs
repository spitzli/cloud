//! Core-owned grants for CNPG recurrence. Initiating sessions authorize the grant, not each backup.
use super::*;
use super::{capacity::amount, control::ReceiptScope};
impl DatabaseService {
    pub(super) async fn backup_view(&self, tx: &DatabaseTransaction, id: &str) -> Result<Value> {
        let mut view = optional(tx,"SELECT jsonb_build_object('status',CASE WHEN revoked_at IS NOT NULL THEN CASE WHEN observed_revision=revision AND observed_suspended AND observed_at>clock_timestamp()-interval '90 seconds' THEN 'suspended' ELSE 'suspension-pending' END WHEN observed_revision=revision AND observed_at>clock_timestamp()-interval '90 seconds' THEN CASE WHEN observed_suspended THEN 'suspended' ELSE 'configured' END ELSE 'configuration-pending' END,'policyID',id,'revision',revision,'observedRevision',observed_revision,'walEnabled',spec->'walEnabled','retentionDelete',spec->'retentionDelete','observedControls',observed_controls,'lastSuccessAt',(SELECT max(b.completed_at) FROM webdock_auth.hosting_database_backup b WHERE b.policy_id=p.id),'recoverableUntil',NULL,'lastRestoreTestAt',NULL) AS value FROM webdock_auth.hosting_database_backup_policy p WHERE database_id=$1",vec![id.into()]).await?.unwrap_or(json!({"status":"not-configured","lastSuccessAt":null,"recoverableUntil":null,"lastRestoreTestAt":null}));
        view["observedControls"] = view.get("observedControls").cloned().unwrap_or(Value::Null);
        view["publicRecoveryAvailable"] = json!(false);
        view["disabledReason"] = json!("production-qualification-required");
        view["rpoSeconds"] = Value::Null;
        view["rtoSeconds"] = Value::Null;
        Ok(view)
    }
    pub(super) async fn backup_command(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        cmd: &Value,
    ) -> Result<Value> {
        let action = text(cmd, "action")?;
        if action == "databases.backupDestinations.register" {
            self.authorize_mode(tx, actor, None, None, true, true, authority, true)
                .await?;
            let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
            if let Some(prior) = prior {
                return Ok(prior);
            }
            let write: DatabaseObjectStoreMaterial =
                serde_json::from_value(cmd["writeCredentials"].clone()).map_err(|_| invalid())?;
            let recovery: DatabaseObjectStoreMaterial =
                serde_json::from_value(cmd["recoveryCredentials"].clone())
                    .map_err(|_| invalid())?;
            if write.read_only
                || !recovery.read_only
                || write.access_key_id == recovery.access_key_id
            {
                return Err(invalid());
            }
            let id = self.id().await?;
            let destination = json!({"id":id,"endpointURL":cmd["endpointURL"],"region":cmd["region"],"bucket":cmd["bucket"],"archivePath":"","serverName":"","credentialID":self.id().await?,"credentialRevision":1,"recoveryCredentialID":self.id().await?,"recoveryCredentialRevision":1,"keyRecoveryID":cmd["keyRecoveryID"]});
            let write = self
                .cipher
                .seal(&write, &format!("database-backup-destination:{id}:write:1"))
                .map_err(|_| unavailable())?;
            let recovery = self
                .cipher
                .seal(
                    &recovery,
                    &format!("database-backup-destination:{id}:recovery:1"),
                )
                .map_err(|_| unavailable())?;
            exec(tx,"INSERT INTO webdock_auth.hosting_database_backup_destination(id,spec,write_encrypted,recovery_encrypted,created_by) VALUES($1,$2,$3,$4,$5)",vec![id.clone().into(),destination.into(),write.into(),recovery.into(),actor.user_id.clone().into()]).await?;
            let result = json!({"destinationID":id});
            self.record_control(tx, actor, cmd, &hash, &result, ReceiptScope::default())
                .await?;
            self.control_audit(tx, actor, action, &id).await?;
            return Ok(result);
        }
        let id = text(cmd, "databaseID")?;
        let read = action == "databases.backupPolicies.get";
        let d = self
            .control_access(tx, actor, authority, id, !read, !read, !read)
            .await?;
        if d["kind"] != "dedicated" {
            return Err(conflict(
                "Shared PostgreSQL backup and recovery are unavailable.",
            ));
        }
        if read {
            let policy=optional(tx,"SELECT jsonb_build_object('id',id,'spec',spec,'revision',revision,'revokedAt',revoked_at,'observedRevision',observed_revision,'observedSuspended',observed_suspended,'observedAt',observed_at,'observedControls',observed_controls,'revocationEffective',revoked_at IS NOT NULL AND observed_suspended AND observed_revision=revision AND observed_at>clock_timestamp()-interval '90 seconds') AS value FROM webdock_auth.hosting_database_backup_policy WHERE database_id=$1",vec![id.into()]).await?;
            let backups=all(tx,"SELECT jsonb_build_object('id',b.id,'policyID',b.policy_id,'baseBackupID',b.base_backup_id,'completedAt',b.completed_at) AS value FROM webdock_auth.hosting_database_backup b JOIN webdock_auth.hosting_database_backup_policy p ON p.id=b.policy_id WHERE p.database_id=$1 ORDER BY b.completed_at DESC LIMIT 100",vec![id.into()]).await?;
            return Ok(json!({"policy":policy,"backups":backups}));
        }
        let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
        if let Some(prior) = prior {
            return Ok(prior);
        }
        if d["revision"] != cmd["revision"] {
            return Err(conflict("Database revision changed."));
        }
        let b=one(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1 FOR UPDATE",vec![text(&d,"backend_id")?.into()]).await?;
        self.cluster_lock(tx, text(&b, "cluster_id")?).await?;
        self.fresh_cluster(tx, text(&b, "cluster_id")?).await?;
        let existing=optional(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_backup_policy p WHERE database_id=$1 FOR UPDATE",vec![id.into()]).await?;
        let policy = if action == "databases.backupPolicies.grant" {
            if existing.is_some() {
                return Err(conflict(
                    "A backup policy already exists. Its archive identity is immutable.",
                ));
            }
            if d["state"] != "ready"
                || b["state"] != "ready"
                || b["cnpg_uid"].as_str().is_none()
                || !d["pending_backend_id"].is_null()
                || d["confirmed_credential_revision"] != d["credential_revision"]
            {
                return Err(conflict(
                    "A ready dedicated installation with confirmed credentials is required.",
                ));
            }
            let destination=one(tx,"SELECT spec AS value FROM webdock_auth.hosting_database_backup_destination WHERE id=$1",vec![text(cmd,"destinationID")?.into()]).await?;
            let pid = self.id().await?;
            let mut spec = json!({"id":pid,"installationID":b["id"],"customerID":d["customer_id"],"projectID":d["project_id"],"sourceCnpgUID":b["cnpg_uid"],"revision":1,"resourceRevision":b["revision"],"destination":destination,"schedule":cmd["schedule"],"suspended":false,"walEnabled":true,"retentionDays":cmd["retentionDays"],"retentionDelete":cmd["retentionDelete"],"sidecar":{"cpuMillicores":100,"memoryBytes":268435456,"ephemeralBytes":268435456},"recoveryJob":{"cpuMillicores":500,"memoryBytes":536870912,"ephemeralBytes":536870912}});
            spec["destination"]["archivePath"] = json!(format!(
                "postgresql/{}/{}/{}/{pid}",
                text(&d, "customer_id")?,
                text(&d, "project_id")?,
                text(&b, "id")?
            ));
            spec["destination"]["serverName"] = json!(format!("wd-{}-{pid}", text(&b, "id")?));
            let typed: DatabaseBackupPolicySpec =
                serde_json::from_value(spec.clone()).map_err(|_| invalid())?;
            typed.validate()?;
            self.reserve_backup_resources(tx, &b, &typed).await?;
            let credentials: DatabaseCredentials = self
                .cipher
                .open(
                    text(&d, "confirmed_credentials_encrypted")?,
                    &format!("database:{id}:{}", number(&d, "credential_revision")?),
                )
                .map_err(|_| unavailable())?;
            let sealed = self
                .cipher
                .seal(
                    &credentials,
                    &format!(
                        "database-backup-policy:{pid}:credentials:{}",
                        number(&d, "credential_revision")?
                    ),
                )
                .map_err(|_| unavailable())?;
            one(tx,"INSERT INTO webdock_auth.hosting_database_backup_policy(id,database_id,backend_id,cluster_id,customer_id,project_id,destination_id,source_uid,spec,revision,approved_by,credentials_encrypted,credential_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,1,$10,$11,$12) RETURNING to_jsonb(hosting_database_backup_policy) AS value",vec![pid.into(),id.into(),text(&b,"id")?.into(),text(&b,"cluster_id")?.into(),text(&d,"customer_id")?.into(),text(&d,"project_id")?.into(),text(cmd,"destinationID")?.into(),text(&b,"cnpg_uid")?.into(),spec.into(),actor.user_id.clone().into(),sealed.into(),number(&d,"credential_revision")?.into()]).await?
        } else {
            let p = existing.ok_or_else(|| conflict("No backup policy is configured."))?;
            if action.starts_with("databases.backupPolicies.")
                && p["revision"] != cmd["policyRevision"]
            {
                return Err(conflict("Backup policy revision changed."));
            }
            if action == "databases.backupPolicies.update" {
                if !p["revoked_at"].is_null() {
                    return Err(conflict("The backup policy is revoked."));
                }
                let mut spec: DatabaseBackupPolicySpec =
                    serde_json::from_value(p["spec"].clone()).map_err(|_| invalid())?;
                spec.revision += 1;
                spec.resource_revision = number(&b, "revision")?;
                spec.schedule = text(cmd, "schedule")?.into();
                spec.suspended = cmd["suspended"].as_bool().ok_or_else(invalid)?;
                spec.wal_enabled = cmd["walEnabled"].as_bool().ok_or_else(invalid)?;
                spec.retention_delete = cmd["retentionDelete"].as_bool().ok_or_else(invalid)?;
                spec.retention_days =
                    u32::try_from(number(cmd, "retentionDays")?).map_err(|_| invalid())?;
                spec.validate()?;
                one(tx,"UPDATE webdock_auth.hosting_database_backup_policy SET spec=$2,revision=$3 WHERE id=$1 RETURNING to_jsonb(hosting_database_backup_policy) AS value",vec![text(&p,"id")?.into(),serde_json::to_value(spec.clone()).map_err(|_|invalid())?.into(),spec.revision.into()]).await?
            } else if action == "databases.backupPolicies.revoke" {
                if p["revoked_at"].is_null() {
                    one(tx,"UPDATE webdock_auth.hosting_database_backup_policy SET revoked_at=now(),revision=revision+1,spec=jsonb_set(jsonb_set(jsonb_set(spec,'{suspended}','true'),'{revision}',to_jsonb(revision+1)),'{resourceRevision}',to_jsonb($2::integer)) WHERE id=$1 RETURNING to_jsonb(hosting_database_backup_policy) AS value",vec![text(&p,"id")?.into(),number(&b,"revision")?.into()]).await?
                } else {
                    p
                }
            } else {
                admit_manual_backup(&p)?;
                p
            }
        };
        if action == "databases.backupPolicies.grant" {
            exec(tx,"INSERT INTO webdock_auth.hosting_database_backup_credentials(policy_id,revision,credentials_encrypted) VALUES($1,$2,$3)",vec![text(&policy,"id")?.into(),number(&policy,"credential_revision")?.into(),text(&policy,"credentials_encrypted")?.into()]).await?;
        }
        if matches!(
            action,
            "databases.backupPolicies.revoke" | "databases.backupPolicies.update"
        ) {
            exec(tx,"UPDATE webdock_auth.hosting_operation o SET status='failed',generation=generation+1,lease_until=NULL FROM webdock_auth.hosting_database_job j WHERE j.operation_id=o.id AND j.backup_policy_id=$1 AND o.status IN('queued','running','needs-reconciliation')",vec![text(&policy,"id")?.into()]).await?;
        }
        let operation = self
            .enqueue_backup_policy(
                tx,
                actor,
                authority,
                &d,
                &policy,
                if action == "databases.backup" {
                    "backup"
                } else {
                    "configureBackup"
                },
            )
            .await?;
        let result = json!({"policyID":policy["id"],"revision":policy["revision"],"operationID":operation,"revocationEffective":false});
        self.backup_audit(
            tx,
            &policy,
            &actor.user_id,
            action,
            json!({"operationID":operation}),
        )
        .await?;
        self.record_control(
            tx,
            actor,
            cmd,
            &hash,
            &result,
            ReceiptScope {
                customer: d["customer_id"].as_str(),
                project: d["project_id"].as_str(),
                cluster: b["cluster_id"].as_str(),
            },
        )
        .await?;
        Ok(result)
    }
    pub(super) async fn restore_backup(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        cmd: &Value,
    ) -> Result<Value> {
        let source = self
            .control_access(
                tx,
                actor,
                authority,
                text(cmd, "sourceDatabaseID")?,
                true,
                true,
                true,
            )
            .await?;
        if source["kind"] != "dedicated" {
            return Err(conflict("Shared PostgreSQL recovery is unavailable."));
        }
        let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
        if let Some(prior) = prior {
            return Ok(prior);
        }
        let (p, target_time) = self.restore_source(tx, &source, cmd).await?;
        let (target, b, op) = self.unused_restore_target(tx, &source, &p, cmd).await?;
        let (typed, sealed) = self
            .reserve_restore_recovery(tx, &p, &target, &b, cmd)
            .await?;
        let credential_revision = number(cmd, "credentialRevision")?;
        exec(tx,"UPDATE webdock_auth.hosting_operation SET status='failed',generation=generation+1 WHERE id=$1",vec![text(&op,"id")?.into()]).await?;
        exec(tx,"UPDATE webdock_auth.hosting_database SET state='restoring',credential_revision=$2,credentials_encrypted=$3 WHERE id=$1",vec![text(&target,"id")?.into(),credential_revision.into(),sealed.into()]).await?;
        let recovery = restore_recovery_spec(&source, &p, cmd, typed, target_time)?;
        let operation = self
            .enqueue_restore(
                tx,
                actor,
                authority,
                &b,
                &target,
                &recovery,
                text(&p, "backend_id")?,
            )
            .await?
            .id;
        let result = json!({"operationID":operation,"databaseID":target["id"],"installationID":b["id"],"state":"restoring"});
        self.backup_audit(tx, &p, &actor.user_id, "backup.restore", result.clone())
            .await?;
        self.record_control(
            tx,
            actor,
            cmd,
            &hash,
            &result,
            ReceiptScope {
                customer: source["customer_id"].as_str(),
                project: source["project_id"].as_str(),
                cluster: b["cluster_id"].as_str(),
            },
        )
        .await?;
        Ok(result)
    }
    pub(super) async fn restore_source(
        &self,
        tx: &DatabaseTransaction,
        source: &Value,
        cmd: &Value,
    ) -> Result<(Value, chrono::DateTime<chrono::Utc>)> {
        admit_restore_source(source, cmd)?;
        // Backup records are immutable. Locking them also invokes UPDATE RLS,
        // which correctly grants no update authority over completed evidence.
        let p=one(tx,"SELECT to_jsonb(p)||jsonb_build_object('base_backup_id',b.base_backup_id,'completed_at',b.completed_at) AS value FROM webdock_auth.hosting_database_backup_policy p JOIN webdock_auth.hosting_database_backup b ON b.policy_id=p.id WHERE b.id=$1 AND p.database_id=$2 FOR SHARE OF p",vec![text(cmd,"restorePointID")?.into(),text(source,"id")?.into()]).await?;
        let target_time = restore_target_time(&p, cmd, chrono::Utc::now())?;
        Ok((p, target_time))
    }
    async fn unused_restore_target(
        &self,
        tx: &DatabaseTransaction,
        source: &Value,
        p: &Value,
        cmd: &Value,
    ) -> Result<(Value, Value, Value)> {
        let target=one(tx,"SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_order o ON o.id=d.order_id WHERE o.id=$1 AND d.project_id=$2 AND d.customer_id=$3 AND d.kind='dedicated' FOR UPDATE OF d,o",vec![text(cmd,"targetOrderID")?.into(),text(cmd,"targetProjectID")?.into(),text(source,"customer_id")?.into()]).await?;
        admit_restore_target(&target, source, cmd)?;
        let b=one(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1 FOR UPDATE",vec![text(&target,"backend_id")?.into()]).await?;
        self.cluster_lock(tx, text(&b, "cluster_id")?).await?;
        admit_restore_backend(&b, p)?;
        let op=one(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE o.id=$1 FOR UPDATE",vec![text(&target,"operation_id")?.into()]).await?;
        admit_restore_create(&op)?;
        Ok((target, b, op))
    }
    pub(super) async fn reserve_restore_recovery(
        &self,
        tx: &DatabaseTransaction,
        p: &Value,
        target: &Value,
        b: &Value,
        cmd: &Value,
    ) -> Result<(DatabaseBackupPolicySpec, String)> {
        let typed: DatabaseBackupPolicySpec =
            serde_json::from_value(p["spec"].clone()).map_err(|_| invalid())?;
        self.reserve_backup_resources(tx, b, &typed).await?;
        let credential_revision = number(cmd, "credentialRevision")?;
        let version=one(tx,"SELECT to_jsonb(v) AS value FROM webdock_auth.hosting_database_backup_credentials v WHERE policy_id=$1 AND revision=$2",vec![text(p,"id")?.into(),credential_revision.into()]).await?;
        let sealed = reseal_restore_credentials(
            &self.cipher,
            p,
            credential_revision,
            text(&version, "credentials_encrypted")?,
            text(target, "id")?,
        )?;
        Ok((typed, sealed))
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "The existing transaction, actor and exact recovery target stay explicit at enqueue."
    )]
    pub(super) async fn enqueue_restore(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        b: &Value,
        target: &Value,
        recovery: &DatabaseRecoverySpec,
        source_backend: &str,
    ) -> Result<super::orders::QueuedDatabase> {
        let operation = self
            .insert_database_job(
                tx,
                actor,
                authority,
                text(b, "id")?,
                Some(text(target, "id")?),
                "restore",
                number(target, "revision")?,
                true,
            )
            .await?;
        exec(tx,"UPDATE webdock_auth.hosting_database_job SET recovery=$2,source_backend_id=$3 WHERE operation_id=$1",vec![operation.id.clone().into(),serde_json::to_value(recovery).map_err(|_|invalid())?.into(),source_backend.into()]).await?;
        Ok(operation)
    }
    async fn enqueue_backup_policy(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        d: &Value,
        p: &Value,
        action: &str,
    ) -> Result<String> {
        let op = self
            .enqueue_database(
                tx,
                actor,
                authority,
                text(p, "backend_id")?,
                Some(text(d, "id")?),
                action,
                number(d, "revision")?,
                false,
            )
            .await?;
        exec(tx,"UPDATE webdock_auth.hosting_database_job SET backup_policy_id=$2,backup_policy_revision=$3 WHERE operation_id=$1",vec![op.clone().into(),text(p,"id")?.into(),number(p,"revision")?.into()]).await?;
        Ok(op)
    }
    pub(super) async fn backup_audit(
        &self,
        tx: &DatabaseTransaction,
        p: &Value,
        subject: &str,
        action: &str,
        detail: Value,
    ) -> Result<()> {
        exec(tx,"INSERT INTO webdock_auth.hosting_database_backup_audit(id,policy_id,subject,action,revision,detail) VALUES($1,$2,$3,$4,$5,$6)",vec![self.id().await?.into(),text(p,"id")?.into(),subject.into(),action.into(),number(p,"revision")?.into(),detail.into()]).await
    }
    pub(super) async fn backup_policy_authority(
        &self,
        tx: &DatabaseTransaction,
        op: &Value,
    ) -> Result<()> {
        let c = self.fresh_cluster(tx, text(op, "cluster_id")?).await?;
        if c["observation"]["capabilities"]["databaseBackupVersion"] != 1 {
            return Err(denied());
        }
        let p=one(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_backup_policy p WHERE p.id=$1 AND p.cluster_id=$2 AND webdock_rls.database_backup_policy_live(p.id,p.cluster_id) AND EXISTS(SELECT 1 FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=p.database_id AND d.revision=$3 AND b.id=p.backend_id AND b.revision=(p.spec->>'resourceRevision')::integer) FOR SHARE OF p",vec![text(op,"backup_policy_id")?.into(),text(op,"cluster_id")?.into(),number(op,"database_revision")?.into()]).await?;
        let spec: DatabaseBackupPolicySpec =
            serde_json::from_value(p["spec"].clone()).map_err(|_| denied())?;
        spec.validate()?;
        if p["revision"] != op["backup_policy_revision"]
            || p["database_id"] != op["database_id"]
            || p["backend_id"] != op["backend_id"]
            || !["configureBackup", "backup"].contains(&text(op, "database_action")?)
            || (!p["revoked_at"].is_null()
                && (op["database_action"] != "configureBackup" || !spec.suspended))
        {
            return Err(denied());
        }
        Ok(())
    }
    pub(super) async fn retain_backup_credentials(
        &self,
        tx: &DatabaseTransaction,
        database: &str,
    ) -> Result<()> {
        let Some(p)=optional(tx,"SELECT to_jsonb(p)||jsonb_build_object('current_revision',d.credential_revision,'current_credentials',d.credentials_encrypted) AS value FROM webdock_auth.hosting_database_backup_policy p JOIN webdock_auth.hosting_database d ON d.id=p.database_id JOIN webdock_auth.hosting_database_backend b ON b.id=p.backend_id AND b.cnpg_uid=p.source_uid WHERE p.database_id=$1 AND d.backend_id=p.backend_id FOR SHARE OF p",vec![database.into()]).await? else{return Ok(())};
        let revision = number(&p, "current_revision")?;
        let credentials: DatabaseCredentials = self
            .cipher
            .open(
                text(&p, "current_credentials")?,
                &format!("database:{database}:{revision}"),
            )
            .map_err(|_| unavailable())?;
        let encrypted = self
            .cipher
            .seal(
                &credentials,
                &format!(
                    "database-backup-policy:{}:credentials:{revision}",
                    text(&p, "id")?
                ),
            )
            .map_err(|_| unavailable())?;
        exec(tx,"INSERT INTO webdock_auth.hosting_database_backup_credentials(policy_id,revision,credentials_encrypted) VALUES($1,$2,$3) ON CONFLICT(policy_id,revision) DO NOTHING",vec![text(&p,"id")?.into(),revision.into(),encrypted.into()]).await
    }
    pub(super) async fn enqueue_backup_observation(
        &self,
        tx: &DatabaseTransaction,
        cluster: &str,
    ) -> Result<()> {
        let Some(p)=optional(tx,"SELECT to_jsonb(p)||jsonb_build_object('database_revision',d.revision) AS value FROM webdock_auth.hosting_database_backup_policy p JOIN webdock_auth.hosting_database d ON d.id=p.database_id JOIN webdock_auth.hosting_database_backend b ON b.id=p.backend_id WHERE p.cluster_id=$1 AND (p.observed_revision<>p.revision OR (NOT p.spec->>'suspended'='true' AND (p.observed_at IS NULL OR p.observed_at<now()-interval '60 seconds'))) AND d.state='ready' AND b.revision=(p.spec->>'resourceRevision')::integer AND webdock_rls.database_backup_policy_live(p.id,p.cluster_id) AND NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.backend_id=p.backend_id AND o.status IN('queued','running','needs-reconciliation')) ORDER BY p.observed_at NULLS FIRST,p.id FOR UPDATE OF p SKIP LOCKED LIMIT 1",vec![cluster.into()]).await? else{return Ok(())};
        let id = self.id().await?;
        let nonce = token()?;
        let observation_only = p["observed_revision"] == p["revision"];
        let checkpoint =
            observation_only.then(|| json!({"phase":"observing","cnpgUID":p["source_uid"]}));
        exec(tx,"INSERT INTO webdock_auth.hosting_operation(id,subject,customer_id,project_id,cluster_id,action,idempotency_key,payload_hash,status,desired,target_revision) VALUES($1,$2,$3,$4,$5,'databases.configureBackup',$6,$7,'queued',$8,$9)",vec![id.clone().into(),text(&p,"approved_by")?.into(),text(&p,"customer_id")?.into(),text(&p,"project_id")?.into(),cluster.into(),format!("database-backup-observe-{nonce}").into(),hash(&nonce)?.into(),json!({"databaseVersion":1,"backupPolicyID":p["id"],"authority":"durable-backup-policy","observationOnly":observation_only}).into(),number(&p,"database_revision")?.into()]).await?;
        exec(tx,"INSERT INTO webdock_auth.hosting_database_job(operation_id,backend_id,database_id,action,revision,backup_policy_id,backup_policy_revision,checkpoint) VALUES($1,$2,$3,'configureBackup',$4,$5,$6,$7)",vec![id.into(),text(&p,"backend_id")?.into(),text(&p,"database_id")?.into(),number(&p,"database_revision")?.into(),text(&p,"id")?.into(),number(&p,"revision")?.into(),checkpoint.into()]).await
    }
    pub(super) async fn complete_backup_proof(
        &self,
        tx: &DatabaseTransaction,
        op: &Value,
        proof: &DatabaseCompletionProof,
        b: &Value,
    ) -> Result<()> {
        let p=one(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_backup_policy p WHERE id=$1 AND revision=$2 FOR UPDATE",vec![text(op,"backup_policy_id")?.into(),number(op,"backup_policy_revision")?.into()]).await?;
        let observed = proof
            .backup
            .as_ref()
            .ok_or_else(|| conflict("An observed CNPG backup proof is required."))?;
        if proof.installation_id != text(&p, "backend_id")?
            || proof.database_id.as_deref() != p["database_id"].as_str()
            || proof.cnpg_uid != text(&p, "source_uid")?
            || b["cnpg_uid"] != p["source_uid"]
            || proof.revision != number(op, "database_revision")?
            || observed.policy_id != text(&p, "id")?
            || observed.revision != number(&p, "revision")?
            || Some(observed.suspended) != p["spec"]["suspended"].as_bool()
            || observed.scheduled_backup_uid.is_empty()
            || p["spec"]["schedule"] != observed.controls.schedule
            || p["spec"]["walEnabled"] != observed.controls.wal_enabled
            || p["spec"]["retentionDelete"] != observed.controls.retention_delete
            || p["spec"]["retentionDays"] != observed.controls.retention_days
            || op["checkpoint"]["phase"] != "observing"
        {
            return Err(conflict("Backup observation is stale or foreign."));
        }
        if op["database_action"] == "backup" || observed.base_backup_id.is_some() {
            let base = observed
                .base_backup_id
                .as_ref()
                .filter(|s| {
                    s.len() == 15
                        && s.bytes().enumerate().all(|(i, v)| {
                            if i == 8 {
                                v == b'T'
                            } else {
                                v.is_ascii_digit()
                            }
                        })
                })
                .ok_or_else(invalid)?;
            let uid = observed
                .backup_uid
                .as_ref()
                .filter(|s| !s.is_empty())
                .ok_or_else(invalid)?;
            let completed = observed.completed_at.ok_or_else(invalid)?;
            if completed > chrono::Utc::now() + chrono::Duration::seconds(30) {
                return Err(invalid());
            }
            exec(tx,"INSERT INTO webdock_auth.hosting_database_backup(id,policy_id,policy_revision,backup_uid,base_backup_id,completed_at,proof) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(policy_id,backup_uid) DO NOTHING",vec![self.id().await?.into(),text(&p,"id")?.into(),number(&p,"revision")?.into(),uid.clone().into(),base.clone().into(),completed.into(),serde_json::to_value(observed).map_err(|_|invalid())?.into()]).await?;
        }
        exec(tx,"UPDATE webdock_auth.hosting_database_backup_policy SET observed_revision=$2,observed_suspended=$3,schedule_uid=$4,observed_at=now(),observed_controls=$5 WHERE id=$1",vec![text(&p,"id")?.into(),observed.revision.into(),observed.suspended.into(),observed.scheduled_backup_uid.clone().into(),serde_json::to_value(&observed.controls).map_err(|_|invalid())?.into()]).await?;
        self.backup_audit(
            tx,
            &p,
            "enrolled-agent",
            "backup.observed",
            serde_json::to_value(observed).map_err(|_| invalid())?,
        )
        .await
    }
    pub(super) async fn reserve_backup_resources(
        &self,
        tx: &DatabaseTransaction,
        b: &Value,
        p: &DatabaseBackupPolicySpec,
    ) -> Result<()> {
        let cluster = text(b, "cluster_id")?;
        let c = self.fresh_cluster(tx, cluster).await?;
        if c["observation"]["capabilities"]["databaseBackupVersion"] != 1 {
            return Err(conflict(
                "A qualified native PostgreSQL backup agent is required.",
            ));
        }
        let apps = self.capacity_demand(tx, cluster, "apps").await?;
        let native = self.capacity_demand(tx, cluster, "native").await?;
        let mut demand = b["demand"].clone();
        for (key, extra) in [
            (
                "cpuMillicores",
                p.sidecar.cpu_millicores + p.recovery_job.cpu_millicores,
            ),
            (
                "memoryBytes",
                p.sidecar.memory_bytes + p.recovery_job.memory_bytes,
            ),
            (
                "ephemeralBytes",
                p.sidecar.ephemeral_bytes + p.recovery_job.ephemeral_bytes,
            ),
        ] {
            if amount(&apps[key])? + amount(&native[key])? + extra > amount(&c["capacity"][key])? {
                return Err(conflict(
                    "Backup and recovery peak capacity is insufficient.",
                ));
            }
            demand[key] = json!(amount(&demand[key])? + extra);
        }
        let pools = all(
            tx,
            "SELECT to_jsonb(p) AS value FROM webdock_rls.capacity_pools($1) p",
            vec![cluster.into()],
        )
        .await?;
        let aggregate = pools
            .iter()
            .find(|p| p["node"] == b["node"] && p["pool_id"] == b["pool_id"])
            .ok_or_else(unavailable)?;
        let pool = c["observation"]["capabilities"]["databasePools"]
            .as_array()
            .and_then(|ps| {
                ps.iter().find(|p| {
                    p["node"] == b["node"] && p["poolID"] == b["pool_id"] && p["ready"] == true
                })
            })
            .ok_or_else(unavailable)?;
        let node = c["observation"]["nodes"]
            .as_array()
            .and_then(|ns| ns.iter().find(|n| n["id"] == b["node"]))
            .ok_or_else(unavailable)?;
        let delta = json!({"cpuMillicores":p.sidecar.cpu_millicores+p.recovery_job.cpu_millicores,"memoryBytes":p.sidecar.memory_bytes+p.recovery_job.memory_bytes,"volumeBytes":0});
        if !super::capacity::fits(pool, aggregate, node, &apps, &delta)? {
            return Err(conflict("Backup node headroom is insufficient."));
        }
        exec(
            tx,
            "UPDATE webdock_auth.hosting_database_backend SET demand=$2 WHERE id=$1",
            vec![text(b, "id")?.into(), demand.into()],
        )
        .await
    }
}

fn admit_manual_backup(policy: &Value) -> Result<()> {
    if !policy["revoked_at"].is_null() {
        return Err(conflict("The backup policy is revoked."));
    }
    if policy["spec"]["suspended"] != false || policy["spec"]["walEnabled"] != true {
        return Err(conflict(
            "Resume the backup policy and WAL archiving before requesting a backup.",
        ));
    }
    Ok(())
}
#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn suspended_or_wal_disabled_policy_cannot_admit_a_manual_backup() {
        for spec in [
            json!({"suspended":true,"walEnabled":true}),
            json!({"suspended":true,"walEnabled":false}),
            json!({"suspended":false,"walEnabled":false}),
        ] {
            assert!(admit_manual_backup(&json!({"revoked_at":null,"spec":spec})).is_err());
        }
        admit_manual_backup(
            &json!({"revoked_at":null,"spec":{"suspended":false,"walEnabled":true}}),
        )
        .unwrap();
    }
}

pub(super) fn admit_restore_source(source: &Value, cmd: &Value) -> Result<()> {
    if source["revision"] != cmd["revision"] || source["project_id"] != cmd["targetProjectID"] {
        return Err(conflict(
            "Recovery requires the current source and the same project.",
        ));
    }
    Ok(())
}
pub(super) fn restore_target_time(
    p: &Value,
    cmd: &Value,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<chrono::DateTime<chrono::Utc>> {
    let target = chrono::DateTime::parse_from_rfc3339(text(cmd, "targetTime")?)
        .map_err(|_| invalid())?
        .with_timezone(&chrono::Utc);
    let completed =
        chrono::DateTime::parse_from_rfc3339(text(p, "completed_at")?).map_err(|_| invalid())?;
    if target < completed || target > now {
        return Err(conflict(
            "The recovery target must follow the exact completed base backup.",
        ));
    }
    Ok(target)
}
pub(super) fn admit_restore_target(target: &Value, source: &Value, cmd: &Value) -> Result<()> {
    if target["id"] == source["id"]
        || target["name"] != cmd["name"]
        || target["state"] != "provisioning"
    {
        return Err(conflict(
            "An unused accepted dedicated target order is required.",
        ));
    }
    Ok(())
}
pub(super) fn admit_restore_backend(b: &Value, p: &Value) -> Result<()> {
    if b["cnpg_uid"].as_str().is_some()
        || b["revision"] != 1
        || b["id"] == p["backend_id"]
        || b["state"] != "provisioning"
        || b["cluster_id"] != p["cluster_id"]
    {
        return Err(conflict(
            "Recovery must use a new installation, namespace and bounded volume.",
        ));
    }
    Ok(())
}
fn admit_restore_create(op: &Value) -> Result<()> {
    if op["status"] != "queued" || op["action"] != "databases.create" {
        return Err(conflict(
            "The target has already been claimed. Create a fresh target order.",
        ));
    }
    Ok(())
}
pub(super) fn restore_recovery_spec(
    source: &Value,
    p: &Value,
    cmd: &Value,
    typed: DatabaseBackupPolicySpec,
    target_time: chrono::DateTime<chrono::Utc>,
) -> Result<DatabaseRecoverySpec> {
    Ok(DatabaseRecoverySpec {
        source: typed,
        source_database_id: text(source, "id")?.into(),
        base_backup_id: text(p, "base_backup_id")?.into(),
        target_time,
        timeline: u32::try_from(number(cmd, "timeline")?).map_err(|_| invalid())?,
        credential_revision: number(cmd, "credentialRevision")?,
        verification_sha256: text(cmd, "verificationSHA256")?.into(),
    })
}
pub(super) fn reseal_restore_credentials(
    cipher: &SecretCipher,
    p: &Value,
    revision: i32,
    encrypted: &str,
    target: &str,
) -> Result<String> {
    let credentials: DatabaseCredentials = cipher
        .open(
            encrypted,
            &format!(
                "database-backup-policy:{}:credentials:{revision}",
                text(p, "id")?
            ),
        )
        .map_err(|_| unavailable())?;
    cipher
        .seal(&credentials, &format!("database:{target}:{revision}"))
        .map_err(|_| unavailable())
}

#[cfg(test)]
mod restore_admission_tests {
    use super::*;
    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-10-25T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }
    fn command() -> Value {
        json!({"revision":9,"targetProjectID":"101","name":"Recovered","targetTime":"2026-10-25T10:00:01Z","timeline":2,"credentialRevision":1,"verificationSHA256":"a".repeat(64)})
    }
    #[test]
    fn recovery_source_requires_the_current_revision_and_same_project() {
        let source = json!({"id":"102","revision":9,"project_id":"101"});
        admit_restore_source(&source, &command()).unwrap();
        for (key, value) in [("revision", json!(8)), ("targetProjectID", json!("999"))] {
            let mut cmd = command();
            cmd[key] = value;
            assert!(admit_restore_source(&source, &cmd).is_err());
        }
    }
    #[test]
    fn recovery_time_is_bounded_by_the_exact_base_and_current_clock_with_offsets() {
        let p = json!({"completed_at":"2026-10-25T10:00:00Z"});
        for time in ["2026-10-25T12:00:00+02:00", "2026-10-25T12:00:00Z"] {
            let mut cmd = command();
            cmd["targetTime"] = json!(time);
            restore_target_time(&p, &cmd, now()).unwrap();
        }
        for time in ["2026-10-25T09:59:59Z", "2026-10-25T12:00:01Z", "latest"] {
            let mut cmd = command();
            cmd["targetTime"] = json!(time);
            assert!(restore_target_time(&p, &cmd, now()).is_err());
        }
    }
    #[test]
    fn recovery_target_must_be_a_different_unused_exactly_named_database() {
        let source = json!({"id":"102"});
        let target = json!({"id":"103","name":"Recovered","state":"provisioning"});
        admit_restore_target(&target, &source, &command()).unwrap();
        for (key, value) in [
            ("id", json!("102")),
            ("name", json!("Other")),
            ("state", json!("ready")),
            ("state", json!("restoring")),
        ] {
            let mut bad = target.clone();
            bad[key] = value;
            assert!(admit_restore_target(&bad, &source, &command()).is_err());
        }
    }
    #[test]
    fn recovery_backend_must_still_be_fresh_and_distinct_in_the_source_cluster() {
        let p = json!({"backend_id":"104","cluster_id":"105"});
        let backend = json!({"id":"106","cluster_id":"105","cnpg_uid":null,"revision":1,"state":"provisioning"});
        admit_restore_backend(&backend, &p).unwrap();
        for (key, value) in [
            ("cnpg_uid", json!("already-issued")),
            ("cnpg_uid", json!("")),
            ("revision", json!(2)),
            ("id", json!("104")),
            ("cluster_id", json!("999")),
            ("state", json!("ready")),
        ] {
            let mut bad = backend.clone();
            bad[key] = value;
            assert!(admit_restore_backend(&bad, &p).is_err());
        }
    }
    #[test]
    fn legacy_restore_only_retires_a_queued_create() {
        admit_restore_create(&json!({"status":"queued","action":"databases.create"})).unwrap();
        for status in ["running", "needs-reconciliation", "succeeded", "failed"] {
            assert!(
                admit_restore_create(&json!({"status":status,"action":"databases.create"}))
                    .is_err()
            );
        }
        assert!(
            admit_restore_create(&json!({"status":"queued","action":"databases.restore"})).is_err()
        );
    }
    #[test]
    fn recovery_packet_preserves_exact_policy_history_and_explicit_role_revision() {
        let recorded: Value = serde_json::from_str(include_str!(
            "../../../../docs/verification/2026-10-25-native-postgresql-backup-transitions.json"
        ))
        .unwrap();
        let policy = &recorded["connectedEvidence"]["resize"]["policy"]["spec"];
        let typed: DatabaseBackupPolicySpec = serde_json::from_value(policy.clone()).unwrap();
        let source = json!({"id":"102","revision":9,"credential_revision":7});
        let p = json!({"base_backup_id":"20261025T100000"});
        let time = chrono::DateTime::parse_from_rfc3339("2026-10-25T10:00:01Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let recovery = restore_recovery_spec(&source, &p, &command(), typed, time).unwrap();
        assert_eq!(serde_json::to_value(&recovery.source).unwrap(), *policy);
        assert_eq!(recovery.source_database_id, "102");
        assert_eq!(recovery.base_backup_id, "20261025T100000");
        assert_eq!(recovery.credential_revision, 1);
        assert_eq!(recovery.timeline, 2);
        assert_eq!(recovery.target_time, time);
        assert_eq!(recovery.verification_sha256, "a".repeat(64));
    }
    #[test]
    fn historical_role_ciphertext_is_bound_to_its_policy_revision_and_new_target() {
        let cipher = SecretCipher::new(&"fixture-encryption-key-".repeat(3)).unwrap();
        let historical = DatabaseCredentials {
            application_password: "fixture-historical-app".into(),
            migration_password: "fixture-historical-migrate".into(),
        };
        let retained = cipher
            .seal(&historical, "database-backup-policy:107:credentials:1")
            .unwrap();
        let p = json!({"id":"107"});
        let sealed = reseal_restore_credentials(&cipher, &p, 1, &retained, "103").unwrap();
        let recovered: DatabaseCredentials = cipher.open(&sealed, "database:103:1").unwrap();
        assert_eq!(
            recovered.application_password,
            historical.application_password
        );
        assert_eq!(recovered.migration_password, historical.migration_password);
        for aad in ["database:103:2", "database:102:1"] {
            assert!(cipher.open::<DatabaseCredentials>(&sealed, aad).is_err());
        }
        assert!(reseal_restore_credentials(&cipher, &p, 2, &retained, "103").is_err());
        assert!(
            reseal_restore_credentials(&cipher, &json!({"id":"108"}), 1, &retained, "103").is_err()
        );
        let wrong = SecretCipher::new(&"different-fixture-key-".repeat(3)).unwrap();
        assert!(reseal_restore_credentials(&wrong, &p, 1, &retained, "103").is_err());
    }
}
