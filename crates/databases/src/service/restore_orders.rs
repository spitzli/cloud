//! Staged admission only, compiled under service's cfg(test); never a public route.
use super::control::ReceiptScope;
use super::*;
#[path = "../restore_intent.rs"]
mod intent;
mod review_sql;
use intent::{RestoreAcceptance, RestoreIntent, RestoreSource, RestoreTarget};
const SOURCE_BACKEND_READ: &str = "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1 FOR SHARE NOWAIT";
/// Proposed Browser caller ownership: early worker epoch comes first; the caller
/// then holds session-context fence, these real auth/business transactions and
/// current authorization through worker commit. No transaction is committed here.
/// ANY failed method retires this caller. The transaction-owning entrypoint MUST
/// drop borrows and roll back worker, business and authentication in full, then
/// restart at the early epoch and fresh session/auth. Never retry an inner
/// savepoint while retaining policy/cluster/auth locks. Borrowed methods cannot
/// consume or roll back those transactions; this latch only forbids service reuse.
struct RestoreCaller<'a> {
    authentication: &'a DatabaseTransaction,
    business: &'a DatabaseTransaction,
    principal: &'a Principal,
    authority: &'a StoredAuthority,
    attempt: RestoreAttemptState,
}
type AttemptResult<T> = std::result::Result<T, RestoreAttemptFailure>;
#[derive(Debug)]
enum RestoreAttemptFailure {
    Contention { sqlstate: &'static str },
    Rejected(Error),
    Retired,
}
impl From<Error> for RestoreAttemptFailure {
    fn from(error: Error) -> Self {
        Self::Rejected(error)
    }
}
#[derive(Default)]
struct RestoreAttemptState {
    failed: std::cell::Cell<bool>,
}
impl RestoreAttemptState {
    fn check(&self) -> AttemptResult<()> {
        if self.failed.get() {
            Err(RestoreAttemptFailure::Retired)
        } else {
            Ok(())
        }
    }
    fn finish<T>(&self, result: AttemptResult<T>) -> AttemptResult<T> {
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
}
fn restore_database_failure(error: sea_orm::DbErr) -> RestoreAttemptFailure {
    let code = match &error {
        sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(e))
        | sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) => e
            .as_database_error()
            .and_then(|e| e.code())
            .map(|code| code.into_owned()),
        _ => None,
    };
    if code.as_deref() == Some("55P03") {
        RestoreAttemptFailure::Contention { sqlstate: "55P03" }
    } else {
        RestoreAttemptFailure::Rejected(error.into())
    }
}
async fn restore_one(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> AttemptResult<Value> {
    let row = tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(restore_database_failure)?
        .ok_or_else(denied)?;
    row.try_get("", "value").map_err(restore_database_failure)
}
impl RestoreCaller<'_> {
    async fn current(&self, service: &DatabaseService) -> Result<Principal> {
        if self.attempt.failed.get() {
            return Err(conflict(
                "Discard the whole failed restore attempt and reauthorize.",
            ));
        }
        let current = webdock_browser_session::revalidate_stored_authority(
            &service.identity,
            self.authentication,
            service.browser.as_deref(),
            &self.principal.user_id,
            &self.principal.session_id,
            self.authority,
        )
        .await
        .map_err(|_| denied())?;
        if !current.can_access() || current.in_read_only_preview() || self.authority.read_only() {
            return Err(denied());
        }
        Ok(current)
    }
}
impl DatabaseService {
    async fn prepare_restore_order(
        &self,
        caller: &RestoreCaller<'_>,
        input: Value,
    ) -> AttemptResult<Value> {
        caller.attempt.check()?;
        let result = self.prepare_restore_order_in(caller, input).await;
        caller.attempt.finish(result)
    }
    async fn prepare_restore_order_in(
        &self,
        caller: &RestoreCaller<'_>,
        input: Value,
    ) -> AttemptResult<Value> {
        let cmd = staged_command(input, "databases.restoreOrder.prepare")?;
        let actor = caller.current(self).await?;
        let tx = caller.business;
        self.installed(tx, true).await?;
        let source = self
            .control_access(
                tx,
                &actor,
                caller.authority,
                text(&cmd, "sourceDatabaseID")?,
                true,
                true,
                true,
            )
            .await?;
        let access = self
            .authorize_mode(
                tx,
                &actor,
                Some(text(&cmd, "targetProjectID")?),
                None,
                true,
                true,
                caller.authority,
                true,
            )
            .await?;
        if source["kind"] != "dedicated"
            || source["customer_id"] != access["customerID"]
            || source["project_id"] != cmd["targetProjectID"]
        {
            return Err((conflict(
                "Recovery requires the same customer and project on Dedicated PostgreSQL.",
            ))
            .into());
        }
        let (request_hash, prior) = self.replay_control(tx, &actor, &cmd).await?;
        if let Some(prior) = prior {
            return Ok(prior);
        }
        let source=one(tx,"SELECT jsonb_build_object('id',d.id,'customer_id',d.customer_id,'project_id',d.project_id,'backend_id',d.backend_id,'kind',d.kind,'state',d.state,'revision',d.revision,'pending_backend_id',d.pending_backend_id,'pending_order_id',d.pending_order_id) AS value FROM webdock_auth.hosting_database d WHERE d.id=$1 FOR SHARE OF d",vec![text(&source,"id")?.into()]).await?;
        if source["customer_id"] != access["customerID"]
            || source["project_id"] != cmd["targetProjectID"]
        {
            return Err((denied()).into());
        }
        let (p, target_time) = self.restore_source(tx, &source, &cmd).await?;
        let backend = restore_one(
            tx,
            SOURCE_BACKEND_READ,
            vec![text(&source, "backend_id")?.into()],
        )
        .await?;
        let policy: DatabaseBackupPolicySpec =
            serde_json::from_value(p["spec"].clone()).map_err(|_| invalid())?;
        policy.validate().map_err(|_| invalid())?;
        if !p["revoked_at"].is_null()
            || source["state"] == "purged"
            || !source["pending_backend_id"].is_null()
            || !source["pending_order_id"].is_null()
            || backend["cnpg_uid"] != p["source_uid"]
            || backend["id"] != p["backend_id"]
            || backend["revision"] != policy.resource_revision
            || backend["cluster_id"] != p["cluster_id"]
        {
            return Err((conflict("Refresh the current source and archive identity.")).into());
        }
        one(tx,"SELECT jsonb_build_object('revision',revision) AS value FROM webdock_auth.hosting_database_backup_credentials WHERE policy_id=$1 AND revision=$2",vec![text(&p,"id")?.into(),number(&cmd,"credentialRevision")?.into()]).await?;
        let plan=one(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_plan p WHERE id=$1 FOR SHARE",vec![text(&cmd,"targetPlanRevisionID")?.into()]).await?;
        self.plan_available(&plan, text(&source, "customer_id")?, true)?;
        let now = chrono::Utc::now();
        let review = RestoreIntent {
            version: 1,
            purpose: "restore".into(),
            offer_id: self.id().await?,
            expires_at: review_expiry(now),
            source: RestoreSource {
                database_id: text(&source, "id")?.into(),
                revision: number(&source, "revision")?,
                cluster_id: text(&backend, "cluster_id")?.into(),
                policy,
                restore_point_id: text(&cmd, "restorePointID")?.into(),
                base_backup_id: text(&p, "base_backup_id")?.into(),
                completed_at: chrono::DateTime::parse_from_rfc3339(text(&p, "completed_at")?)
                    .map_err(|_| invalid())?
                    .with_timezone(&chrono::Utc),
                credential_revision: number(&cmd, "credentialRevision")?,
            },
            target: RestoreTarget {
                customer_id: text(&source, "customer_id")?.into(),
                project_id: text(&source, "project_id")?.into(),
                cluster_id: text(&backend, "cluster_id")?.into(),
                plan_revision_id: text(&plan, "id")?.into(),
                name: text(&cmd, "name")?.into(),
                plan: plan["plan"].clone(),
            },
            target_time,
            timeline: u32::try_from(cmd["timeline"].as_u64().ok_or_else(invalid)?)
                .map_err(|_| invalid())?,
            verification_sha256: text(&cmd, "verificationSHA256")?.into(),
        };
        let review =
            RestoreIntent::parse(serde_json::to_value(review).map_err(|_| invalid())?, now)
                .map_err(|_| invalid())?;
        let snapshot = restore_snapshot(&review)?;
        exec(tx,"INSERT INTO webdock_auth.hosting_database_offer(id,customer_id,project_id,plan_revision_id,name,snapshot,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)",vec![review.offer_id.clone().into(),review.target.customer_id.clone().into(),review.target.project_id.clone().into(),review.target.plan_revision_id.clone().into(),review.target.name.clone().into(),snapshot.clone().into(),review.expires_at.into()]).await?;
        let result = json!({"offerID":review.offer_id,"snapshot":snapshot,"intentHash":review.hash().map_err(|_|invalid())?,"expiresAt":review.expires_at});
        self.record_control(
            tx,
            &actor,
            &cmd,
            &request_hash,
            &result,
            ReceiptScope {
                customer: Some(&review.target.customer_id),
                project: Some(&review.target.project_id),
                cluster: Some(&review.target.cluster_id),
            },
        )
        .await?;
        caller.current(self).await?;
        self.authorize_mode(
            tx,
            &actor,
            Some(&review.target.project_id),
            None,
            true,
            true,
            caller.authority,
            true,
        )
        .await?;
        Ok(result)
    }
    /// Worker already owns the single early epoch SHARE. Undefined 0056 calls are
    /// confined to this cfg(test) module until coordinated materialization.
    async fn accept_restore_order(
        &self,
        worker: &DatabaseTransaction,
        caller: &RestoreCaller<'_>,
        input: Value,
    ) -> AttemptResult<Value> {
        caller.attempt.check()?;
        let result = self.accept_restore_order_in(worker, caller, input).await;
        caller.attempt.finish(result)
    }
    async fn accept_restore_order_in(
        &self,
        worker: &DatabaseTransaction,
        caller: &RestoreCaller<'_>,
        input: Value,
    ) -> AttemptResult<Value> {
        let cmd = staged_command(input, "databases.restoreOrder.accept")?;
        let accept = RestoreAcceptance::parse(cmd.clone()).map_err(|_| invalid())?;
        let actor = caller.current(self).await?;
        self.installed(caller.business, true).await?;
        let access = self
            .authorize_mode(
                caller.business,
                &actor,
                Some(&accept.project_id),
                None,
                true,
                true,
                caller.authority,
                true,
            )
            .await?;
        let discovered=one(caller.business,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_database_offer o WHERE id=$1 AND project_id=$2 AND customer_id=$3",vec![accept.offer_id.clone().into(),accept.project_id.clone().into(),text(&access,"customerID")?.into()]).await?;
        let discovered_intent = decode_offer(&discovered, &accept)?;
        let source = self
            .control_access(
                caller.business,
                &actor,
                caller.authority,
                &discovered_intent.source.database_id,
                true,
                true,
                true,
            )
            .await?;
        if source["customer_id"] != access["customerID"]
            || source["project_id"] != accept.project_id
        {
            return Err((denied()).into());
        }
        self.cluster_lock(worker, &discovered_intent.target.cluster_id)
            .await?;
        let (request_hash, prior) = self.replay_control(worker, &actor, &cmd).await?;
        let offer=one(worker,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_database_offer o WHERE id=$1 AND project_id=$2 AND customer_id=$3 FOR UPDATE",vec![accept.offer_id.clone().into(),accept.project_id.clone().into(),text(&access,"customerID")?.into()]).await?;
        let review = decode_offer(&offer, &accept)?;
        if review.hash().map_err(|_| invalid())?
            != discovered_intent.hash().map_err(|_| invalid())?
        {
            return Err((conflict("Refresh the immutable restore review.")).into());
        }
        let scope = one(
            worker,
            "SELECT webdock_rls.request_scope() AS value",
            vec![],
        )
        .await?;
        check_worker_scope(&scope, &actor, caller.authority, &review)?;
        // Existing order/job identity is authoritative even after expiry or a later
        // operator's retry. This branch never rewrites desired/authority/generation.
        let rows = all(
            worker,
            RESTORE_RECEIPT_QUERY,
            vec![
                review.offer_id.clone().into(),
                review.hash().map_err(|_| invalid())?.into(),
            ],
        )
        .await?;
        if let Some(result) = existing_restore_receipt(&rows)? {
            if prior.as_ref().is_some_and(|p| p != &result) {
                return Err((conflict("Restore receipt lineage changed.")).into());
            }
            if prior.is_none() {
                self.record_control(
                    worker,
                    &actor,
                    &cmd,
                    &request_hash,
                    &result,
                    ReceiptScope {
                        customer: Some(&review.target.customer_id),
                        project: Some(&review.target.project_id),
                        cluster: Some(&review.target.cluster_id),
                    },
                )
                .await?;
            }
            self.finish_restore_authorization(caller, &actor, &review)
                .await?;
            return Ok(result);
        }
        if prior.is_some() {
            return Err((conflict("Restore receipt requires its retained order and job.")).into());
        }
        review
            .validate_acceptance(&accept, chrono::Utc::now())
            .map_err(|_| conflict("Restore review expired or changed. Request a new review."))?;
        let plan=one(worker,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_plan p WHERE id=$1 FOR SHARE",vec![review.target.plan_revision_id.clone().into()]).await?;
        self.plan_available(&plan, &review.target.customer_id, true)?;
        if plan["plan"] != review.target.plan {
            return Err((conflict("Restore target plan changed.")).into());
        }
        let material = restore_one(
            worker,
            "SELECT webdock_rls.database_restore_source($1,$2) AS value",
            vec![
                review.offer_id.clone().into(),
                review.hash().map_err(|_| invalid())?.into(),
            ],
        )
        .await?;
        let material: SourceMaterial = serde_json::from_value(material).map_err(|_| denied())?;
        material.matches(&review)?;
        let mut allocated = self
            .allocate_order_target(
                worker,
                &actor,
                &offer,
                &review.target.plan,
                &review.target.cluster_id,
            )
            .await?;
        super::backups::admit_restore_backend(
            &allocated.backend,
            &json!({"backend_id":review.source.policy.installation_id,"cluster_id":review.source.cluster_id}),
        )?;
        self.reserve_backup_resources(worker, &allocated.backend, &review.source.policy)
            .await?;
        let sealed = super::backups::reseal_restore_credentials(
            &self.cipher,
            &json!({"id":review.source.policy.id}),
            review.source.credential_revision,
            &material.credentials_encrypted,
            &allocated.database,
        )?;
        exec(worker,"UPDATE webdock_auth.hosting_database SET state='restoring',credential_revision=$2,credentials_encrypted=$3 WHERE id=$1",vec![allocated.database.clone().into(),review.source.credential_revision.into(),sealed.into()]).await?;
        let target = json!({"id":allocated.database,"revision":1});
        let recovery = DatabaseRecoverySpec {
            source: review.source.policy.clone(),
            source_database_id: review.source.database_id.clone(),
            base_backup_id: review.source.base_backup_id.clone(),
            target_time: review.target_time,
            timeline: review.timeline,
            credential_revision: review.source.credential_revision,
            verification_sha256: review.verification_sha256.clone(),
        };
        let queued = self
            .enqueue_restore(
                worker,
                &actor,
                caller.authority,
                &allocated.backend,
                &target,
                &recovery,
                &review.source.policy.installation_id,
            )
            .await?;
        let operation = allocated
            .fresh
            .as_mut()
            .ok_or_else(unavailable)?
            .append_job(queued);
        let binding = json!({"restoreOfferID":review.offer_id,"restoreIntentHash":review.hash().map_err(|_|invalid())?,"restoreOrderID":allocated.order,"nativeEpoch":scope["nativeEpoch"],"restorePriceAccepted":true,"restorePrice":cmd["reviewedPrice"]});
        exec(worker,"UPDATE webdock_auth.hosting_operation SET desired=desired||$2 WHERE id=$1 AND status='queued' AND generation=0 AND lease_until IS NULL",vec![operation.clone().into(),binding.into()]).await?;
        let facts = allocated
            .fresh
            .as_ref()
            .ok_or_else(unavailable)?
            .final_facts(worker, &allocated.backend)
            .await?;
        let mut attested_scope = scope.clone();
        attested_scope["restoreAllocation"] = json!({"version":1,"topXid8":facts["topXid8"],"epoch":scope["nativeEpoch"],"offerID":review.offer_id,
            "intentHash":review.hash().map_err(|_|invalid())?,"subject":actor.user_id,"sessionID":actor.session_id,"backendID":allocated.backend["id"],"orderID":allocated.order,"databaseID":allocated.database,"rows":facts["rows"]});
        // Only server-owned INSERT facts enter the existing private RLS MAC.
        self.apply_worker_scope(worker, attested_scope).await?;
        exec(
            worker,
            "SELECT webdock_rls.record_database_restore_admission($1)",
            vec![operation.clone().into()],
        )
        .await?;
        let result = json!({"orderID":allocated.order,"databaseID":allocated.database,"operationID":operation});
        self.record_control(
            worker,
            &actor,
            &cmd,
            &request_hash,
            &result,
            ReceiptScope {
                customer: Some(&review.target.customer_id),
                project: Some(&review.target.project_id),
                cluster: Some(&review.target.cluster_id),
            },
        )
        .await?;
        self.finish_restore_authorization(caller, &actor, &review)
            .await?;
        Ok(result)
    }
    async fn finish_restore_authorization(
        &self,
        caller: &RestoreCaller<'_>,
        actor: &Principal,
        review: &RestoreIntent,
    ) -> Result<()> {
        caller.current(self).await?;
        self.control_access(
            caller.business,
            actor,
            caller.authority,
            &review.source.database_id,
            true,
            true,
            true,
        )
        .await?;
        self.authorize_mode(
            caller.business,
            actor,
            Some(&review.target.project_id),
            None,
            true,
            true,
            caller.authority,
            true,
        )
        .await?;
        Ok(())
    }
}
// Compose an INSERT's real xmin sub-XID with this transaction's full XID epoch.
// Status/tuple checks in SQL remain required; this arithmetic alone is not proof.
pub(super) fn expand_insert_xid(top: u64, xmin: u32) -> Result<u64> {
    if top < 3 || xmin < 3 {
        return Err(unavailable());
    }
    let base = top & !u64::from(u32::MAX);
    let candidate = base | u64::from(xmin);
    if candidate < top {
        candidate.checked_add(1_u64 << 32).ok_or_else(unavailable)
    } else {
        Ok(candidate)
    }
}
fn review_expiry(now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    chrono::SubsecRound::trunc_subsecs(now + chrono::Duration::minutes(30), 6)
}
fn staged_command(value: Value, action: &str) -> Result<Value> {
    static SCHEMA: std::sync::OnceLock<jsonschema::Validator> = std::sync::OnceLock::new();
    let validator = SCHEMA.get_or_init(|| {
        jsonschema::options()
            .should_validate_formats(true)
            .build(
                &serde_json::from_str(include_str!("../restore-order.schema.json"))
                    .expect("static restore schema"),
            )
            .expect("valid restore schema")
    });
    if value["action"] != action
        || serde_json::to_vec(&value).map_err(|_| invalid())?.len() > 8192
        || !validator.is_valid(&value)
    {
        return Err(invalid());
    }
    for key in [
        "sourceDatabaseID",
        "restorePointID",
        "targetProjectID",
        "targetPlanRevisionID",
        "offerID",
        "projectID",
    ] {
        if let Some(id) = value.get(key)
            && id
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .is_none_or(|n| n <= 0)
        {
            return Err(invalid());
        }
    }
    if let Some(target) = value.get("targetTime").and_then(Value::as_str) {
        let time = chrono::DateTime::parse_from_rfc3339(target).map_err(|_| invalid())?;
        if time.timestamp_subsec_nanos() % 1000 != 0 {
            return Err(invalid());
        }
    }
    if value
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| name.trim() != name)
    {
        return Err(invalid());
    }
    Ok(value)
}
fn restore_snapshot(review: &RestoreIntent) -> Result<Value> {
    let mut snapshot = review.target.plan.clone();
    snapshot["purpose"] = json!("restore");
    snapshot["restoreIntent"] = serde_json::to_value(review).map_err(|_| invalid())?;
    snapshot["intentHash"] = json!(review.hash().map_err(|_| invalid())?);
    snapshot["billing"] = json!(if review.target.plan.get("sponsoredCustomerID").is_some() {
        "sponsored"
    } else {
        "monthly-net-fixed"
    });
    snapshot["billingStarts"] = json!("observed-ready");
    snapshot["backupStatus"] = json!("not-configured");
    Ok(snapshot)
}
fn decode_offer(offer: &Value, accept: &RestoreAcceptance) -> Result<RestoreIntent> {
    let review =
        RestoreIntent::decode(offer["snapshot"]["restoreIntent"].clone()).map_err(|_| invalid())?;
    review
        .validate_reviewed_acceptance(accept)
        .map_err(|_| conflict("Exact immutable restore review and price required."))?;
    let expires =
        chrono::DateTime::parse_from_rfc3339(text(offer, "expires_at")?).map_err(|_| invalid())?;
    if offer["id"] != review.offer_id
        || offer["customer_id"] != review.target.customer_id
        || offer["project_id"] != review.target.project_id
        || offer["plan_revision_id"] != review.target.plan_revision_id
        || offer["name"] != review.target.name
        || expires != review.expires_at
        || offer["snapshot"] != restore_snapshot(&review)?
    {
        return Err(conflict("Exact immutable restore-purpose offer required."));
    }
    Ok(review)
}
const RESTORE_RECEIPT_QUERY: &str = "SELECT jsonb_build_object('orderID',o.id,'databaseID',d.id,'operationID',op.id) AS value FROM webdock_auth.hosting_database_order o LEFT JOIN webdock_auth.hosting_database d ON d.id=o.database_id AND d.customer_id=o.customer_id AND d.project_id=o.project_id LEFT JOIN webdock_auth.hosting_database_job j ON j.database_id=d.id AND j.action='restore' LEFT JOIN webdock_auth.hosting_operation op ON op.id=j.operation_id AND op.desired->>'restoreOfferID'=o.offer_id AND op.desired->>'restoreOrderID'=o.id AND op.desired->>'restoreIntentHash'=$2 WHERE o.offer_id=$1 ORDER BY op.created_at,op.id LIMIT 2";
fn existing_restore_receipt(rows: &[Value]) -> Result<Option<Value>> {
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() != 1
        || ["orderID", "databaseID", "operationID"]
            .iter()
            .any(|k| rows[0][k].as_str().is_none_or(str::is_empty))
    {
        return Err(conflict(
            "Retained restore order requires its unique original job.",
        ));
    }
    Ok(Some(rows[0].clone()))
}
fn check_worker_scope(
    scope: &Value,
    actor: &Principal,
    authority: &StoredAuthority,
    review: &RestoreIntent,
) -> Result<()> {
    if scope["kind"] != "system"
        || scope["purpose"] != "database-order"
        || scope["customerID"] != review.target.customer_id
        || scope["projectID"] != review.target.project_id
        || scope["clusterID"] != review.target.cluster_id
        || scope["subject"] != actor.user_id
        || scope["sessionID"] != actor.session_id
        || scope["restoreOfferID"] != review.offer_id
        || scope["restoreIntentHash"] != review.hash().map_err(|_| invalid())?
        || scope["nativeEpoch"].as_str().is_none_or(str::is_empty)
        || scope["operator"] != true
        || scope["canWrite"] != true
        || scope["permission"] != "org:databases:manage"
        || scope["nativeAuthority"] != serde_json::to_value(authority).map_err(|_| invalid())?
    {
        return Err(denied());
    }
    Ok(())
}
// Only the whitelisted source binding and exact ROLE ciphertext cross worker RLS.
// No Debug and no provider material; provider recovery reads remain in their own lane.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceMaterial {
    source: RestoreSource,
    credentials_encrypted: String,
}
impl SourceMaterial {
    fn matches(&self, review: &RestoreIntent) -> Result<()> {
        if serde_json::to_value(&self.source).map_err(|_| invalid())?
            != serde_json::to_value(&review.source).map_err(|_| invalid())?
            || self.credentials_encrypted.len() > 32768
            || !self.credentials_encrypted.starts_with("v1.")
        {
            return Err(denied());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
    fn review() -> Value {
        let recorded: Value = serde_json::from_str(include_str!(
            "../../../../docs/verification/2026-10-25-native-postgresql-backup-transitions.json"
        ))
        .unwrap();
        let policy = recorded["connectedEvidence"]["resize"]["policy"]["spec"].clone();
        json!({
            "version":1,"purpose":"restore","offerID":"2001","expiresAt":"2026-10-25T12:30:00Z",
            "source":{"databaseID":"2002","revision":2,"clusterID":"2003","policy":policy,
                "restorePointID":"2004","baseBackupID":"20261025T100000","completedAt":"2026-10-25T10:00:00Z","credentialRevision":1},
            "target":{"customerID":policy["customerID"],"projectID":policy["projectID"],"clusterID":"2003","planRevisionID":"2005","name":"Recovered",
                "plan":{"kind":"dedicated","postgresMajor":18,"name":"Dedicated restore","monthlyPriceCents":1200,"currency":"EUR",
                    "cpuMillicores":1000,"memoryBytes":2147483648_u64,"volumeBytes":10737418240_u64,"logicalStorageAllowanceBytes":null,"connectionLimit":10,
                    "terms":"Reviewed immutable fixed monthly net database price. Billing starts only after observed readiness."}},
            "targetTime":"2026-10-25T10:00:01Z","timeline":1,"verificationSHA256":"a".repeat(64)
        })
    }
    fn acceptance(intent: &RestoreIntent) -> Value {
        json!({"action":"databases.restoreOrder.accept","offerID":"2001","projectID":review()["target"]["projectID"],
        "intentHash":intent.hash().unwrap(),"confirmPrice":true,
        "reviewedPrice":{"monthlyPriceCents":1200,"currency":"EUR","billing":"monthly-net-fixed","billingStarts":"observed-ready"},
        "idempotencyKey":"restore-intent-accept-001"})
    }

    fn offer() -> Value {
        let value = review();
        let intent = RestoreIntent::parse(value.clone(), now()).unwrap();
        let mut snapshot = intent.target.plan.clone();
        snapshot["purpose"] = json!("restore");
        snapshot["restoreIntent"] = value;
        snapshot["intentHash"] = json!(intent.hash().unwrap());
        snapshot["billing"] = json!("monthly-net-fixed");
        snapshot["billingStarts"] = json!("observed-ready");
        snapshot["backupStatus"] = json!("not-configured");
        json!({"id":intent.offer_id,"customer_id":intent.target.customer_id,"project_id":intent.target.project_id,
            "plan_revision_id":intent.target.plan_revision_id,"name":intent.target.name,"expires_at":intent.expires_at,"snapshot":snapshot})
    }
    #[test]
    fn failed_restore_attempt_cannot_reenter_with_retained_transactions() {
        let attempt = RestoreAttemptState::default();
        attempt.check().unwrap();
        let error = attempt
            .finish::<()>(Err(RestoreAttemptFailure::Contention { sqlstate: "55P03" }))
            .unwrap_err();
        assert!(matches!(
            error,
            RestoreAttemptFailure::Contention { sqlstate: "55P03" }
        ));
        assert!(matches!(
            attempt.check(),
            Err(RestoreAttemptFailure::Retired)
        ));
        let fresh = RestoreAttemptState::default();
        fresh.finish(Ok(())).unwrap();
        fresh.check().unwrap();
        let rejected = fresh.finish::<()>(Err(denied().into())).unwrap_err();
        assert!(matches!(
            rejected,
            RestoreAttemptFailure::Rejected(Error { status: 404, .. })
        ));
        assert!(fresh.check().is_err());
    }
    #[test]
    fn new_source_backend_reads_must_fail_fast_without_waiting_after_other_locks() {
        assert!(SOURCE_BACKEND_READ.ends_with("FOR SHARE NOWAIT"));
        let sql = include_str!(
            "../../../../docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql"
        );
        let backend = sql
            .lines()
            .find(|line| {
                line.contains(
                    "INTO b FROM webdock_auth.hosting_database_backend WHERE id=d.backend_id",
                )
            })
            .unwrap();
        assert!(backend.ends_with("FOR SHARE NOWAIT;"));
    }
    #[test]
    fn only_retained_proof_time_is_truncated_before_the_exact_pg_comparison() {
        let expression = review_sql::retained_proof_time_expression();
        assert!(expression.starts_with("regexp_replace("));
        assert!(expression.contains("backup.proof->>'completedAt'"));
        assert!(expression.ends_with("::timestamptz"));
        assert!(!expression.contains("targetTime"));
    }
    #[test]
    fn inserting_subxid_expands_across_wrap_without_trusting_current_xmin() {
        assert_eq!(expand_insert_xid(100, 101).unwrap(), 101);
        assert_eq!(
            expand_insert_xid((7_u64 << 32) + u64::from(u32::MAX) - 1, 3).unwrap(),
            (8_u64 << 32) + 3
        );
        assert_eq!(
            expand_insert_xid((8_u64 << 32) + 10, 20).unwrap(),
            (8_u64 << 32) + 20
        );
        assert!(expand_insert_xid(100, 2).is_err());
        assert!(expand_insert_xid(u64::MAX - 1, 3).is_err());
    }
    #[test]
    fn source_target_time_is_never_silently_rounded_in_a_review() {
        let command = json!({"action":"databases.restoreOrder.prepare","sourceDatabaseID":"101","revision":2,"restorePointID":"102","credentialRevision":1,
            "targetTime":"2026-10-25T10:00:01.123456789Z","timeline":1,"verificationSHA256":"a".repeat(64),"targetProjectID":"103",
            "targetPlanRevisionID":"104","name":"Recovered","idempotencyKey":"restore-order-prepare-001"});
        assert!(staged_command(command.clone(), "databases.restoreOrder.prepare").is_err());
        let mut exact = command;
        exact["targetTime"] = json!("2026-10-25T10:00:01.123456Z");
        assert_eq!(
            staged_command(exact.clone(), "databases.restoreOrder.prepare").unwrap()["targetTime"],
            exact["targetTime"]
        );
    }
    #[test]
    fn expiry_is_postgres_precise_before_hash_and_storage_roundtrip() {
        let nano = DateTime::parse_from_rfc3339("2026-10-25T12:00:00.123456789Z")
            .unwrap()
            .with_timezone(&Utc);
        let expiry = review_expiry(nano);
        assert_eq!(expiry.timestamp_subsec_nanos(), 123456000);
        let mut value = review();
        value["expiresAt"] = json!(expiry);
        let intent = RestoreIntent::parse(value, nano).unwrap();
        let hash = intent.hash().unwrap();
        let mut stored = offer();
        stored["snapshot"] = restore_snapshot(&intent).unwrap();
        // Emulates PostgreSQL's timestamp precision; actual engine roundtrip is
        // reserved for the separately coordinated SQL primitive component.
        stored["expires_at"] =
            json!(DateTime::<Utc>::from_timestamp_micros(expiry.timestamp_micros()).unwrap());
        let accept = RestoreAcceptance::parse(acceptance(&intent)).unwrap();
        assert_eq!(
            decode_offer(&stored, &accept).unwrap().hash().unwrap(),
            hash
        );
    }
    #[test]
    fn immutable_offer_decode_allows_expired_review_only_for_existing_order_replay() {
        let intent = RestoreIntent::parse(review(), now()).unwrap();
        let accept = RestoreAcceptance::parse(acceptance(&intent)).unwrap();
        let decoded = decode_offer(&offer(), &accept).unwrap();
        assert_eq!(decoded.hash().unwrap(), intent.hash().unwrap());
        assert!(
            decoded
                .validate_acceptance(&accept, now() + chrono::Duration::hours(1))
                .is_err()
        );
    }
    #[test]
    fn immutable_offer_decode_binds_purpose_plan_price_name_expiry_and_hash() {
        let intent = RestoreIntent::parse(review(), now()).unwrap();
        let accept = RestoreAcceptance::parse(acceptance(&intent)).unwrap();
        for (pointer, value) in [
            ("/snapshot/purpose", json!("create")),
            ("/snapshot/intentHash", json!("b".repeat(64))),
            ("/snapshot/monthlyPriceCents", json!(1201)),
            ("/snapshot/billingStarts", json!("accepted")),
            ("/name", json!("Another")),
            ("/expires_at", json!("2026-10-25T12:31:00Z")),
            ("/customer_id", json!("999")),
            ("/project_id", json!("999")),
            ("/plan_revision_id", json!("999")),
            (
                "/snapshot/restoreIntent/source/policy/sourceCnpgUID",
                json!("foreign"),
            ),
            (
                "/snapshot/restoreIntent/source/credentialRevision",
                json!(2),
            ),
        ] {
            let mut bad = offer();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(decode_offer(&bad, &accept).is_err(), "{pointer}");
        }
        let mut changed = acceptance(&intent);
        changed["reviewedPrice"]["monthlyPriceCents"] = json!(1201);
        assert!(decode_offer(&offer(), &RestoreAcceptance::parse(changed).unwrap()).is_err());
        intent::require_ordinary_offer(&offer()["snapshot"]).unwrap_err();
    }
    #[test]
    fn existing_order_receipt_reuses_original_job_without_rebinding_authority_or_allocation() {
        let original = json!({"orderID":"301","databaseID":"302","operationID":"303"});
        assert_eq!(
            existing_restore_receipt(std::slice::from_ref(&original)).unwrap(),
            Some(original.clone())
        );
        assert_eq!(existing_restore_receipt(&[]).unwrap(), None);
        assert!(existing_restore_receipt(&[original.clone(), original]).is_err());
        assert!(
            existing_restore_receipt(&[
                json!({"orderID":"301","databaseID":"302","operationID":null})
            ])
            .is_err()
        );
    }
    #[test]
    fn source_material_rejects_foreign_archive_key_revision_and_secret_fields() {
        let intent = RestoreIntent::parse(review(), now()).unwrap();
        let value = json!({"source":intent.source,"credentialsEncrypted":"v1.fixture-ciphertext"});
        serde_json::from_value::<SourceMaterial>(value.clone())
            .unwrap()
            .matches(&intent)
            .unwrap();
        for pointer in [
            "/source/databaseID",
            "/source/policy/sourceCnpgUID",
            "/source/policy/destination/keyRecoveryID",
            "/source/policy/destination/credentialRevision",
            "/source/credentialRevision",
        ] {
            let mut changed = value.clone();
            let slot = changed.pointer_mut(pointer).unwrap();
            *slot = if slot.is_string() {
                json!("999")
            } else {
                json!(999)
            };
            assert!(
                serde_json::from_value::<SourceMaterial>(changed)
                    .unwrap()
                    .matches(&intent)
                    .is_err(),
                "{pointer}"
            );
        }
        for field in [
            "providerWriterKeys",
            "liveSourceCredentials",
            "writeEncrypted",
        ] {
            let mut injected = value.clone();
            injected[field] = json!("fixture-only");
            assert!(serde_json::from_value::<SourceMaterial>(injected).is_err());
        }
    }
    #[test]
    fn staged_dtos_reject_authority_injection_missing_review_and_unbounded_ids() {
        let intent = RestoreIntent::parse(review(), now()).unwrap();
        let valid = acceptance(&intent);
        staged_command(valid.clone(), "databases.restoreOrder.accept").unwrap();
        for field in ["nativeAuthority", "mcpAuthority", "writeCredentials"] {
            let mut bad = valid.clone();
            bad[field] = json!({});
            assert!(staged_command(bad, "databases.restoreOrder.accept").is_err());
        }
        let mut bad = valid.clone();
        bad.as_object_mut().unwrap().remove("reviewedPrice");
        assert!(staged_command(bad, "databases.restoreOrder.accept").is_err());
        let mut bad = valid;
        bad["offerID"] = json!("9999999999999999999");
        assert!(staged_command(bad, "databases.restoreOrder.accept").is_err());
    }
    #[test]
    fn draft_has_only_three_worker_interfaces_and_no_schema_or_provider_material_grants() {
        let sql = include_str!(
            "../../../../docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql"
        );
        assert_eq!(sql.matches("CREATE FUNCTION webdock_rls.").count(), 3);
        assert_eq!(sql.matches("GRANT EXECUTE ON FUNCTION").count(), 3);
        assert_eq!(sql.matches("TO webdock_worker_runtime;").count(), 3);
        for line in sql.lines().filter(|l| l.starts_with("GRANT")) {
            assert!(line.ends_with("TO webdock_worker_runtime;"));
        }
        for forbidden in [
            "CREATE TABLE",
            "CREATE POLICY",
            "ALTER TABLE",
            "GRANT SELECT",
            "GRANT UPDATE",
            "GRANT INSERT",
        ] {
            assert!(!sql.contains(forbidden));
        }
        let early = sql
            .split("CREATE FUNCTION webdock_rls.database_restore_epoch")
            .nth(1)
            .unwrap()
            .split("ALTER FUNCTION")
            .next()
            .unwrap();
        assert_eq!(early.matches("FOR SHARE NOWAIT").count(), 1);
        let source_backend = sql
            .lines()
            .find(|line| {
                line.contains(
                    "INTO b FROM webdock_auth.hosting_database_backend WHERE id=d.backend_id",
                )
            })
            .unwrap();
        assert!(source_backend.ends_with("FOR SHARE NOWAIT;"));
        let verify = sql
            .split("-- BEGIN owner-only witness verification")
            .nth(1)
            .unwrap()
            .split("-- END owner-only witness verification")
            .next()
            .unwrap();
        assert!(!verify.contains("FOR SHARE NOWAIT"));
        assert!(!sql.contains("sha256("));
        let source = sql
            .split("CREATE FUNCTION webdock_rls.database_restore_source")
            .nth(1)
            .unwrap()
            .split("ALTER FUNCTION")
            .next()
            .unwrap();
        assert!(!source.contains("destination.write_encrypted"));
        assert!(!source.contains("destination.recovery_encrypted"));
        assert!(!source.contains("d.credentials_encrypted"));
        assert!(!source.contains("to_jsonb("));
        assert!(source.contains("'credentialsEncrypted',v.credentials_encrypted"));
    }
    // Requires a fresh factory-owned actual Native source, enrolled operator's
    // existing PKCE/MFA Console cookie and current 0056 applied by its owner.
    // This stage only compiles the ignored caller; it reads no input by default.
    #[tokio::test]
    #[ignore = "fresh actual Native source factory plus coordinated 0056 release only"]
    async fn actual_native_source_prepares_direct_restore_retries_and_rolls_back() {
        use sea_orm::Database;
        use std::collections::BTreeMap;
        use webdock_identity::{CookiePolicy, SecretKeys};
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Input {
            auth_url: String,
            business_url: String,
            worker_url: String,
            identity_secret: String,
            encryption_secret: String,
            rls_secret: String,
            lease_secret: String,
            browser_cookie_secret: String,
            app_origin: String,
            issuer: String,
            client_id: String,
            client_secret: String,
            cookie_header: String,
            cluster_id: String,
            endpoint_domain: String,
            prepare: Value,
        }
        fn local(raw: &str) -> &str {
            let u = url::Url::parse(raw).unwrap_or_else(|_| panic!("local fixture URL"));
            assert!(["postgres", "postgresql"].contains(&u.scheme()));
            assert!([Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&u.host_str()));
            assert!(u.query().is_none() && u.fragment().is_none());
            raw
        }
        let path = std::env::var("WEBDOCK_PG_RESTORE_INTERNAL_FIXTURE")
            .expect("owned Native factory input path required");
        assert!(path.starts_with("/tmp/webdock-pg-restore-internal-"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        let private = std::fs::read(&path).expect("private owned fixture input");
        let input: Input = serde_json::from_slice(&private)
            .unwrap_or_else(|_| panic!("exact private fixture input shape"));
        let auth = Database::connect(local(&input.auth_url))
            .await
            .unwrap_or_else(|_| panic!("fixture auth connection"));
        let db = Database::connect(local(&input.business_url))
            .await
            .unwrap_or_else(|_| panic!("fixture business connection"));
        let worker = Database::connect(local(&input.worker_url))
            .await
            .unwrap_or_else(|_| panic!("fixture worker connection"));
        let ids = Arc::new(
            IdAllocator::connect(
                local(&input.auth_url),
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap_or_else(|_| panic!("fixture allocator")),
        );
        let keys = Arc::new(
            SecretKeys::new(
                BTreeMap::from([(0, input.identity_secret.clone())]),
                0,
                Some(input.identity_secret.clone()),
            )
            .unwrap(),
        );
        let identity = Arc::new(
            IdentityService::new(
                auth.clone(),
                CookiePolicy::new(false, input.identity_secret.clone()).unwrap(),
            )
            .with_secret_keys(
                SecretKeys::new(
                    BTreeMap::from([(0, input.identity_secret.clone())]),
                    0,
                    Some(input.identity_secret.clone()),
                )
                .unwrap(),
            )
            .with_allocator(ids.clone()),
        );
        let oauth = Arc::new(webdock_oauth::OAuthService::new(
            auth.clone(),
            identity.clone(),
            ids.clone(),
            keys.clone(),
            webdock_oauth::OAuthConfig::new(
                &input.issuer,
                &format!("{}/mcp", input.app_origin),
                Some(input.client_id.clone()),
            )
            .unwrap(),
        ));
        let browser = Arc::new(
            BrowserSessions::new(
                auth.clone(),
                identity.clone(),
                oauth,
                keys,
                &input.browser_cookie_secret,
                webdock_browser_session::BrowserConfig::new(
                    &input.app_origin,
                    &input.issuer,
                    input.client_id,
                    input.client_secret,
                )
                .unwrap(),
            )
            .unwrap(),
        );
        let actual = browser
            .resolve_authority(&input.app_origin, &input.cookie_header)
            .await
            .ok()
            .flatten()
            .expect("actual enrolled operator PKCE/MFA Browser session");
        let authority = StoredAuthority::FirstPartyBrowser {
            browser: actual.authority,
        };
        let service = DatabaseService::new(
            db.clone(),
            auth.clone(),
            worker.clone(),
            identity,
            Some(browser),
            ids,
            input.rls_secret,
            &input.encryption_secret,
            &input.app_origin,
        )
        .await
        .unwrap()
        .with_management(ManagementConfig {
            cluster_id: Some(input.cluster_id),
            endpoint_domain: Some(input.endpoint_domain),
            public_ordering: false,
            protected_database_id: None,
            lease_secret: Some(input.lease_secret),
        })
        .unwrap();
        // Prepare persists only a review in this fresh owned fixture; its factory
        // retains responsibility for deleting that exact offer/receipt at teardown.
        let business = service.db.begin().await.unwrap();
        session_fence(&business, &actual.principal).await;
        let authentication = service.auth.begin().await.unwrap();
        service
            .signer
            .apply(&business, &actual.principal)
            .await
            .unwrap();
        let caller = RestoreCaller {
            attempt: RestoreAttemptState::default(),
            authentication: &authentication,
            business: &business,
            principal: &actual.principal,
            authority: &authority,
        };
        let prepare_key = input.prepare["idempotencyKey"].clone();
        let prepared = service
            .prepare_restore_order(&caller, input.prepare)
            .await
            .unwrap();
        business.commit().await.unwrap();
        authentication.commit().await.unwrap();
        let review = RestoreIntent::decode(prepared["snapshot"]["restoreIntent"].clone()).unwrap();
        let key = service.id().await.unwrap();
        let command = json!({"action":"databases.restoreOrder.accept","offerID":review.offer_id,"projectID":review.target.project_id,
            "intentHash":review.hash().unwrap(),"confirmPrice":true,"reviewedPrice":{"monthlyPriceCents":review.target.plan["monthlyPriceCents"],"currency":review.target.plan["currency"],"billing":prepared["snapshot"]["billing"],"billingStarts":"observed-ready"},"idempotencyKey":format!("staged-restore-{key}")});
        let tx = service.worker.begin().await.unwrap();
        // No retained auth/business locks exist when acquiring the one epoch.
        let epoch = one(
            &tx,
            "SELECT to_jsonb(webdock_rls.database_restore_epoch(NULL)) AS value",
            vec![],
        )
        .await
        .unwrap();
        let business = service.db.begin().await.unwrap();
        session_fence(&business, &actual.principal).await;
        let authentication = service.auth.begin().await.unwrap();
        service
            .signer
            .apply(&business, &actual.principal)
            .await
            .unwrap();
        let caller = RestoreCaller {
            attempt: RestoreAttemptState::default(),
            authentication: &authentication,
            business: &business,
            principal: &actual.principal,
            authority: &authority,
        };
        let current = caller.current(&service).await.unwrap();
        let access = service
            .authorize_mode(
                &business,
                &current,
                Some(&review.target.project_id),
                None,
                true,
                true,
                &authority,
                true,
            )
            .await
            .unwrap();
        service.apply_worker_scope(&tx,json!({"kind":"system","purpose":"database-order","clusterID":review.target.cluster_id,"customerID":access["customerID"],"projectID":review.target.project_id,
            "subject":current.user_id,"sessionID":current.session_id,"operator":access["operator"],"canWrite":access["canWrite"],"permission":"org:databases:manage",
            "restoreOfferID":review.offer_id,"restoreIntentHash":review.hash().unwrap(),"nativeEpoch":epoch,"nativeAuthority":authority})).await.unwrap();
        let first = service
            .accept_restore_order(&tx, &caller, command.clone())
            .await
            .unwrap();
        let mut retry = command;
        retry["idempotencyKey"] = json!(format!("staged-retry-{key}"));
        let second = service
            .accept_restore_order(&tx, &caller, retry)
            .await
            .unwrap();
        assert_eq!(first, second);
        let inventory=one(&tx,"SELECT jsonb_build_object('restore',count(*) FILTER(WHERE action='restore'),'create',count(*) FILTER(WHERE action='create')) AS value FROM webdock_auth.hosting_database_job WHERE database_id=$1",vec![text(&first,"databaseID").unwrap().into()]).await.unwrap();
        assert_eq!(inventory, json!({"restore":1,"create":0}));
        let target = text(&first, "databaseID").unwrap().to_owned();
        tx.rollback().await.unwrap();
        business.rollback().await.unwrap();
        authentication.rollback().await.unwrap();
        let check = service.begin_actor(&actual.principal).await.unwrap();
        let absent=one(&check,"SELECT jsonb_build_object('count',count(*)) AS value FROM webdock_auth.hosting_database WHERE id=$1",vec![target.into()]).await.unwrap();
        assert_eq!(absent["count"], 0);
        check.rollback().await.unwrap();
        // Store only new review identifiers for the owning factory's exact cleanup.
        let receipt = json!({"offerID":prepared["offerID"],"prepareKey":prepare_key,"subjectID":actual.principal.user_id,"targetRolledBack":true});
        let receipt_path = format!("{path}.receipt.json");
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(receipt_path)
            .unwrap()
            .write_all(serde_json::to_string(&receipt).unwrap().as_bytes())
            .unwrap();
    }
    async fn session_fence(tx: &DatabaseTransaction, principal: &Principal) {
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            [format!("native-session-context:{}", principal.session_id).into()],
        ))
        .await
        .unwrap();
    }
}
