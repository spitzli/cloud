use super::*;
use subtle::ConstantTimeEq;
use webdock_hosting::AgentIdentity;
mod completion;
mod delivery;
mod observations;
struct AuthorityGuard {
    auth: Option<DatabaseTransaction>,
    business: Option<DatabaseTransaction>,
}
impl AuthorityGuard {
    async fn commit(self) -> Result<()> {
        if let Some(tx) = self.business {
            tx.commit().await?;
        }
        if let Some(tx) = self.auth {
            tx.commit().await?;
        }
        Ok(())
    }
}
const JOB: &str = "SELECT to_jsonb(o)||jsonb_build_object('backend_id',j.backend_id,'database_id',j.database_id,'database_action',j.action,'database_revision',j.revision,'lease_token_hash',j.lease_token_hash,'checkpoint',j.checkpoint,'migration_id',j.migration_id,'source_backend_id',j.source_backend_id,'backup_policy_id',j.backup_policy_id,'backup_policy_revision',j.backup_policy_revision,'recovery',j.recovery) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_database_job j ON j.operation_id=o.id";
fn lease_conflict() -> Error {
    conflict("Database operation requires reconciliation or belongs to another agent.")
}
fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn checkpoint_digest(data: &DatabaseCheckpoint) -> Result<String> {
    let mut value = serde_json::to_value(data).map_err(|_| invalid())?;
    value
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("leaseToken");
    Ok(digest(
        &webdock_hosting::canonical_command(&value).map_err(|_| invalid())?,
    ))
}
fn completion_digest(data: &DatabaseCompletion) -> Result<String> {
    let mut value = serde_json::to_value(data).map_err(|_| invalid())?;
    value
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("leaseToken");
    Ok(digest(
        &webdock_hosting::canonical_command(&value).map_err(|_| invalid())?,
    ))
}
impl DatabaseService {
    async fn agent_scope(&self, tx: &DatabaseTransaction, agent: &AgentIdentity) -> Result<()> {
        let scope = one(tx, "SELECT webdock_rls.request_scope() AS value", vec![]).await?;
        if scope["kind"] != "system"
            || scope["purpose"] != "agent"
            || scope["clusterID"] != agent.cluster_id()
        {
            return Err(denied());
        }
        one(tx,"SELECT jsonb_build_object('id',cluster_id) AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1 AND generation=$2 AND NOT revoked FOR SHARE",vec![agent.cluster_id().into(),agent.generation().into()]).await?;
        Ok(())
    }
    async fn qualified_agent(&self, tx: &DatabaseTransaction, agent: &AgentIdentity) -> Result<()> {
        self.agent_scope(tx, agent).await?;
        self.installed_scoped(tx, false).await?;
        self.fresh_cluster(tx, agent.cluster_id()).await?;
        Ok(())
    }
    fn database_lease_token(&self, id: &str, generation: i32) -> Result<String> {
        let secret = self
            .management
            .as_ref()
            .and_then(|c| c.lease_secret.as_deref())
            .ok_or_else(unavailable)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| unavailable())?;
        mac.update(format!("database-lease:{id}:{generation}").as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }
    async fn operation_authority(
        &self,
        tx: &DatabaseTransaction,
        op: &Value,
    ) -> Result<AuthorityGuard> {
        if !op["backup_policy_id"].is_null() {
            self.backup_policy_authority(tx, op).await?;
            if policy_authority_only(op) {
                return Ok(AuthorityGuard {
                    auth: None,
                    business: None,
                });
            }
        }
        let plugin=one(tx,"SELECT jsonb_build_object('binding',webdock_rls.plugin_setup_operation_binding($1),'allowed',webdock_rls.plugin_setup_operation_live($1)) AS value",vec![text(op,"id")?.into()]).await?;
        if !plugin["binding"].is_null() && plugin["allowed"] != true {
            return Err(denied());
        }
        if !op["desired"]["consoleIdentity"].is_null() {
            return Err(denied());
        }
        let authority = match crate::database_authority::DatabaseAuthority::from_persisted(
            op["desired"]["nativeAuthority"].clone(),
        )
        .map_err(|_| denied())?
        {
            crate::database_authority::DatabaseAuthority::BrowserStored(browser) => browser,
            // MCP execution needs the unmaterialized early0056 epoch caller. No fallback.
            crate::database_authority::DatabaseAuthority::Mcp(_) => return Err(denied()),
        };
        let (auth, principal) = self
            .verify(text(op, "subject")?, text(op, "actor_session")?, &authority)
            .await?;
        let business = self.begin_actor(&principal).await?;
        let project = op["project_id"].as_str();
        let access = self
            .authorize_mode(
                &business,
                &principal,
                project,
                None,
                true,
                project.is_none(),
                &authority,
                true,
            )
            .await?;
        if !op["customer_id"].is_null() && access["customerID"] != op["customer_id"] {
            return Err(denied());
        }
        Ok(AuthorityGuard {
            auth: Some(auth),
            business: Some(business),
        })
    }
    async fn mark_reconciliation(&self, tx: &DatabaseTransaction, op: &Value) -> Result<()> {
        exec(
            tx,
            "UPDATE webdock_auth.hosting_operation SET status='needs-reconciliation' WHERE id=$1",
            vec![text(op, "id")?.into()],
        )
        .await?;
        if !op["backup_policy_id"].is_null() {
            exec(tx,"UPDATE webdock_auth.hosting_database_backup_policy SET observed_at=NULL WHERE id=$1 AND revision=$2",vec![text(op,"backup_policy_id")?.into(),number(op,"backup_policy_revision")?.into()]).await?;
            return Ok(());
        }
        if let Some(database) = op["database_id"]
            .as_str()
            .filter(|_| op["database_action"] != "purgeSource")
        {
            exec(
                tx,
                "UPDATE webdock_auth.hosting_database SET state='needs-reconciliation' WHERE id=$1 AND operation_id=$2 AND revision=$3",
                vec![database.into(), text(op,"id")?.into(), number(op,"database_revision")?.into()],
            )
            .await?;
        }
        Ok(())
    }
    async fn lease_job(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        lease: &DatabaseLease,
    ) -> Result<Value> {
        lease.validate()?;
        self.qualified_agent(tx, agent).await?;
        self.cluster_lock(tx, agent.cluster_id()).await?;
        let op = one(
            tx,
            &format!("{JOB} WHERE o.id=$1 AND o.cluster_id=$2 FOR UPDATE OF o,j"),
            vec![lease.operation_id.clone().into(), agent.cluster_id().into()],
        )
        .await?;
        self.verify_job_lease(&op, agent, lease, false)?;
        Ok(op)
    }
    fn verify_job_lease(
        &self,
        op: &Value,
        agent: &AgentIdentity,
        lease: &DatabaseLease,
        terminal: bool,
    ) -> Result<()> {
        if lease.generation != agent.generation()
            || !op["lease_token_hash"].as_str().is_some_and(|hash| {
                bool::from(hash.as_bytes().ct_eq(digest(&lease.lease_token).as_bytes()))
            })
        {
            return Err(lease_conflict());
        }
        if !terminal
            && (op["status"] != "running"
                || op["lease_until"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .is_none_or(|date| date <= chrono::Utc::now()))
        {
            return Err(lease_conflict());
        }
        Ok(())
    }
    /// Caller obtains the transaction from HostingService::begin_authenticated_agent.
    pub async fn claim_database_operation(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
    ) -> Result<Option<DatabaseClaim>> {
        self.agent_scope(&tx, agent).await?;
        let capability=one(&tx,"SELECT jsonb_build_object('enabled',observation#>'{capabilities,databaseVersion}'='1'::jsonb) AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![agent.cluster_id().into()]).await?;
        if capability["enabled"] != true {
            tx.commit().await?;
            return Ok(None);
        }
        self.qualified_agent(&tx, agent).await?;
        self.cluster_lock(&tx, agent.cluster_id()).await?;
        exec(&tx,"WITH expired AS(UPDATE webdock_auth.hosting_operation SET status='needs-reconciliation' WHERE cluster_id=$1 AND action LIKE 'databases.%' AND status='running' AND lease_until<now() RETURNING id) UPDATE webdock_auth.hosting_database d SET state='needs-reconciliation' FROM webdock_auth.hosting_database_job j JOIN expired e ON e.id=j.operation_id WHERE d.id=j.database_id AND j.action NOT IN('purgeSource','backup','configureBackup')",vec![agent.cluster_id().into()]).await?;
        self.enqueue_backup_observation(&tx, agent.cluster_id())
            .await?;
        let Some(mut op)=optional(&tx,&format!("{JOB} WHERE o.cluster_id=$1 AND (o.status='running' OR(o.status='queued' AND NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_database_job busy JOIN webdock_auth.hosting_operation bo ON bo.id=busy.operation_id WHERE(busy.backend_id IN(j.backend_id,j.source_backend_id) OR busy.source_backend_id IN(j.backend_id,j.source_backend_id)) AND bo.status IN('running','needs-reconciliation')))) ORDER BY(o.status='running') DESC,o.created_at,o.id FOR UPDATE OF o,j SKIP LOCKED LIMIT 1"),vec![agent.cluster_id().into()]).await?else{tx.commit().await?;return Ok(None)};
        let Ok(guard) = self.operation_authority(&tx, &op).await else {
            self.mark_reconciliation(&tx, &op).await?;
            tx.commit().await?;
            return Ok(None);
        };
        let first = op["status"] == "queued";
        let unissued = first
            && op["generation"] == 0
            && op["lease_until"].is_null()
            && op["checkpoint"].is_null()
            && op["lease_token_hash"].is_null()
            && (op["desired"]["nativeIssuedSpecs"].is_null()
                || op["desired"]["nativeIssuedSpecs"]
                    .as_array()
                    .is_some_and(Vec::is_empty));
        let original = op.clone();
        if first {
            op["generation"] = json!(
                number(&op, "generation")?
                    .checked_add(1)
                    .ok_or_else(lease_conflict)?
            );
            op["lease_until"] =
                json!((chrono::Utc::now() + chrono::Duration::seconds(300)).to_rfc3339());
        }
        let admitted = async {
            let claim = self.database_claim(&tx, agent, &op, first).await?;
            claim.validate(chrono::Utc::now())?;
            Ok::<_, Error>(claim)
        }
        .await;
        let mut claim = match admitted {
            Ok(claim) => claim,
            Err(error) if [400, 409].contains(&error.status) => {
                if unissued && !original["backup_policy_id"].is_null() {
                    // No envelope or lease was ever issued. Retire the invalid
                    // control request, retaining the installation reservation.
                    exec(&tx,"UPDATE webdock_auth.hosting_operation SET status='failed',lease_until=NULL,result=$2 WHERE id=$1",vec![text(&original,"id")?.into(),json!({"error":"invalid-database-contract","externalEffects":"not-issued"}).into()]).await?;
                } else {
                    self.mark_reconciliation(&tx, &original).await?;
                }
                tx.commit().await?;
                guard.commit().await?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if first {
            exec(&tx,"UPDATE webdock_auth.hosting_operation SET status='running',generation=$2,lease_until=$3 WHERE id=$1",vec![text(&op,"id")?.into(),number(&op,"generation")?.into(),claim.lease_until.into()]).await?;
            exec(&tx,"UPDATE webdock_auth.hosting_database_job SET lease_token_hash=$2 WHERE operation_id=$1",vec![text(&op,"id")?.into(),digest(&claim.lease.lease_token).into()]).await?;
        } else {
            let renewed=one(&tx,"UPDATE webdock_auth.hosting_operation SET lease_until=CASE WHEN lease_until<now()+interval '60 seconds' THEN now()+interval '300 seconds' ELSE lease_until END WHERE id=$1 AND status='running' AND lease_until>now() RETURNING jsonb_build_object('lease_until',lease_until) AS value",vec![text(&op,"id")?.into()]).await?;
            op["lease_until"] = renewed["lease_until"].clone();
            claim.lease_until =
                chrono::DateTime::parse_from_rfc3339(text(&renewed, "lease_until")?)
                    .map_err(|_| lease_conflict())?
                    .with_timezone(&chrono::Utc);
        }
        if !first
            && op["lease_token_hash"].as_str() != Some(digest(&claim.lease.lease_token).as_str())
        {
            self.mark_reconciliation(&tx, &op).await?;
            tx.commit().await?;
            guard.commit().await?;
            return Ok(None);
        }
        self.record_issued_database_spec(&tx, &op, &claim).await?;
        tx.commit().await?;
        guard.commit().await?;
        Ok(Some(claim))
    }
    async fn record_issued_database_spec(
        &self,
        tx: &DatabaseTransaction,
        op: &Value,
        claim: &DatabaseClaim,
    ) -> Result<()> {
        let fingerprint = claim.spec.fingerprint()?;
        let mut desired = op["desired"].as_object().cloned().unwrap_or_default();
        let mut issued = desired
            .get("nativeIssuedSpecs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !issued.iter().any(|v| v["fingerprint"] == fingerprint) {
            if issued.len() >= 64 {
                issued.remove(0);
            }
            issued.push(json!({"fingerprint":fingerprint,"tokenHash":digest(&claim.lease.lease_token),"agentGeneration":claim.spec.agent_generation,"operationGeneration":claim.spec.operation_generation,"spec":claim.spec}));
            desired.insert("nativeIssuedSpecs".into(), json!(issued));
            exec(
                tx,
                "UPDATE webdock_auth.hosting_operation SET desired=$2 WHERE id=$1",
                vec![text(op, "id")?.into(), Value::Object(desired).into()],
            )
            .await?;
        }
        Ok(())
    }
    async fn issued_database_callback(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &DatabaseJobSpec,
        lease: &DatabaseLease,
    ) -> Result<Value> {
        spec.validate()?;
        lease.validate()?;
        self.agent_scope(tx, agent).await?;
        if spec.operation_id != lease.operation_id
            || spec.cluster_id != agent.cluster_id()
            || spec.agent_generation != agent.generation()
            || lease.generation != spec.agent_generation
        {
            return Err(lease_conflict());
        }
        // Match Poll and lifecycle lock order before taking either operation row.
        self.cluster_lock(tx, agent.cluster_id()).await?;
        let op = one(
            tx,
            &format!("{JOB} WHERE o.id=$1 AND o.cluster_id=$2 FOR UPDATE OF o,j"),
            vec![spec.operation_id.clone().into(), agent.cluster_id().into()],
        )
        .await?;
        let fingerprint = spec.fingerprint()?;
        let token_hash = digest(&lease.lease_token);
        if !op["desired"]["nativeIssuedSpecs"]
            .as_array()
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row["fingerprint"] == fingerprint
                        && row["tokenHash"] == token_hash
                        && row["agentGeneration"] == spec.agent_generation
                        && row["operationGeneration"] == spec.operation_generation
                })
            })
        {
            return Err(lease_conflict());
        }
        Ok(op)
    }
    /// Exact committed checkpoint delivery can be acknowledged after expiry without renewing execution.
    pub async fn validate_database_checkpoint_in(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &DatabaseJobSpec,
        data: &DatabaseCheckpoint,
    ) -> Result<()> {
        data.validate()?;
        let op = self
            .issued_database_callback(tx, agent, spec, &data.lease)
            .await?;
        if !checkpoint_phase_allowed(spec.phase, data.phase) {
            return Err(lease_conflict());
        }
        if checkpoint_recorded(&op, agent, data)? {
            return Ok(());
        }
        self.validate_database_callback_in(
            tx,
            agent,
            spec,
            &data.lease,
            DatabaseCallback::Checkpoint(data.phase),
        )
        .await
    }
    /// Native gRPC calls this before invoking the lease lifecycle method with the same transaction.
    pub async fn validate_database_callback_in(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        spec: &DatabaseJobSpec,
        lease: &DatabaseLease,
        callback: DatabaseCallback,
    ) -> Result<()> {
        let op = self
            .issued_database_callback(tx, agent, spec, lease)
            .await?;
        if let DatabaseCallback::Completion(outcome) = callback {
            if outcome == DatabaseCompletionOutcome::Succeeded
                && spec.phase != DatabaseExecutionPhase::Observe
            {
                return Err(lease_conflict());
            }
            return Ok(());
        }
        self.verify_job_lease(&op, agent, lease, false)?;
        if number(&op, "generation")? != spec.operation_generation {
            return Err(lease_conflict());
        }
        let checkpoint = op["checkpoint"]["phase"].as_str();
        let phase_allowed = match callback {
            DatabaseCallback::Guard => match spec.phase {
                DatabaseExecutionPhase::Apply => checkpoint.is_none(),
                DatabaseExecutionPhase::Configure => {
                    [Some("applied"), Some("configuring")].contains(&checkpoint)
                }
                DatabaseExecutionPhase::Observe => checkpoint == Some("observing"),
                DatabaseExecutionPhase::Uncertain => false,
            },
            DatabaseCallback::Checkpoint(phase) => checkpoint_phase_allowed(spec.phase, phase),
            DatabaseCallback::Completion(_) => unreachable!(),
        };
        if !phase_allowed {
            return Err(lease_conflict());
        }
        let backend = one(
            tx,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
            vec![text(&op, "backend_id")?.into()],
        )
        .await?;
        if spec.backend.id != text(&backend, "id")?
            || spec.backend.revision != number(&backend, "revision")?
            || serde_json::to_value(&spec.backend.resources).map_err(|_| invalid())?
                != backend["profile"]
        {
            return Err(lease_conflict());
        }
        if let Some(policy) = &spec.backup {
            let p=one(tx,"SELECT spec AS value FROM webdock_auth.hosting_database_backup_policy WHERE id=$1 AND revision=$2",vec![policy.id.clone().into(),policy.revision.into()]).await?;
            if serde_json::to_value(policy).map_err(|_| invalid())? != p {
                return Err(lease_conflict());
            }
            self.backup_policy_authority(tx, &op).await?;
        }
        if let Some(database) = op["database_id"].as_str() {
            let target = one(
                tx,
                "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
                vec![database.into()],
            )
            .await?;
            if number(&target, "revision")? != spec.revision {
                return Err(lease_conflict());
            }
            if let Some(spec_db) = &spec.database {
                let revision = if op["database_action"] == "purgeSource" {
                    number(&one(tx,"SELECT to_jsonb(m) AS value FROM webdock_auth.hosting_database_migration m WHERE id=$1",vec![text(&op,"migration_id")?.into()]).await?,"source_credential_revision")?
                } else {
                    number(&target, "credential_revision")?
                };
                if spec_db.id != database
                    || spec_db.name != text(&target, "database_name")?
                    || spec_db.credential_revision != revision
                {
                    return Err(lease_conflict());
                }
            }
        }
        Ok(())
    }
    async fn database_claim(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        op: &Value,
        first: bool,
    ) -> Result<DatabaseClaim> {
        let b = one(
            tx,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
            vec![text(op, "backend_id")?.into()],
        )
        .await?;
        let d = if let Some(id) = op["database_id"].as_str() {
            one(tx,"SELECT to_jsonb(d)||jsonb_build_object('plan',p.plan) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=coalesce(d.pending_plan_revision_id,d.plan_revision_id) WHERE d.id=$1",vec![id.into()]).await?
        } else {
            Value::Null
        };
        let action = text(op, "database_action")?;
        let retained = if action == "purgeSource" {
            one(tx,"SELECT to_jsonb(m) AS value FROM webdock_auth.hosting_database_migration m WHERE id=$1 AND source_backend_id=$2",vec![text(op,"migration_id")?.into(),text(&b,"id")?.into()]).await?
        } else {
            Value::Null
        };
        let migration = if ["prepareMigration", "migrate"].contains(&action) {
            one(tx,"SELECT to_jsonb(m)||jsonb_build_object('source_uid',b.cnpg_uid,'source_host',b.host,'source_kind',b.kind) AS value FROM webdock_auth.hosting_database_migration m JOIN webdock_auth.hosting_database_backend b ON b.id=m.source_backend_id WHERE m.database_id=$1 AND m.target_backend_id=$2 AND m.state IN('preparing','copying','needs-reconciliation')",vec![text(&d,"id")?.into(),text(&b,"id")?.into()]).await?
        } else {
            Value::Null
        };
        let mut members=all(tx,"SELECT jsonb_build_object('id',id,'projectID',project_id,'databaseName',database_name,'enabled',state NOT IN('paused','pausing','retained','deleting')) AS value FROM webdock_auth.hosting_database WHERE backend_id=$1 AND state<>'purged' ORDER BY id LIMIT 100",vec![text(&b,"id")?.into()]).await?;
        if action == "purgeSource" {
            members.retain(|m| m["id"] != d["id"]);
            members.push(json!({"id":d["id"],"projectID":d["project_id"],"databaseName":d["database_name"],"enabled":false}));
        }
        if action == "prepareMigration" {
            members.clear()
        }
        if action == "migrate" {
            members = vec![
                json!({"id":d["id"],"projectID":d["project_id"],"databaseName":d["database_name"],"enabled":true}),
            ]
        }
        let phase = if first && op["checkpoint"].is_null() {
            "apply"
        } else if op["checkpoint"]["phase"] == "applied" {
            "configure"
        } else if op["checkpoint"]["phase"] == "observing" {
            "observe"
        } else {
            "uncertain"
        };
        let credential_revision = if retained.is_null() {
            d["credential_revision"].clone()
        } else {
            retained["source_credential_revision"].clone()
        };
        let database = if !d.is_null() && action != "prepareMigration" {
            json!({"id":d["id"],"kind":if action=="purgeSource"{if b["kind"]=="shared-pool"{"shared"}else{"dedicated"}}else if action=="migrate"{"dedicated"}else{text(&d,"kind")?},"name":d["database_name"],"revision":d["revision"],"credentialRevision":credential_revision,"connectionLimit":d["plan"]["connectionLimit"]})
        } else {
            Value::Null
        };
        let credentials = if database.is_null() {
            None
        } else {
            let encrypted = if retained.is_null() {
                text(&d, "credentials_encrypted")?
            } else {
                text(&retained, "source_credentials_encrypted")?
            };
            Some(
                self.cipher
                    .open(
                        encrypted,
                        &format!(
                            "database:{}:{}",
                            text(&d, "id")?,
                            credential_revision.as_i64().ok_or_else(lease_conflict)?
                        ),
                    )
                    .map_err(|_| unavailable())?,
            )
        };
        let migration_spec = if action == "migrate" {
            json!({"id":migration["id"],"source":{"id":migration["source_backend_id"],"namespace":format!("wd-db-{}",text(&migration,"source_backend_id")?),"cnpgUID":migration["source_uid"],"host":migration["source_host"],"kind":migration["source_kind"]},"jobUID":migration["job_uid"],"copySHA256":migration["copy_sha256"]})
        } else {
            Value::Null
        };
        let transfer_password = if migration_spec.is_null() {
            None
        } else {
            let secret: Value = self
                .cipher
                .open(
                    text(&migration, "transfer_credentials_encrypted")?,
                    &format!("database-migration:{}", text(&migration, "id")?),
                )
                .map_err(|_| unavailable())?;
            Some(text(&secret, "password")?.to_owned())
        };
        let backup_policy = if !op["backup_policy_id"].is_null() {
            Some(one(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_backup_policy p WHERE id=$1 AND revision=$2",vec![text(op,"backup_policy_id")?.into(),number(op,"backup_policy_revision")?.into()]).await?)
        } else {
            None
        };
        let object_store = if !op["recovery"].is_null() {
            let id = text(&op["recovery"]["source"]["destination"], "id")?;
            let destination=one(tx,"SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database_backup_destination d WHERE id=$1",vec![id.into()]).await?;
            Some(
                self.cipher
                    .open::<DatabaseObjectStoreMaterial>(
                        text(&destination, "recovery_encrypted")?,
                        &format!("database-backup-destination:{id}:recovery:1"),
                    )
                    .map_err(|_| unavailable())?,
            )
        } else if let Some(p) = &backup_policy {
            let destination=one(tx,"SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database_backup_destination d WHERE id=$1",vec![text(p,"destination_id")?.into()]).await?;
            Some(
                self.cipher
                    .open::<DatabaseObjectStoreMaterial>(
                        text(&destination, "write_encrypted")?,
                        &format!(
                            "database-backup-destination:{}:write:1",
                            text(p, "destination_id")?
                        ),
                    )
                    .map_err(|_| unavailable())?,
            )
        } else {
            None
        };
        let mut spec:DatabaseJobSpec=serde_json::from_value(json!({"version":1,"operationID":op["id"],"agentGeneration":agent.generation(),"operationGeneration":op["generation"],"clusterID":agent.cluster_id(),"action":if action=="purgeSource"{"purge"}else{action},"phase":phase,"revision":op["database_revision"],"previousPodUID":op["checkpoint"]["previousPodUID"],"backend":{"id":b["id"],"kind":b["kind"],"namespace":format!("wd-db-{}",text(&b,"id")?),"node":b["node"],"poolID":b["pool_id"],"revision":b["revision"],"cnpgUID":b["cnpg_uid"],"resources":b["profile"],"host":b["host"],"directPort":b["direct_port"],"pooledPort":b["pooled_port"]},"database":database,"members":members,"migration":migration_spec})).map_err(|_|lease_conflict())?;
        spec.recovery = if op["recovery"].is_null() {
            None
        } else {
            Some(serde_json::from_value(op["recovery"].clone()).map_err(|_| lease_conflict())?)
        };
        spec.backup = backup_policy
            .as_ref()
            .map(|p| serde_json::from_value(p["spec"].clone()).map_err(|_| lease_conflict()))
            .transpose()?;
        let lease = DatabaseLease {
            operation_id: text(op, "id")?.into(),
            generation: agent.generation(),
            lease_token: self.database_lease_token(text(op, "id")?, number(op, "generation")?)?,
        };
        if !first
            && op["checkpoint"].is_null()
            && let Some(saved) = op["desired"]["nativeIssuedSpecs"]
                .as_array()
                .and_then(|rows| {
                    rows.iter().rev().find(|entry| {
                        entry["agentGeneration"] == agent.generation()
                            && entry["operationGeneration"] == op["generation"]
                            && entry["spec"]["phase"] == "apply"
                    })
                })
        {
            let original: DatabaseJobSpec =
                serde_json::from_value(saved["spec"].clone()).map_err(|_| lease_conflict())?;
            self.validate_database_callback_in(
                tx,
                agent,
                &original,
                &lease,
                DatabaseCallback::Guard,
            )
            .await?;
            spec = original;
        }
        Ok(DatabaseClaim {
            spec,
            lease,
            lease_until: chrono::DateTime::parse_from_rfc3339(text(op, "lease_until")?)
                .map_err(|_| lease_conflict())?
                .with_timezone(&chrono::Utc),
            material: DatabaseMaterial {
                credentials,
                transfer_password,
                object_store,
            },
        })
    }
    pub async fn guard_database_operation(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        lease: &DatabaseLease,
    ) -> Result<()> {
        let op = self.lease_job(&tx, agent, lease).await?;
        let guard = self.operation_authority(&tx, &op).await?;
        tx.commit().await?;
        guard.commit().await
    }
    pub async fn checkpoint_database_operation(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        data: DatabaseCheckpoint,
    ) -> Result<()> {
        data.validate()?;
        self.agent_scope(&tx, agent).await?;
        self.cluster_lock(&tx, agent.cluster_id()).await?;
        let recorded = one(
            &tx,
            &format!("{JOB} WHERE o.id=$1 AND o.cluster_id=$2 FOR UPDATE OF o,j"),
            vec![
                data.lease.operation_id.clone().into(),
                agent.cluster_id().into(),
            ],
        )
        .await?;
        if checkpoint_recorded(&recorded, agent, &data)? {
            if self
                .verify_job_lease(&recorded, agent, &data.lease, false)
                .is_ok()
            {
                let current = async {
                    self.qualified_agent(&tx, agent).await?;
                    self.operation_authority(&tx, &recorded).await
                }
                .await;
                if let Ok(guard) = current {
                    exec(&tx,"UPDATE webdock_auth.hosting_operation SET lease_until=now()+interval '300 seconds' WHERE id=$1",vec![text(&recorded,"id")?.into()]).await?;
                    tx.commit().await?;
                    guard.commit().await?;
                    return Ok(());
                }
                // Authorization loss may prevent renewal, but cannot undo an already committed receipt.
                tx.query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT 1"))
                    .await?;
            }
            tx.commit().await?;
            return Ok(());
        }
        let op = self.lease_job(&tx, agent, &data.lease).await?;
        let guard = self.operation_authority(&tx, &op).await?;
        let hash = checkpoint_digest(&data)?;
        let generation = number(&op, "generation")?;
        let mut checkpoint = if op["checkpoint"].is_null() {
            json!({})
        } else {
            op["checkpoint"].clone()
        };
        let mut receipts = checkpoint["nativeReceipts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let b=one(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1 FOR UPDATE",vec![text(&op,"backend_id")?.into()]).await?;
        if b["cnpg_uid"]
            .as_str()
            .is_some_and(|uid| uid != data.cnpg_uid)
        {
            return Err(lease_conflict());
        }
        if let Some(ca) = &data.ca_certificate {
            super::binding::validate_certificate(ca, true)?;
            if b["ca_certificate"].as_str().is_some_and(|old| old != ca) {
                return Err(lease_conflict());
            }
            exec(
                &tx,
                "UPDATE webdock_auth.hosting_database_backend SET ca_certificate=$2 WHERE id=$1",
                vec![text(&b, "id")?.into(), ca.clone().into()],
            )
            .await?;
        }
        let previous = checkpoint["phase"].as_str();
        if matches!(data.phase, DatabaseCheckpointPhase::Applied) && previous.is_some()
            || matches!(data.phase, DatabaseCheckpointPhase::Configuring)
                && previous != Some("applied")
            || matches!(data.phase, DatabaseCheckpointPhase::Observing)
                && ![Some("configuring"), Some("observing")].contains(&previous)
        {
            return Err(lease_conflict());
        }
        for (key, value) in [
            ("previousPodUID", &data.previous_pod_uid),
            ("migrationJobUID", &data.migration_job_uid),
            ("copySHA256", &data.copy_sha256),
        ] {
            if let Some(value) = value {
                if checkpoint[key].as_str().is_some_and(|old| old != value) {
                    return Err(lease_conflict());
                }
                checkpoint[key] = json!(value);
            }
        }
        checkpoint["phase"] = serde_json::to_value(data.phase).map_err(|_| invalid())?;
        checkpoint["cnpgUID"] = json!(data.cnpg_uid);
        if receipts.len() >= 16 {
            return Err(lease_conflict());
        }
        let receipt = json!({"hash":hash,"generation":generation,"agentGeneration":agent.generation(),"tokenHash":digest(&data.lease.lease_token)});
        receipts.push(receipt.clone());
        checkpoint["nativeReceipts"] = json!(receipts);
        let mut desired = op["desired"].as_object().cloned().unwrap_or_default();
        let mut history = desired
            .get("nativeDatabaseCheckpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if history.len() >= 64 {
            history.remove(0);
        }
        history.push(receipt);
        desired.insert("nativeDatabaseCheckpoints".into(), json!(history));
        exec(
            &tx,
            "UPDATE webdock_auth.hosting_database_backend SET cnpg_uid=$2 WHERE id=$1",
            vec![text(&b, "id")?.into(), data.cnpg_uid.into()],
        )
        .await?;
        exec(
            &tx,
            "UPDATE webdock_auth.hosting_database_job SET checkpoint=$2 WHERE operation_id=$1",
            vec![text(&op, "id")?.into(), checkpoint.into()],
        )
        .await?;
        if data.migration_job_uid.is_some() || data.copy_sha256.is_some() {
            if op["database_action"] != "migrate" {
                return Err(lease_conflict());
            }
            let m=one(&tx,"SELECT to_jsonb(m) AS value FROM webdock_auth.hosting_database_migration m WHERE database_id=$1 AND target_backend_id=$2 FOR UPDATE",vec![text(&op,"database_id")?.into(),text(&b,"id")?.into()]).await?;
            if m["job_uid"]
                .as_str()
                .zip(data.migration_job_uid.as_deref())
                .is_some_and(|(old, new)| old != new)
            {
                return Err(lease_conflict());
            }
            exec(&tx,"UPDATE webdock_auth.hosting_database_migration SET job_uid=coalesce(job_uid,$3),copy_sha256=coalesce($4,copy_sha256) WHERE database_id=$1 AND target_backend_id=$2",vec![text(&op,"database_id")?.into(),text(&b,"id")?.into(),data.migration_job_uid.into(),data.copy_sha256.into()]).await?;
        }
        exec(&tx,"UPDATE webdock_auth.hosting_operation SET lease_until=now()+interval '300 seconds',desired=$2 WHERE id=$1",vec![text(&op,"id")?.into(),Value::Object(desired).into()]).await?;
        tx.commit().await?;
        guard.commit().await
    }
}

fn checkpoint_phase_allowed(spec: DatabaseExecutionPhase, phase: DatabaseCheckpointPhase) -> bool {
    match spec {
        DatabaseExecutionPhase::Apply => phase == DatabaseCheckpointPhase::Applied,
        DatabaseExecutionPhase::Configure => [
            DatabaseCheckpointPhase::Configuring,
            DatabaseCheckpointPhase::Observing,
        ]
        .contains(&phase),
        DatabaseExecutionPhase::Observe => phase == DatabaseCheckpointPhase::Observing,
        DatabaseExecutionPhase::Uncertain => false,
    }
}
fn checkpoint_recorded(
    op: &Value,
    agent: &AgentIdentity,
    data: &DatabaseCheckpoint,
) -> Result<bool> {
    if data.lease.generation != agent.generation() {
        return Ok(false);
    }
    let hash = checkpoint_digest(data)?;
    let token_hash = digest(&data.lease.lease_token);
    Ok([
        op["desired"]["nativeDatabaseCheckpoints"].as_array(),
        op["checkpoint"]["nativeReceipts"].as_array(),
    ]
    .into_iter()
    .flatten()
    .flatten()
    .any(|receipt| {
        receipt["hash"] == hash
            && receipt["agentGeneration"] == agent.generation()
            && (receipt["tokenHash"] == token_hash
                || (receipt["tokenHash"].is_null()
                    && receipt["generation"] == op["generation"]
                    && op["lease_token_hash"] == token_hash))
    }))
}

fn policy_authority_only(op: &Value) -> bool {
    // This selects a lane only; backup_policy_authority retains all UID/revision/resource fences.
    op["action"] == "databases.configureBackup"
        && op["database_action"] == "configureBackup"
        && op["backup_policy_id"]
            .as_str()
            .is_some_and(|id| id.parse::<i64>().is_ok_and(|id| id > 0))
        && op["backup_policy_revision"]
            .as_i64()
            .is_some_and(|revision| revision > 0)
}

#[cfg(test)]
mod original_authority_tests {
    use super::*;
    #[test]
    fn manual_backup_and_restore_never_bypass_original_authority_for_a_policy_id() {
        for action in ["backup", "restore"] {
            let op = json!({"action":format!("databases.{action}"),"database_action":action,"backup_policy_id":"101","backup_policy_revision":1});
            assert!(
                !policy_authority_only(&op),
                "non-grant job requires original authority"
            );
        }
    }
    #[test]
    fn only_bound_configuration_jobs_can_use_committed_durable_policy_lane() {
        let op = json!({"action":"databases.configureBackup","database_action":"configureBackup","backup_policy_id":"101","backup_policy_revision":1});
        assert!(policy_authority_only(&op));
        for (key, value) in [
            ("action", json!("databases.backup")),
            ("database_action", json!("backup")),
            ("backup_policy_id", Value::Null),
            ("backup_policy_revision", json!(0)),
        ] {
            let mut invalid = op.clone();
            invalid[key] = value;
            assert!(!policy_authority_only(&invalid));
        }
    }
}
