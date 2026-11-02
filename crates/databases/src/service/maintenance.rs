use super::control::{ReceiptScope, iso};
use super::*;
impl DatabaseService {
    pub(super) async fn maintenance(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        cmd: &Value,
    ) -> Result<Value> {
        let action = text(cmd, "action")?;
        let id = text(cmd, "databaseID")?;
        let preview = ["databases.deletionPreview", "databases.purgePreview"].contains(&action);
        let destructive =
            ["databases.delete", "databases.purge", "databases.reconcile"].contains(&action);
        let row = self
            .control_access(
                tx,
                actor,
                authority,
                id,
                !preview,
                preview || destructive,
                !preview,
            )
            .await?;
        self.protect_database(id, action)?;
        let backend = one(
            tx,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
            vec![text(&row, "backend_id")?.into()],
        )
        .await?;
        let cluster = text(&backend, "cluster_id")?;
        self.cluster_lock(tx, cluster).await?;
        let current=one(tx,"SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1 FOR UPDATE",vec![id.into()]).await?;
        if preview {
            let kind = if action == "databases.purgePreview" {
                "purge"
            } else {
                "delete"
            };
            let mut plan = json!({"databaseID":id,"name":row["name"],"revision":current["revision"],"kind":kind,"backendID":backend["id"],"cnpgUID":backend["cnpg_uid"],"scope":if row["kind"]=="shared"{"this-logical-database"}else{"this-installation"},"backupStatus":"not-configured","retainedUntil":iso(&current["retained_until"])?});
            let hash = hex::encode(Sha256::digest(
                format!("{}{random}", plan, random = token()?).as_bytes(),
            ));
            let stored=one(tx,"INSERT INTO webdock_auth.hosting_database_preview(database_id,session_id,kind,plan_hash,revision,expires_at) VALUES($1,$2,$3,$4,$5,now()+interval '5 minutes') ON CONFLICT(database_id,session_id,kind) DO UPDATE SET plan_hash=excluded.plan_hash,revision=excluded.revision,expires_at=excluded.expires_at RETURNING jsonb_build_object('expiresAt',expires_at) AS value",vec![id.into(),actor.session_id.clone().into(),kind.into(),hash.clone().into(),number(&current,"revision")?.into()]).await?;
            plan["planHash"] = json!(hash);
            plan["expiresAt"] = iso(&stored["expiresAt"])?;
            return Ok(plan);
        }
        let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
        if let Some(value) = prior {
            return Ok(value);
        }
        if current["revision"] != cmd["revision"] {
            return Err(conflict("Database changed. Refresh and try again."));
        }
        match action {
            "databases.requestCancellation"=>exec(tx,"UPDATE webdock_auth.hosting_database_order SET cancellation_requested_at=coalesce(cancellation_requested_at,now()) WHERE id=$1",vec![text(&current,"order_id")?.into()]).await?,
            "databases.reconcile"=>{
                let pending=optional(tx,"SELECT to_jsonb(o)||jsonb_build_object('backend_id',j.backend_id,'database_action',j.action,'database_revision',j.revision,'lease_token_hash',j.lease_token_hash,'checkpoint',j.checkpoint) AS value FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.database_id=$1 AND o.status='needs-reconciliation' ORDER BY o.created_at DESC LIMIT 1 FOR UPDATE OF o,j",vec![id.into()]).await?.ok_or_else(||conflict("Operator must inspect the unconfirmed installation before reconciliation."))?;
                let actual=one(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",vec![text(&pending,"backend_id")?.into()]).await?;
                let never_claimed=pending["id"]==current["operation_id"]&&pending["backend_id"]==row["backend_id"]&&pending["database_action"]=="create"&&pending["database_revision"]==1&&current["revision"]==1&&current["state"]=="needs-reconciliation"&&pending["generation"]==0&&pending["lease_until"].is_null()&&pending["lease_token_hash"].is_null()&&pending["checkpoint"].is_null()&&actual["cnpg_uid"].is_null()&&actual["revision"]==1&&actual["observed_revision"]==0&&actual["state"]=="provisioning";
                if !never_claimed&&(actual["cnpg_uid"].is_null()||pending["checkpoint"].is_null()){return Err(conflict("Operator must inspect the unconfirmed installation before reconciliation."))}
                if never_claimed{self.fresh_cluster(tx,cluster).await?;exec(tx,"UPDATE webdock_auth.hosting_database SET state='provisioning' WHERE id=$1",vec![id.into()]).await?;}else{
                    let mut checkpoint=pending["checkpoint"].clone();checkpoint["phase"]=json!(reconciliation_phase(text(&pending,"database_action")?,&checkpoint));checkpoint["cnpgUID"]=actual["cnpg_uid"].clone();exec(tx,"UPDATE webdock_auth.hosting_database_job SET checkpoint=$2 WHERE operation_id=$1",vec![text(&pending,"id")?.into(),checkpoint.into()]).await?;
                }
                exec(tx,"UPDATE webdock_auth.hosting_operation SET status='queued',actor_session=$2,subject=$3,desired=(coalesce(desired,'{}'::jsonb)-'consoleIdentity')||jsonb_build_object('nativeAuthority',$4::jsonb) WHERE id=$1",vec![text(&pending,"id")?.into(),actor.session_id.clone().into(),actor.user_id.clone().into(),serde_json::to_value(authority).map_err(|_|invalid())?.into()]).await?;
            },
            _=>{
                self.fresh_cluster(tx,cluster).await?;
                if optional(tx,"SELECT jsonb_build_object('busy',true) AS value FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.database_id=$1 AND o.status IN('queued','running','needs-reconciliation') LIMIT 1",vec![id.into()]).await?.is_some(){return Err(conflict("Finish or reconcile the current database operation first."))}
                let (intent,state)=match action {
                    "databases.pause"=>{if !["ready","degraded"].contains(&text(&current,"state")?){return Err(conflict("Only a ready database can be paused."))}("pause","pausing")},
                    "databases.resume"=>{if current["state"]!="paused"{return Err(conflict("Only a paused database can be resumed."))}("resume","resuming")},
                    "databases.restart"=>{if row["kind"]=="shared"{return Err(conflict("Shared databases cannot restart their pool."))}
                    if !["ready","degraded"].contains(&text(&current,"state")?){return Err(conflict("Only a ready instance can be restarted."))}("restart","updating")},
                    "databases.rotateCredentials"=>{if current["state"]!="ready"{return Err(conflict("Only a ready database can rotate credentials."))}let revision=number(&current,"credential_revision")?.checked_add(1).ok_or_else(invalid)?;let credentials=DatabaseCredentials{application_password:token()?,migration_password:token()?};let sealed=self.cipher.seal(&credentials,&format!("database:{id}:{revision}")).map_err(|_|unavailable())?;exec(tx,"UPDATE webdock_auth.hosting_database SET credential_revision=$2,credentials_encrypted=$3 WHERE id=$1",vec![id.into(),revision.into(),sealed.into()]).await?;("rotateCredentials","updating")},
                    "databases.delete"|"databases.purge"=>{
                        let kind=if action=="databases.delete"{"delete"}else{"purge"};let proof=optional(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_preview p WHERE database_id=$1 AND session_id=$2 AND kind=$3 FOR UPDATE",vec![id.into(),actor.session_id.clone().into(),kind.into()]).await?.ok_or_else(||conflict("Refresh and confirm the database deletion preview."))?;
                        if proof["plan_hash"]!=cmd["planHash"]||proof["revision"]!=current["revision"]||cmd["confirmName"]!=row["name"]||!future(&proof["expires_at"]){return Err(conflict("Refresh and confirm the database deletion preview."))}
                        if kind=="purge"&&(current["state"]!="retained"||current["retained_until"].is_null()||future(&current["retained_until"])){return Err(conflict("Database retention has not expired."))}
                        if kind=="delete"&&["retained","purged"].contains(&text(&current,"state")?){return Err(conflict("Database is already retained or purged."))}
                        exec(tx,"DELETE FROM webdock_auth.hosting_database_preview WHERE database_id=$1",vec![id.into()]).await?;(kind,if kind=="delete"{"deleting"}else{"retained"})
                    },_=>return Err(conflict("Database maintenance requires an accepted matching upgrade order.")),
                };
                let revision=number(&current,"revision")?.checked_add(1).ok_or_else(invalid)?;
                exec(tx,"UPDATE webdock_auth.hosting_database SET revision=$3,state=$2 WHERE id=$1",vec![id.into(),state.into(),revision.into()]).await?;
                self.enqueue_database(tx,actor,authority,text(&row,"backend_id")?,Some(id),intent,revision,true).await?;
            }
        }
        let result = self.managed_view(tx, id).await?;
        self.record_control(
            tx,
            actor,
            cmd,
            &hash,
            &result,
            ReceiptScope {
                customer: row["customer_id"].as_str(),
                project: row["project_id"].as_str(),
                cluster: Some(cluster),
            },
        )
        .await?;
        self.control_audit(tx, actor, action, id).await?;
        Ok(result)
    }
    pub(super) async fn migration_maintenance(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        cmd: &Value,
    ) -> Result<Value> {
        let action = text(cmd, "action")?;
        if action == "databases.migrations.list" {
            self.control_access(
                tx,
                actor,
                authority,
                text(cmd, "databaseID")?,
                false,
                true,
                false,
            )
            .await?;
            let mut rows=all(tx,"SELECT jsonb_build_object('id',id,'sourceBackendID',source_backend_id,'targetBackendID',target_backend_id,'state',state,'copySHA256',copy_sha256,'retainedUntil',retained_until) AS value FROM webdock_auth.hosting_database_migration WHERE database_id=$1 ORDER BY created_at DESC",vec![text(cmd,"databaseID")?.into()]).await?;
            for row in &mut rows {
                row["retainedUntil"] = iso(&row["retainedUntil"])?;
            }
            return Ok(json!(rows));
        }
        let writing = action == "databases.purgeMigrationSource";
        self.authorize_mode(tx, actor, None, None, writing, true, authority, writing)
            .await?;
        let migration=one(tx,"SELECT to_jsonb(m)||jsonb_build_object('cluster_id',b.cluster_id,'cnpg_uid',b.cnpg_uid,'kind',b.kind) AS value FROM webdock_auth.hosting_database_migration m JOIN webdock_auth.hosting_database_backend b ON b.id=m.source_backend_id WHERE m.id=$1",vec![text(cmd,"migrationID")?.into()]).await?;
        let id = text(&migration, "database_id")?;
        let d = self
            .control_access(tx, actor, authority, id, writing, true, writing)
            .await?;
        let cluster = text(&migration, "cluster_id")?;
        self.cluster_lock(tx, cluster).await?;
        let kind = format!("migration:{}", text(&migration, "id")?);
        if !writing {
            let mut plan = json!({"migrationID":migration["id"],"databaseID":d["id"],"name":d["name"],"revision":d["revision"],"sourceBackendID":migration["source_backend_id"],"targetBackendID":migration["target_backend_id"],"copySHA256":migration["copy_sha256"],"retainedUntil":iso(&migration["retained_until"])?});
            let hash = hex::encode(Sha256::digest(
                format!("{}{random}", plan, random = token()?).as_bytes(),
            ));
            exec(tx,"INSERT INTO webdock_auth.hosting_database_preview(database_id,session_id,kind,plan_hash,revision,expires_at) VALUES($1,$2,$3,$4,$5,now()+interval '5 minutes') ON CONFLICT(database_id,session_id,kind) DO UPDATE SET plan_hash=excluded.plan_hash,revision=excluded.revision,expires_at=excluded.expires_at",vec![id.into(),actor.session_id.clone().into(),kind.into(),hash.clone().into(),number(&d,"revision")?.into()]).await?;
            plan["planHash"] = json!(hash);
            return Ok(plan);
        }
        let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
        if let Some(value) = prior {
            return Ok(value);
        }
        let preview=optional(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_preview p WHERE database_id=$1 AND session_id=$2 AND kind=$3 FOR UPDATE",vec![id.into(),actor.session_id.clone().into(),kind.clone().into()]).await?.ok_or_else(||conflict("Refresh and confirm the retained-source deletion preview."))?;
        if preview["plan_hash"] != cmd["planHash"]
            || preview["revision"] != d["revision"]
            || cmd["revision"] != d["revision"]
            || cmd["confirmName"] != d["name"]
            || !future(&preview["expires_at"])
        {
            return Err(conflict(
                "Refresh and confirm the retained-source deletion preview.",
            ));
        }
        if migration["state"] != "retained"
            || migration["copy_sha256"].as_str().is_none()
            || migration["retained_until"].is_null()
            || future(&migration["retained_until"])
            || d["backend_id"] == migration["source_backend_id"]
            || d["pending_backend_id"] == migration["source_backend_id"]
        {
            return Err(conflict(
                "The retained source is not eligible for deletion.",
            ));
        }
        if optional(tx,"SELECT jsonb_build_object('busy',true) AS value FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE(j.database_id=$1 OR j.backend_id=$2 OR j.source_backend_id=$2) AND o.status IN('queued','running','needs-reconciliation') LIMIT 1",vec![id.into(),text(&migration,"source_backend_id")?.into()]).await?.is_some(){return Err(conflict("Finish or reconcile the current database operation first."))}
        let operation = self
            .enqueue_database(
                tx,
                actor,
                authority,
                text(&migration, "source_backend_id")?,
                Some(id),
                "purgeSource",
                number(&d, "revision")?,
                false,
            )
            .await?;
        exec(
            tx,
            "UPDATE webdock_auth.hosting_database_job SET migration_id=$2 WHERE operation_id=$1",
            vec![operation.clone().into(), text(&migration, "id")?.into()],
        )
        .await?;
        exec(tx,"DELETE FROM webdock_auth.hosting_database_preview WHERE database_id=$1 AND session_id=$2 AND kind=$3",vec![id.into(),actor.session_id.clone().into(),kind.into()]).await?;
        let result = json!({"migrationID":migration["id"],"operationID":operation});
        self.record_control(
            tx,
            actor,
            cmd,
            &hash,
            &result,
            ReceiptScope {
                customer: d["customer_id"].as_str(),
                project: d["project_id"].as_str(),
                cluster: Some(cluster),
            },
        )
        .await?;
        self.control_audit(tx, actor, action, text(&migration, "id")?)
            .await?;
        Ok(result)
    }
}
fn future(value: &Value) -> bool {
    value
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|date| date > chrono::Utc::now())
}

fn reconciliation_phase(action: &str, checkpoint: &serde_json::Value) -> &'static str {
    if action == "restore"
        && matches!(
            checkpoint["phase"].as_str(),
            Some("applied" | "configuring")
        )
    {
        "applied"
    } else {
        "observing"
    }
}
#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn unfinished_recovery_reconciles_through_configuration_before_sql_readiness() {
        for phase in ["applied", "configuring"] {
            assert_eq!(
                reconciliation_phase("restore", &json!({"phase":phase})),
                "applied"
            );
        }
        assert_eq!(
            reconciliation_phase("restore", &json!({"phase":"observing"})),
            "observing"
        );
        assert_eq!(
            reconciliation_phase("create", &json!({"phase":"applied"})),
            "observing"
        );
    }
}
