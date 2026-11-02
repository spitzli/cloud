use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use subtle::ConstantTimeEq;
impl HostingService {
    pub async fn authenticate_agent(
        &self,
        cluster: &str,
        credential: &str,
    ) -> Result<AgentIdentity> {
        contracts::record_id(cluster)?;
        if credential.len() != 43
            || !credential
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err(error(401, "Cluster authentication failed."));
        }
        let row=one(&self.auth,"SELECT generation,credential_hash,revoked FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![cluster.into()]).await?.ok_or_else(||error(401,"Cluster authentication failed."))?;
        let hash = hex::encode(Sha256::digest(credential.as_bytes()));
        let stored: String = row.try_get("", "credential_hash").map_err(db_error)?;
        if row.try_get::<bool>("", "revoked").map_err(db_error)?
            || !bool::from(stored.as_bytes().ct_eq(hash.as_bytes()))
        {
            return Err(error(401, "Cluster authentication failed."));
        }
        Ok(AgentIdentity {
            cluster: cluster.into(),
            generation: row.try_get("", "generation").map_err(db_error)?,
            credential_hash: hash,
        })
    }
    /// Backend-only: Core must first verify the transport peer against its persisted enrollment.
    /// `credential_hash` comes from that trusted enrollment record, never an HTTP header or Hello body.
    /// The live retained generation/hash check makes bearer rotation revoke transport enrollment too.
    pub async fn authenticate_enrolled_agent(
        &self,
        cluster: &str,
        generation: i32,
        credential_hash: &str,
    ) -> Result<AgentIdentity> {
        contracts::record_id(cluster)?;
        if generation <= 0
            || credential_hash.len() != 64
            || !credential_hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(error(401, "Cluster authentication failed."));
        }
        let row=one(&self.auth,"SELECT generation,credential_hash,revoked FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![cluster.into()]).await?.ok_or_else(||error(401,"Cluster authentication failed."))?;
        let stored: String = row.try_get("", "credential_hash").map_err(db_error)?;
        if row.try_get::<bool>("", "revoked").map_err(db_error)?
            || row.try_get::<i32>("", "generation").map_err(db_error)? != generation
            || !bool::from(stored.as_bytes().ct_eq(credential_hash.as_bytes()))
        {
            return Err(error(401, "Cluster authentication failed."));
        }
        Ok(AgentIdentity {
            cluster: cluster.into(),
            generation,
            credential_hash: stored,
        })
    }
    pub(super) async fn verify_agent(
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
    ) -> Result<Value> {
        let row=rows(tx,"SELECT to_jsonb(a) AS value FROM webdock_auth.hosting_agent a WHERE cluster_id=$1 AND NOT revoked FOR SHARE",vec![agent.cluster.clone().into()]).await?.pop().ok_or_else(||error(401,"Cluster authentication failed."))?;
        if numeric(&row, "generation")? != i64::from(agent.generation)
            || !bool::from(
                field(&row, "credential_hash")?
                    .as_bytes()
                    .ct_eq(agent.credential_hash.as_bytes()),
            )
        {
            return Err(error(401, "Cluster authentication failed."));
        }
        Ok(row)
    }
    /// Backend domain integration only. The opaque identity must come from authenticated
    /// enrollment. Locks current agent credentials through transaction completion and applies
    /// the existing cluster-limited agent scope; domain row/lease authorization is still required.
    pub async fn begin_authenticated_agent(
        &self,
        agent: &AgentIdentity,
    ) -> Result<DatabaseTransaction> {
        let tx = self.worker.begin().await.map_err(db_error)?;
        one(&tx,"SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",vec![]).await?;
        self.apply_scope(&tx,"webdock_worker_runtime",json!({"kind":"system","purpose":"agent","clusterID":agent.cluster,"authProvider":"better-auth"})).await?;
        Self::verify_agent(&tx, agent).await?;
        Ok(tx)
    }
    pub(super) async fn agent_tx(&self, agent: &AgentIdentity) -> Result<DatabaseTransaction> {
        let tx = self.begin_authenticated_agent(agent).await?;
        self.apply_scope(
            &tx,
            "webdock_worker_runtime",
            json!({"kind":"system","purpose":"plugin-admission","authProvider":"better-auth"}),
        )
        .await?;
        Self::module(&tx, true, true).await?;
        self.apply_scope(&tx,"webdock_worker_runtime",json!({"kind":"system","purpose":"agent","clusterID":agent.cluster,"authProvider":"better-auth"})).await?;
        Ok(tx)
    }
    /// Agent connectivity is installation-independent. An absent/disabled Hosting module offers
    /// no new dispatch, while existing guard and receipt methods keep their stricter checks.
    pub async fn agent_dispatch_available(&self, agent: &AgentIdentity) -> Result<bool> {
        let Some(tx) = self.dispatch_tx(agent).await? else {
            return Ok(false);
        };
        tx.commit().await.map_err(db_error)?;
        Ok(true)
    }
    pub(super) async fn dispatch_tx(
        &self,
        agent: &AgentIdentity,
    ) -> Result<Option<DatabaseTransaction>> {
        let tx = self.begin_authenticated_agent(agent).await?;
        self.apply_scope(
            &tx,
            "webdock_worker_runtime",
            json!({"kind":"system","purpose":"plugin-admission","authProvider":"better-auth"}),
        )
        .await?;
        one(
            &tx,
            "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))",
            vec![],
        )
        .await?;
        let row = one(
            &tx,
            "SELECT state FROM webdock_auth.plugin_installation WHERE id='hosting'",
            vec![],
        )
        .await?;
        let state = row
            .as_ref()
            .map(|row| row.try_get::<String>("", "state").map_err(db_error))
            .transpose()?;
        match state.as_deref() {
            None | Some("disabled") => {
                tx.commit().await.map_err(db_error)?;
                return Ok(None);
            }
            Some("enabled" | "draining") => {}
            Some(_) => return Err(error(503, "Hosting installation state is invalid.")),
        }
        self.apply_scope(&tx,"webdock_worker_runtime",json!({"kind":"system","purpose":"agent","clusterID":agent.cluster,"authProvider":"better-auth"})).await?;
        Ok(Some(tx))
    }
    pub(super) async fn accepted_authority(
        &self,
        tx: &DatabaseTransaction,
        op: &Value,
    ) -> Result<bool> {
        self.accepted_authority_with_guard(tx, op, None).await
    }
    pub(super) async fn accepted_authority_with_guard(
        &self,
        tx: &DatabaseTransaction,
        op: &Value,
        held_guard: Option<&browser_authority::BrowserGuard>,
    ) -> Result<bool> {
        let binding = one(
            tx,
            "SELECT webdock_rls.plugin_setup_operation_binding($1) AS binding",
            vec![field(op, "id")?.into()],
        )
        .await?;
        if binding.is_some_and(|row| {
            row.try_get::<Option<String>>("", "binding")
                .ok()
                .flatten()
                .is_some()
        }) {
            return Ok(false);
        }
        if op["git_release_id"].as_str().is_some() {
            if let Some(guard) = held_guard {
                guard.verify().await?;
            } else {
                let guard = match self.operation_browser_guard(op).await {
                    Ok(guard) => guard,
                    Err(e) if e.status == 403 => return Ok(false),
                    Err(e) => return Err(e),
                };
                browser_authority::verify(&guard).await?;
            }
            let authority = rows(
                tx,
                "SELECT coalesce(webdock_rls.git_operation_authority($1),'null'::jsonb) AS value",
                vec![field(op, "id")?.into()],
            )
            .await?
            .pop()
            .unwrap_or(Value::Null);
            return Ok(!authority.is_null()
                && authority["subject"] == op["subject"]
                && authority["projectID"] == op["project_id"]
                && authority["targetID"] == op["app_id"]
                && matches!(op["action"].as_str(), Some("apps.deploy" | "apps.update")));
        }
        let Some(session) = op["authority_session"]
            .as_str()
            .or_else(|| op["actor_session"].as_str())
        else {
            return Ok(false);
        };
        let actor = op["authority_subject"]
            .as_str()
            .unwrap_or(field(op, "subject")?);
        let check = self.db.begin().await.map_err(db_error)?;
        self.apply_scope(&check,"webdock_auth_runtime",json!({"kind":"session","authProvider":"better-auth","subject":actor,"sessionID":session})).await?;
        let write = !is_read_action(field(op, "action")?);
        let row=one(&check,"SELECT webdock_rls.customer_scope() AS customer_scope,webdock_rls.actor() AS actor,webdock_rls.customer_permission(p.customer_id,'org:projects:manage') AS write_allowed,webdock_rls.customer_allowed(p.customer_id,false) AS read_allowed,h.mode FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id JOIN webdock_auth.hosting_project h ON h.project_id=p.id WHERE p.id=$1 AND p.customer_id=$2 AND h.cluster_id=$3 AND p.status='active' AND c.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')",vec![field(op,"project_id")?.into(),field(op,"customer_id")?.into(),field(op,"cluster_id")?.into()]).await?;
        let allowed = if let Some(row) = row {
            let identity: Option<Value> = row.try_get("", "actor").map_err(db_error)?;
            let operator = identity.as_ref().is_some_and(|i| i["operator"] == true);
            let current_scope: Option<String> =
                row.try_get("", "customer_scope").map_err(db_error)?;
            if current_scope.as_deref() != op["desired"]["customerScope"].as_str() {
                return Ok(false);
            }
            identity.as_ref().is_some_and(|i| {
                i["subject"] == actor
                    && i["sessionID"] == session
                    && (i["preview"].is_null() || i["preview"]["customer_id"] == op["customer_id"])
            }) && row.try_get::<bool>("", "read_allowed").map_err(db_error)?
                && (!write
                    || (row.try_get::<bool>("", "write_allowed").map_err(db_error)?
                        && (operator
                            || row.try_get::<String>("", "mode").map_err(db_error)?
                                == "selfservice")))
                && (!["apps.delete", "apps.purgeStorage"].contains(&field(op, "action")?)
                    || operator)
        } else {
            false
        };
        check.commit().await.map_err(db_error)?;
        Ok(allowed)
    }
    pub async fn agent_request(
        &self,
        agent: &AgentIdentity,
        action: &str,
        input: Value,
    ) -> Result<Value> {
        if serde_json::to_vec(&input)
            .map_err(|_| error(400, "Invalid agent input."))?
            .len()
            > 65536
        {
            return Err(error(400, "Agent input exceeds limit."));
        }
        if !["heartbeat", "claim", "guard", "complete"].contains(&action) {
            return Err(error(404, "Agent endpoint is unavailable."));
        }
        let rate = if action == "heartbeat" {
            self.begin_authenticated_agent(agent).await?
        } else {
            self.agent_tx(agent).await?
        };
        let row=one(&rate,"INSERT INTO webdock_auth.hosting_rate(key_hash) VALUES($1) ON CONFLICT(key_hash) DO UPDATE SET requests=CASE WHEN hosting_rate.window_start<now()-interval '1 minute' THEN 1 ELSE hosting_rate.requests+1 END,window_start=CASE WHEN hosting_rate.window_start<now()-interval '1 minute' THEN now() ELSE hosting_rate.window_start END RETURNING requests",vec![hex::encode(Sha256::digest(format!("heartbeat:{}",agent.cluster))).into()]).await?.unwrap();
        let requests: i32 = row.try_get("", "requests").map_err(db_error)?;
        rate.commit().await.map_err(db_error)?;
        if requests > 120 {
            return Err(error(429, "Agent request rate exceeded."));
        }
        let tx = if action == "heartbeat" {
            self.begin_authenticated_agent(agent).await?
        } else {
            self.agent_tx(agent).await?
        };
        let before_guard = if action == "complete" || action == "guard" {
            let op = Self::operation(&tx, agent, field(&input, "id")?, false).await?;
            if op["status"] == "running" {
                self.operation_browser_guard(&op).await?
            } else {
                None
            }
        } else {
            None
        };
        let completion_operation = if action == "complete" {
            Some(Self::operation(&tx, agent, field(&input, "id")?, true).await?)
        } else {
            None
        };
        let result = match action {
            "heartbeat" => {
                contracts::observation(&input)?;
                if numeric(&input, "generation")? != i64::from(agent.generation)
                    || epoch(&input["observedAt"])
                        .is_none_or(|time| time > now() + 30 || time < now() - 90)
                {
                    return Err(error(
                        409,
                        "Observation is stale or belongs to another generation.",
                    ));
                }
                let kind = one(
                    &tx,
                    "SELECT runtime_kind FROM webdock_auth.hosting_cluster WHERE id=$1",
                    vec![agent.cluster.clone().into()],
                )
                .await?
                .unwrap()
                .try_get::<String>("", "runtime_kind")
                .map_err(db_error)?;
                if input
                    .get("runtimeKind")
                    .and_then(Value::as_str)
                    .unwrap_or("kubernetes")
                    != kind
                {
                    return Err(error(
                        409,
                        "Observation runtime kind does not match enrollment.",
                    ));
                }
                if execute(&tx,"UPDATE webdock_auth.hosting_agent SET observation=$2,last_seen=now(),last_sequence=$3 WHERE cluster_id=$1 AND last_sequence<$3 AND generation=$4 AND NOT revoked",vec![agent.cluster.clone().into(),input.clone().into(),numeric(&input,"sequence")?.into(),agent.generation.into()]).await?!=1{return Err(error(409,"Observation sequence was already processed."));}
                json!({"accepted":true})
            }
            "claim" => {
                if input != json!({}) {
                    return Err(error(400, "Invalid claim input."));
                }
                self.claim(&tx, agent).await?
            }
            "guard" => {
                keys(&input, &["id", "generation"], &[])?;
                let op = Self::operation(&tx, agent, field(&input, "id")?, false).await?;
                if !is_read_action(field(&op, "action")?) {
                    Self::ensure_app_effect_allowed(&tx, field(&op, "app_id")?, None).await?;
                }
                Self::lease(&op, numeric(&input, "generation")?)?;
                if !self
                    .accepted_authority_with_guard(&tx, &op, before_guard.as_ref())
                    .await?
                {
                    return Err(error(403, "Operation authority changed."));
                }
                json!({"valid":true})
            }
            _ => {
                self.complete_with_runtime(&tx, agent, &input, false, before_guard.as_ref())
                    .await?
            }
        };
        let claim_guard = if action == "claim" && !result.is_null() {
            let op = Self::operation(&tx, agent, field(&result, "id")?, false).await?;
            self.operation_browser_guard(&op).await?
        } else {
            None
        };
        Self::verify_agent(&tx, agent).await?;
        browser_authority::verify(&before_guard).await?;
        browser_authority::verify(&claim_guard).await?;
        if let Some(op) = completion_operation
            .as_ref()
            .filter(|op| op["status"] == "running")
        {
            self.admit_completion_commit(&tx, op, None, before_guard.as_ref())
                .await?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }
    pub(super) async fn operation(
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        id: &str,
        lock: bool,
    ) -> Result<Value> {
        contracts::record_id(id)?;
        rows(tx,if lock{"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1 AND cluster_id=$2 FOR UPDATE"}else{"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1 AND cluster_id=$2"},vec![id.into(),agent.cluster.clone().into()]).await?.pop().ok_or_else(||error(409,"Operation lease changed."))
    }
    pub(super) fn lease(op: &Value, generation: i64) -> Result<()> {
        if numeric(op, "generation")? != generation
            || op["status"] != "running"
            || epoch(&op["lease_until"]).is_none_or(|time| time <= now())
        {
            return Err(error(
                409,
                "Operation lease expired or changed. Reconcile before retrying.",
            ));
        }
        Ok(())
    }
    pub(super) async fn claim(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
    ) -> Result<Value> {
        self.claim_filtered(tx, agent, false).await
    }
    pub(super) async fn claim_filtered(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        native: bool,
    ) -> Result<Value> {
        self.claim_filtered_mode(tx, agent, if native { 1 } else { 0 })
            .await
    }
    pub(super) async fn claim_filtered_mode(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        mode: i32,
    ) -> Result<Value> {
        let agent_state = Self::verify_agent(tx, agent).await?;
        execute(tx,"UPDATE webdock_auth.hosting_operation SET status=CASE WHEN action IN ('apps.logs','apps.inspect','apps.metrics') THEN 'failed' ELSE 'needs-reconciliation' END WHERE cluster_id=$1 AND status='running' AND lease_until<now()",vec![agent.cluster.clone().into()]).await?;
        let Some(op)=rows(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE o.cluster_id=$1 AND o.status='queued' AND o.action LIKE 'apps.%' AND o.desired IS NOT NULL AND (NOT(o.desired ? 'publicRoutingVersion') OR ($2::integer=1 AND $6::boolean)) AND (($2::integer=0 AND o.action NOT IN ('apps.inspect','apps.metrics')) OR ($2=2 AND o.action IN ('apps.logs','apps.inspect','apps.metrics') AND o.desired->>'expectedUID' IS NOT NULL) OR ($2=1 AND o.desired->>'action' IN ('apply','delete','purge-storage') AND NOT (o.desired ?| ARRAY['serviceBindingsVersion']) AND (o.desired->>'action'<>'apply' OR o.action='apps.stop' OR NOT (o.desired ? 'environmentVersion') OR $3::boolean) AND (COALESCE((o.desired->'spec'->>'volumeBytes')::bigint,0)=0 OR ($4::boolean AND (o.desired->>'action'<>'apply' OR $5::boolean))) AND (o.desired->'spec'->>'replicas')::integer BETWEEN 0 AND 20 AND ((SELECT runtime_kind FROM webdock_auth.hosting_cluster c WHERE c.id=o.cluster_id)<>'docker' OR (o.desired->'spec'->>'replicas')::integer<=1) AND length(o.desired->'spec'->>'image')<=512 AND o.desired->'spec'->>'image' ~ '^[a-z0-9]+([.-][a-z0-9]+)*(:[0-9]{1,5})?/([a-z0-9]+([._-][a-z0-9]+)*/)*[a-z0-9]+([._-][a-z0-9]+)*@sha256:[a-f0-9]{64}$' AND NOT EXISTS(SELECT 1 FROM jsonb_array_elements_text(o.desired->'spec'->'args') arg WHERE octet_length(arg)>2048))) AND NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_operation busy WHERE busy.project_id=o.project_id AND (busy.status='running' OR(busy.status='needs-reconciliation' AND o.action NOT IN ('apps.logs','apps.inspect','apps.metrics')))) ORDER BY o.created_at,o.id FOR UPDATE SKIP LOCKED LIMIT 1",vec![agent.cluster.clone().into(),mode.into(),(agent_state["observation"]["capabilities"]["environment"]==true).into(),(agent_state["observation"]["capabilities"]["storageVersion"]==1).into(),(agent_state["observation"]["capabilities"]["storage"]==true).into(),(agent_state["observation"]["capabilities"]["publicRoutingVersion"]==1).into()]).await?.pop()else{return Ok(Value::Null)};
        if mode != 2 && op["desired"].get("serviceBindingsVersion").is_some() {
            return Ok(Value::Null);
        }
        if mode != 2 && !is_read_action(field(&op, "action")?) {
            Self::ensure_app_effect_allowed(tx, field(&op, "app_id")?, None).await?;
        }
        if mode != 2
            && op["desired"]["storageVersion"] == 1
            && (agent_state["observation"]["capabilities"]["storageVersion"] != 1
                || (agent_state["observation"]["capabilities"]["storage"] != true
                    && !["apps.delete", "apps.purgeStorage", "apps.logs"]
                        .contains(&field(&op, "action")?)))
        {
            return Ok(Value::Null);
        }
        if mode != 2
            && op["desired"]["action"] == "apply"
            && op["action"] != "apps.stop"
            && op["desired"]["environmentVersion"] == 1
            && agent_state["observation"]["capabilities"]["environment"] != true
        {
            return Ok(Value::Null);
        }
        if !self.accepted_authority(tx, &op).await? {
            execute(tx,"UPDATE webdock_auth.hosting_operation SET status='failed',result=$2 WHERE id=$1",vec![field(&op,"id")?.into(),json!({"error":"Accepted operation authority changed. Reconcile with a current session."}).into()]).await?;
            if !is_read_action(field(&op, "action")?) {
                execute(tx,"UPDATE webdock_auth.hosting_app SET status='failed',last_error='Operation authority changed.' WHERE id=$1 AND revision=$2",vec![field(&op,"app_id")?.into(),numeric(&op,"target_revision")?.into()]).await?;
            }
            return Ok(Value::Null);
        }
        let lock = one(
            tx,
            "SELECT pg_try_advisory_xact_lock(hashtextextended($1,0)) AS locked",
            vec![format!("hosting-target:{}", field(&op, "project_id")?).into()],
        )
        .await?
        .unwrap()
        .try_get::<bool>("", "locked")
        .map_err(db_error)?;
        if !lock {
            return Ok(Value::Null);
        }
        if one(tx,"SELECT 1 FROM webdock_auth.hosting_operation WHERE project_id=$1 AND(status='running' OR(status='needs-reconciliation' AND $2 NOT IN ('apps.logs','apps.inspect','apps.metrics')))",vec![field(&op,"project_id")?.into(),field(&op,"action")?.into()]).await?.is_some(){return Ok(Value::Null);}
        let mut claimed=rows(tx,"UPDATE webdock_auth.hosting_operation SET status='running',generation=generation+1,lease_until=now()+interval '300 seconds' WHERE id=$1 RETURNING jsonb_build_object('id',id,'action',action,'project_id',project_id,'generation',generation,'lease_until',lease_until,'desired',desired,'target_revision',target_revision) AS value",vec![field(&op,"id")?.into()]).await?.pop().unwrap();
        let reservations=rows(tx,"SELECT jsonb_build_object('demand',demand,'status',status) AS value FROM webdock_auth.hosting_reservation WHERE project_id=$1 AND status<>'released'",vec![field(&op,"project_id")?.into()]).await?;
        let apps=rows(tx,"SELECT to_jsonb(a) AS value FROM webdock_auth.hosting_app a WHERE project_id=$1 AND lifecycle='deployed' AND(status<>'deleted' OR COALESCE((spec->>'volumeBytes')::bigint,0)>0)",vec![field(&op,"project_id")?.into()]).await?;
        let mut final_rows = Vec::new();
        for app in apps {
            let target = app["id"] == op["app_id"];
            let deleted = app["status"] == "deleted"
                || (target
                    && ["delete", "purge-storage"]
                        .contains(&field(&claimed["desired"], "action")?));
            let mut spec = if target {
                claimed["desired"]["spec"].clone()
            } else {
                app["spec"].clone()
            };
            if target && claimed["desired"]["action"] == "purge-storage" {
                spec["volumeBytes"] = json!(0);
            }
            final_rows
                .push(json!({"demand":contracts::app_demand(&spec,deleted)?,"status":"active"}));
        }
        claimed["desired"]["quota"] = contracts::totals(&reservations)?;
        claimed["desired"]["finalQuota"] = contracts::totals(&final_rows)?;
        claimed["desired"]["operationID"] = claimed["id"].clone();
        claimed["desired"]["generation"] = claimed["generation"].clone();
        claimed["desired"]["leaseUntil"] = timestamp(&claimed["lease_until"]);
        if claimed["desired"]["environmentVersion"] == 1 {
            claimed["desired"]["environment"] = json!(self.environment.open(
                field(&op, "app_id")?,
                claimed["desired"]["environmentEncrypted"].as_str()
            )?);
            claimed["desired"]
                .as_object_mut()
                .unwrap()
                .remove("environmentEncrypted");
        }
        if !self.accepted_authority(tx, &op).await? {
            return Err(error(403, "Operation authority changed."));
        }
        Ok(claimed)
    }
    pub(super) async fn complete_with_runtime(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        input: &Value,
        native: bool,
        browser_guard: Option<&browser_authority::BrowserGuard>,
    ) -> Result<Value> {
        keys(input, &["id", "generation", "outcome"], &["proof", "error"])?;
        let id = field(input, "id")?;
        let op = Self::operation(tx, agent, id, true).await?;
        let outcome = field(input, "outcome")?;
        if !["succeeded", "failed"].contains(&outcome) || numeric(input, "generation")? <= 0 {
            return Err(error(400, "Invalid operation completion."));
        }
        if numeric(&op, "generation")? != numeric(input, "generation")? {
            return Err(error(409, "Operation lease changed."));
        }
        if op["status"] == "succeeded" && outcome == "succeeded" {
            if !is_read_action(field(&op, "action")?) {
                Self::ensure_app_effect_allowed(tx, field(&op, "app_id")?, Some(&op)).await?;
            }
            return Ok(json!({"id":id,"status":"succeeded"}));
        }
        if !is_read_action(field(&op, "action")?) {
            Self::ensure_app_effect_allowed(tx, field(&op, "app_id")?, None).await?;
        }
        Self::lease(&op, numeric(input, "generation")?)?;
        if !self
            .accepted_authority_with_guard(tx, &op, browser_guard)
            .await?
        {
            return Err(error(403, "Operation authority changed."));
        }
        let app_id = field(&op, "app_id")?;
        let action = field(&op, "action")?;
        if outcome == "failed" {
            if std::env::var("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref() == Ok("1") {
                eprintln!(
                    "recovery_component execution_failure code={}",
                    recovery_execution_class(&input["error"])
                );
            }
            let logging = action == "apps.logs";
            let message = if logging {
                "The application logs could not be read."
            } else {
                match input["error"].as_str() {
                    Some("admission_rejected") => {
                        "Runtime rejected the application safety profile."
                    }
                    Some("rollout_timeout") => {
                        "The application did not become ready. Inspect the healthcheck and resources."
                    }
                    Some("ownership_conflict") => {
                        "Managed resource ownership changed. Operator review is required."
                    }
                    _ => "Cluster execution failed. Inspect and reconcile before retrying.",
                }
            };
            let status = if logging {
                "failed"
            } else {
                "needs-reconciliation"
            };
            execute(
                tx,
                "UPDATE webdock_auth.hosting_operation SET status=$2,result=$3 WHERE id=$1",
                vec![id.into(), status.into(), json!({"error":message}).into()],
            )
            .await?;
            if logging {
                execute(tx,"UPDATE webdock_auth.hosting_app SET last_error=COALESCE(last_error,$2) WHERE id=$1",vec![app_id.into(),message.into()]).await?;
            } else {
                execute(tx,"UPDATE webdock_auth.hosting_app SET status=CASE WHEN $4 THEN 'deleted' ELSE 'failed' END,last_error=$2 WHERE id=$1 AND operation_id=$3",vec![app_id.into(),message.into(),id.into(),(action=="apps.purgeStorage").into()]).await?;
            }
            self.audit(
                tx,
                field(&op, "subject")?,
                &format!("{action}.complete"),
                app_id,
                "failed",
            )
            .await?;
            if !self
                .accepted_authority_with_guard(tx, &op, browser_guard)
                .await?
            {
                return Err(error(403, "Operation authority changed."));
            }
            return Ok(json!({"id":id,"status":status}));
        }
        let mut proof = input["proof"].clone();
        proof_input(&proof)?;
        if proof["appID"] != op["app_id"]
            || proof["revision"] != op["target_revision"]
            || proof["namespace"] != op["desired"]["namespace"]
        {
            return Err(error(409, "Workload proof does not match the operation."));
        }
        let app = rows(
            tx,
            "SELECT to_jsonb(a) AS value FROM webdock_auth.hosting_app a WHERE id=$1 FOR UPDATE",
            vec![app_id.into()],
        )
        .await?
        .pop()
        .ok_or_else(|| error(404, "Application unavailable."))?;
        if app["revision"] != op["target_revision"] {
            return Err(error(409, "Application revision changed."));
        }
        if action == "apps.logs" {
            if proof["uid"].as_str().is_none_or(str::is_empty)
                || proof["deleted"] == true
                || (!app["observed_uid"].is_null() && proof["uid"] != app["observed_uid"])
            {
                return Err(error(409, "Application log ownership changed."));
            }
            let mut logs = proof["logs"].as_str().unwrap_or("").to_owned();
            if let Some(encoded) = proof["logsBase64"].as_str() {
                let decoded = STANDARD
                    .decode(encoded)
                    .map_err(|_| error(400, "Invalid log encoding."))?;
                if STANDARD.encode(&decoded) != encoded {
                    return Err(error(400, "Invalid log encoding."));
                }
                logs = String::from_utf8_lossy(&decoded).into_owned();
            }
            let truncated = proof["logsTruncated"]
                .as_bool()
                .unwrap_or(logs.encode_utf16().count() >= 5996);
            let history=rows(tx,"SELECT jsonb_build_object('encrypted',desired->>'environmentEncrypted') AS value FROM webdock_auth.hosting_operation WHERE app_id=$1 AND desired->>'environmentEncrypted' IS NOT NULL",vec![app_id.into()]).await?;
            let mut values = Vec::new();
            for sealed in [
                app["environment_encrypted"].as_str(),
                app["previous_environment_encrypted"].as_str(),
                app["observed_environment_encrypted"].as_str(),
            ]
            .into_iter()
            .chain(history.iter().map(|r| r["encrypted"].as_str()))
            {
                values.extend(self.environment.open(app_id, sealed)?.into_values());
            }
            logs = crate::environment::redact(logs, values, truncated);
            proof.as_object_mut().unwrap().remove("logsBase64");
            proof["logs"] = json!(logs);
            execute(tx,"UPDATE webdock_auth.hosting_app SET logs=$2,last_error=CASE WHEN last_error='The application logs could not be read.' THEN NULL ELSE last_error END WHERE id=$1",vec![app_id.into(),logs.into()]).await?;
        } else {
            proof.as_object_mut().unwrap().remove("logs");
            let purge = op["desired"]["action"] == "purge-storage";
            let deleting = op["desired"]["action"] == "delete" || purge;
            let mut spec = op["desired"]["spec"].clone();
            if purge {
                spec["volumeBytes"] = json!(0);
            }
            if op["desired"]["storageVersion"] == 1
                && (proof["storage"]["volumeBytes"].as_u64()
                    != Some(spec["volumeBytes"].as_u64().unwrap_or(0))
                    || proof["storage"]["retained"] != json!(deleting && !purge))
            {
                return Err(error(
                    409,
                    "Persistent storage proof does not match the operation.",
                ));
            }
            if op["desired"].get("serviceBindingsVersion").is_some() {
                return Err(error(
                    503,
                    "Service binding completion adapter unavailable.",
                ));
            }
            if proof["ready"] != true
                || proof["deleted"] != json!(deleting)
                || (!deleting
                    && (proof["uid"].as_str().is_none_or(str::is_empty)
                        || proof["replicas"] != spec["replicas"]))
                || (deleting && (!proof["uid"].is_null() || proof["replicas"] != 0))
            {
                return Err(error(
                    409,
                    "Application has not converged to the requested state.",
                ));
            }
            if !native
                && !app["observed_uid"].is_null()
                && !proof["uid"].is_null()
                && app["observed_uid"] != proof["uid"]
            {
                return Err(error(409, "Application ownership changed."));
            }
            execute(tx,"UPDATE webdock_auth.hosting_reservation SET status='released' WHERE app_id=$1 AND operation_id<>$2",vec![app_id.into(),id.into()]).await?;
            execute(tx,"UPDATE webdock_auth.hosting_reservation SET status=$2,demand=$3 WHERE operation_id=$1",vec![id.into(),if deleting&&spec["volumeBytes"].as_u64().unwrap_or(0)==0{"released"}else{"active"}.into(),contracts::app_demand(&spec,deleting)?.into()]).await?;
            execute(tx,"UPDATE webdock_auth.hosting_app SET status=$2,observed_revision=$3,observed_uid=$4,observed_spec=$5,observed_environment_encrypted=$6,last_error=NULL WHERE id=$1",vec![app_id.into(),if deleting{"deleted"}else if proof["replicas"]==0{"stopped"}else{"ready"}.into(),numeric(&proof,"revision")?.into(),proof["uid"].as_str().into(),spec.clone().into(),op["desired"]["environmentEncrypted"].as_str().into()]).await?;
            if purge {
                execute(
                    tx,
                    "UPDATE webdock_auth.hosting_app SET spec=$2,previous_spec=NULL WHERE id=$1",
                    vec![app_id.into(), spec.into()],
                )
                .await?;
            }
            if deleting {
                execute(tx,"UPDATE webdock_auth.hosting_app SET environment_encrypted=NULL,previous_environment_encrypted=NULL,observed_environment_encrypted=NULL,logs=NULL WHERE id=$1",vec![app_id.into()]).await?;
                execute(tx,"UPDATE webdock_auth.hosting_operation SET desired=desired-'environmentEncrypted' WHERE app_id=$1",vec![app_id.into()]).await?;
            }
        }
        proof.as_object_mut().unwrap().remove("logsBase64");
        proof.as_object_mut().unwrap().remove("logsTruncated");
        execute(tx,"UPDATE webdock_auth.hosting_operation SET status='succeeded',result=$2 WHERE id=$1",vec![id.into(),json!({"appID":app_id,"status":if ["delete","purge-storage"].contains(&field(&op["desired"],"action")?){"deleted"}else{"observed"},"proof":proof}).into()]).await?;
        self.audit(
            tx,
            field(&op, "subject")?,
            &format!("{action}.complete"),
            app_id,
            "succeeded",
        )
        .await?;
        if !self
            .accepted_authority_with_guard(tx, &op, browser_guard)
            .await?
        {
            return Err(error(403, "Operation authority changed."));
        }
        Ok(json!({"id":id,"status":"succeeded","appID":app_id}))
    }
}
fn keys(value: &Value, required: &[&str], optional: &[&str]) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| error(400, "Invalid agent input."))?;
    if required.iter().any(|k| !object.contains_key(*k))
        || object
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err(error(400, "Invalid agent input."));
    }
    Ok(())
}
fn proof_input(value: &Value) -> Result<()> {
    keys(
        value,
        &[
            "appID",
            "revision",
            "namespace",
            "uid",
            "ready",
            "deleted",
            "replicas",
        ],
        &[
            "logs",
            "logsBase64",
            "logsTruncated",
            "serviceBindings",
            "storage",
        ],
    )?;
    contracts::record_id(field(value, "appID")?)?;
    if numeric(value, "revision")? <= 0
        || !regex::Regex::new("^wd-[1-9][0-9]*$")
            .unwrap()
            .is_match(field(value, "namespace")?)
        || (!value["uid"].is_null()
            && value["uid"]
                .as_str()
                .is_none_or(|v| v.is_empty() || v.encode_utf16().count() > 128))
        || !value["ready"].is_boolean()
        || !value["deleted"].is_boolean()
        || amount(&value["replicas"])? > 20
    {
        return Err(error(400, "Invalid workload proof."));
    }
    for key in ["logs", "logsBase64"] {
        if let Some(text) = value.get(key)
            && text
                .as_str()
                .is_none_or(|s| s.encode_utf16().count() > 16000)
        {
            return Err(error(400, "Invalid log proof."));
        }
    }
    if let Some(storage) = value.get("storage") {
        keys(storage, &["volumeBytes", "retained"], &[])?;
        amount(&storage["volumeBytes"])?;
        if !storage["retained"].is_boolean() {
            return Err(error(400, "Invalid storage proof."));
        }
    }
    Ok(())
}

fn recovery_execution_class(value: &Value) -> &'static str {
    [
        "authorization_denied",
        "lease_expired",
        "generation_fenced",
        "operation_conflict",
        "ownership_conflict",
        "capability_unavailable",
        "invalid_command",
        "journal_unavailable",
        "runtime_unavailable",
    ]
    .into_iter()
    .find(|code| Some(*code) == value.as_str())
    .unwrap_or("unknown")
}
#[cfg(test)]
mod recovery_execution_diagnostic_tests {
    use super::*;
    #[test]
    fn completion_codes_are_static_and_never_echo_unknown_material() {
        for code in [
            "authorization_denied",
            "lease_expired",
            "generation_fenced",
            "operation_conflict",
            "ownership_conflict",
            "capability_unavailable",
            "invalid_command",
            "journal_unavailable",
            "runtime_unavailable",
        ] {
            assert_eq!(recovery_execution_class(&json!(code)), code);
        }
        for value in [
            Value::Null,
            json!("secret-must-not-leave"),
            json!("runtime_unavailable\nsecret"),
            json!({"error":"secret"}),
        ] {
            assert_eq!(recovery_execution_class(&value), "unknown");
        }
    }
}
