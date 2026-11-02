use super::*;
use webdock_plugin_protocol::{ErrorCode, JobIdentity};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Enqueue {
    name: String,
    payload: Value,
    idempotency_key: String,
    #[serde(default = "default_attempts")]
    attempts: u8,
}
fn default_attempts() -> u8 {
    3
}
// Job payloads are Rust JSON: preserve Number's u64/f64 representation, sorting only object keys.
pub(super) fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    fn ordered(value: &Value, depth: usize) -> Result<Value> {
        if depth > 64 {
            return Err(invalid("Job JSON nesting exceeds limit"));
        }
        Ok(match value {
            Value::Object(object) => {
                let mut entries = object.iter().collect::<Vec<_>>();
                entries.sort_unstable_by_key(|(key, _)| *key);
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| Ok((key.clone(), ordered(value, depth + 1)?)))
                        .collect::<Result<_>>()?,
                )
            }
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|v| ordered(v, depth + 1))
                    .collect::<Result<_>>()?,
            ),
            _ => value.clone(),
        })
    }
    serde_json::to_vec(&ordered(value, 0)?).map_err(|_| invalid("Invalid job JSON"))
}
fn job_id(input: &Value) -> Result<&str> {
    let id = input["id"]
        .as_str()
        .ok_or_else(|| invalid("Invalid job ID"))?;
    if input.as_object().is_none_or(|o| o.len() != 1)
        || id.len() != 68
        || !id.starts_with("job-")
        || !id.as_bytes()[4..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return Err(invalid("Invalid job ID"));
    }
    Ok(id)
}
fn not_found() -> PluginError {
    PluginError::new(ErrorCode::NotFound, "Job not found")
}
fn result_value(value: Option<Value>) -> Result<Value> {
    let value = value.ok_or_else(not_found)?;
    if value["conflict"] == true {
        return Err(conflict("Job request conflicts with retained state"));
    }
    Ok(value)
}

/// A private claim, never a caller-supplied authorization or public job snapshot.
pub struct JobLease {
    id: String,
    token: uuid::Uuid,
    manifest: Manifest,
    invocation: AdmittedInvocation,
}
impl JobLease {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn plugin(&self) -> &str {
        &self.invocation.claims.plugin_id
    }
    pub fn generation(&self) -> u64 {
        self.invocation.claims.generation
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn invocation(&self) -> &AdmittedInvocation {
        &self.invocation
    }
}
impl HostService {
    async fn job_binding(&self, plugin: &str, metadata: &Metadata) -> Result<RuntimeBinding> {
        // A readable enrollment with no entry confirms removal. Read/parse failures remain transient.
        let binding = self
            .bindings
            .runtime(plugin)
            .await?
            .ok_or_else(PluginError::denied)?;
        self.checked_binding(metadata, binding)
    }
    async fn finish_unclaimed_job(&self, row: &Value) -> Result<bool> {
        // An expired create lease can hide an external resource; never dispatch another attempt.
        let uncertain_create = row["provenance"]["session_context"]["resourceAction"]["effect"]
            == "create"
            && integer(row, "attempts")? > 0;
        if row["cancel_requested"] != true
            && integer(row, "attempts")? < integer(row, "max_attempts")?
            && !uncertain_create
        {
            return Ok(false);
        }
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        Self::lock_job_metadata(&tx, row).await?;
        let updated=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "UPDATE webdock_auth.plugin_job_execution SET status=CASE WHEN cancel_requested THEN 'cancelled' ELSE 'failed' END,condition=CASE WHEN cancel_requested THEN 'cancelled' WHEN $2 THEN 'attempt_failed' ELSE 'attempts_exhausted' END,claim_token=NULL,lease_until=NULL WHERE id=$1 AND (status='pending' OR status='running' AND lease_until<=clock_timestamp()) AND (cancel_requested OR attempts>=max_attempts OR $2) RETURNING invocation_id",
            [string(row,"id")?.into(),uncertain_create.into()])).await.map_err(db_error)?;
        if let Some(updated) = &updated {
            let invocation: Option<uuid::Uuid> =
                updated.try_get("", "invocation_id").map_err(db_error)?;
            if let Some(invocation) = invocation {
                Self::revoke_job_invocation(&tx, &invocation.to_string()).await?;
            }
        }
        tx.commit().await.map_err(db_error)?;
        Ok(updated.is_some())
    }
    fn job_declaration<'a>(&self, metadata: &'a Metadata, name: &str) -> Result<&'a Value> {
        let job = metadata
            .manifest
            .job(name)
            .ok_or_else(PluginError::denied)?;
        if job.get("intervalSeconds").is_some() {
            return Err(unsupported("Scheduled jobs are unavailable"));
        }
        for cap in job["capabilities"].as_array().ok_or_else(unavailable)? {
            if ![
                "plugin.data.read",
                "plugin.data.write",
                "plugin.http.read",
                "plugin.http.request",
            ]
            .contains(&cap.as_str().unwrap_or(""))
            {
                return Err(unsupported("Job host capability is unavailable"));
            }
            if !metadata.configuration["approved_capabilities"]
                .as_array()
                .is_some_and(|c| c.contains(cap))
                || !metadata.grant["capabilities"]
                    .as_array()
                    .is_some_and(|c| c.contains(cap))
            {
                return denied();
            }
        }
        if !metadata.grant["jobs"]
            .as_array()
            .is_some_and(|j| j.iter().any(|j| j == name))
        {
            return denied();
        }
        Ok(job)
    }
    pub async fn jobs_enqueue(&self, handle: &InvocationHandle, input: Value) -> Result<Value> {
        if handle.claims.job.is_some() {
            return denied();
        }
        let input: Enqueue =
            serde_json::from_value(input).map_err(|_| invalid("Invalid job request"))?;
        if !(1..=5).contains(&input.attempts)
            || input.name.is_empty()
            || input.name.len() > 64
            || !input.name.as_bytes()[0].is_ascii_lowercase()
            || !input
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
            || !(8..=128).contains(&input.idempotency_key.len())
            || !input
                .idempotency_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || canonical_json(&input.payload)?.len() > 32768
        {
            return Err(invalid("Invalid job request"));
        }
        let (source, metadata, _) = self.persisted(&handle.claims).await?;
        if source.row["intent"] != "write"
            || source.row["session_context"]["authority"].is_null()
            || metadata.installation["state"] != "enabled"
        {
            return denied();
        }
        let job = self.job_declaration(&metadata, &input.name)?;
        validate_value(job.get("input"), &input.payload)?;
        let context = self.authorized(handle, Some("plugin.jobs.enqueue")).await?;
        let id = format!(
            "job-{}",
            digest(crate::data::javascript_json(
                &json!([
                    [
                        context.scope.plugin_id,
                        context.scope.customer_id,
                        context.scope.project_id
                    ],
                    input.idempotency_key
                ]),
                true
            )?)
        );
        let fingerprint = digest(canonical_json(
            &json!({"name":input.name,"payload":input.payload,"attempts":input.attempts}),
        )?);
        let value = context
            .tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.plugin_job_enqueue($1,$2,$3,$4,$5,$6) AS value",
                [
                    id.into(),
                    input.name.into(),
                    input.idempotency_key.into(),
                    input.payload.into(),
                    i32::from(input.attempts).into(),
                    fingerprint.into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(not_found)?
            .try_get::<Option<Value>>("", "value")
            .map_err(db_error)?;
        let value = result_value(value)?;
        self.finish(context, handle, Some("plugin.jobs.enqueue"))
            .await?;
        Ok(value)
    }
    pub async fn jobs_get(&self, handle: &InvocationHandle, input: Value) -> Result<Value> {
        let id = job_id(&input)?;
        let context = self.authorized(handle, Some("plugin.jobs.read")).await?;
        let value = context
            .tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.plugin_job_get($1,'plugin.jobs.read') AS value",
                [id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(not_found)?
            .try_get::<Option<Value>>("", "value")
            .map_err(db_error)?;
        let value = result_value(value)?;
        self.finish(context, handle, Some("plugin.jobs.read"))
            .await?;
        Ok(value)
    }
    pub async fn jobs_cancel(&self, handle: &InvocationHandle, input: Value) -> Result<Value> {
        let id = job_id(&input)?;
        let context = self.authorized(handle, Some("plugin.jobs.cancel")).await?;
        let value = context
            .tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.plugin_job_cancel($1) AS value",
                [id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(not_found)?
            .try_get::<Option<Value>>("", "value")
            .map_err(db_error)?;
        let value = result_value(value)?;
        self.finish(context, handle, Some("plugin.jobs.cancel"))
            .await?;
        Ok(value)
    }

    pub(super) async fn persisted_job(
        &self,
        claims: &InvocationClaims,
    ) -> Result<(PersistedInvocation, Metadata, RuntimeBinding)> {
        let identity = claims.job.as_ref().ok_or_else(PluginError::denied)?;
        let now = chrono::Utc::now().timestamp();
        if claims.v != 1
            || claims.iat > now
            || claims.exp <= now
            || !(1..=60).contains(&(claims.exp - claims.iat))
        {
            return denied();
        }
        let tx = self.controller().await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT to_jsonb(v) AS invocation,to_jsonb(j) AS job FROM webdock_auth.plugin_invocation v JOIN webdock_auth.plugin_job_execution j ON j.id=v.job_execution_id WHERE v.id=$1::uuid AND v.kind='job' AND v.revoked_at IS NULL AND v.expires_at>clock_timestamp() AND j.status='running' AND NOT j.cancel_requested AND j.claim_token=v.job_claim_token AND j.invocation_id=v.id AND j.lease_until>clock_timestamp()",[claims.id.clone().into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let inv: Value = row.try_get("", "invocation").map_err(db_error)?;
        let job: Value = row.try_get("", "job").map_err(db_error)?;
        let source = &job["provenance"];
        if job["id"] != identity.id
            || job["attempts"] != identity.attempt
            || inv["plugin_id"] != claims.plugin_id
            || inv["project_id"] != claims.project_id
            || inv["customer_id"] != claims.customer_id
            || inv["name"] != claims.action
            || integer(&inv, "runtime_generation")? != claims.generation as i64
            || inv["actor_id"] != source["actor_id"]
            || inv["session_id"] != source["session_id"]
            || inv["session_context"] != source["session_context"]
            || inv["session_context"]["authority"].is_null()
            || inv["operation_id"] != job["id"]
            || chrono::DateTime::parse_from_rfc3339(string(&inv, "expires_at")?)
                .map_err(|_| unavailable())?
                .timestamp()
                != claims.exp
        {
            return denied();
        }
        let metadata = self
            .metadata(&tx, &claims.plugin_id, Some(&claims.project_id))
            .await?;
        let binding = self.job_binding(&claims.plugin_id, &metadata).await?;
        let configuration = Self::current_configuration(&metadata, &binding, &claims.project_id)?;
        self.check_job_source(&tx, &job, &metadata, &binding, &configuration)
            .await?;
        if inv["capabilities"] != self.job_declaration(&metadata, &claims.action)?["capabilities"]
            || inv["intent"] != "write"
        {
            return denied();
        }
        tx.commit().await.map_err(db_error)?;
        Ok((
            PersistedInvocation {
                id: claims.id.clone(),
                actor: string(&inv, "actor_id")?.into(),
                session: string(&inv, "session_id")?.into(),
                row: inv,
            },
            metadata,
            binding,
        ))
    }

    async fn check_job_source(
        &self,
        tx: &DatabaseTransaction,
        row: &Value,
        metadata: &Metadata,
        binding: &RuntimeBinding,
        configuration: &Value,
    ) -> Result<()> {
        let source = &row["provenance"];
        if source["kind"] != "action"
            || source["intent"] != "write"
            || source["session_context"]["authority"].is_null()
            || source["plugin_id"] != row["plugin_id"]
            || source["project_id"] != row["project_id"]
            || source["customer_id"] != row["customer_id"]
            || source["runtime_generation"] != row["runtime_generation"]
            || source["grant_revision"] != row["grant_revision"]
            || metadata.grant["customer_id"] != row["customer_id"]
            || metadata.grant["revision"] != row["grant_revision"]
            || !metadata.grant["revoked_at"].is_null()
            || integer(row, "runtime_generation")? != binding.generation as i64
            || ![json!("enabled"), json!("draining")].contains(&metadata.installation["state"])
            || source["session_context"]["nativeAdmission"]
                != Self::admission_snapshot(metadata, binding, configuration)?
        {
            return denied();
        }
        let parent = self.action(metadata, string(source, "name")?)?;
        if parent["intent"] != "write"
            || parent["capabilities"] != source["capabilities"]
            || !parent["capabilities"]
                .as_array()
                .is_some_and(|c| c.contains(&json!("plugin.jobs.enqueue")))
        {
            return denied();
        }
        self.job_declaration(metadata, string(row, "name")?)?;
        self_service::gate(
            tx,
            source["session_context"]["operator"] == true,
            parent,
            string(row, "plugin_id")?,
            string(row, "customer_id")?,
            string(row, "project_id")?,
            source["session_context"]["resourceID"].as_str(),
        )
        .await
    }

    async fn job_lookup(&self, id: &str) -> Result<Value> {
        let tx = self.controller().await?;
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT to_jsonb(j) AS job FROM webdock_auth.plugin_job_execution j WHERE id=$1",
                [id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?
            .try_get("", "job")
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(row)
    }

    async fn lock_job_metadata(tx: &DatabaseTransaction, row: &Value) -> Result<()> {
        let allowed = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.lock_plugin_job_metadata($1) AS allowed",
                [string(row, "id")?.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?
            .try_get::<bool>("", "allowed")
            .map_err(db_error)?;
        if !allowed {
            return denied();
        }
        Ok(())
    }

    async fn job_authorization(&self, row: &Value) -> Result<JobAuthorization> {
        let source = &row["provenance"];
        let reference: AuthorityReference =
            serde_json::from_value(source["session_context"]["authority"].clone())
                .map_err(|_| PluginError::denied())?;
        let (auth, principal) = self
            .session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?
            .begin_principal(
                string(source, "actor_id")?,
                string(source, "session_id")?,
                &reference,
                true,
            )
            .await?;
        if source["session_context"]["customerScope"].as_str() != principal.customer_scope() {
            return denied();
        }
        let session = self
            .signer
            .begin(&self.db, &principal)
            .await
            .map_err(|_| unavailable())?;
        timeouts(&session).await?;
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        Self::lock_job_metadata(&tx, row).await?;
        if Self::project(&session, string(row, "project_id")?, true).await?
            != string(row, "customer_id")?
        {
            return denied();
        }
        let operator: bool = session
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator() AS value",
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?
            .try_get("", "value")
            .map_err(db_error)?;
        if source["session_context"]["operator"] != operator {
            return denied();
        }
        let metadata = self
            .metadata(
                &tx,
                string(row, "plugin_id")?,
                Some(string(row, "project_id")?),
            )
            .await?;
        let binding = self
            .job_binding(string(row, "plugin_id")?, &metadata)
            .await?;
        let configuration =
            Self::current_configuration(&metadata, &binding, string(row, "project_id")?)?;
        self.check_job_source(&tx, row, &metadata, &binding, &configuration)
            .await?;
        Ok(JobAuthorization {
            tx,
            session,
            auth,
            metadata,
            configuration,
            reference,
            subject: principal.user_id,
            session_id: principal.session_id,
        })
    }

    async fn cancel_invalid_job(&self, row: &Value, token: Option<uuid::Uuid>) -> Result<()> {
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        Self::lock_job_metadata(&tx, row).await?;
        let changed=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "UPDATE webdock_auth.plugin_job_execution SET status='cancelled',cancel_requested=true,claim_token=NULL,lease_until=NULL,condition='authority_revoked' WHERE id=$1 AND coalesce(provenance,'null'::jsonb)=$2 AND status IN ('pending','running') AND (($3::uuid IS NOT NULL AND claim_token=$3::uuid AND lease_until>clock_timestamp()) OR ($3::uuid IS NULL AND (status='pending' OR lease_until<=clock_timestamp()))) RETURNING invocation_id",
            [string(row,"id")?.into(),row["provenance"].clone().into(),token.map(|t|t.to_string()).into()])).await.map_err(db_error)?;
        if let Some(changed) = changed {
            let invocation: Option<uuid::Uuid> =
                changed.try_get("", "invocation_id").map_err(db_error)?;
            if let Some(invocation) = invocation {
                Self::revoke_job_invocation(&tx, &invocation.to_string()).await?;
            }
        }
        tx.commit().await.map_err(db_error)
    }
    async fn revoke_job_invocation(tx: &DatabaseTransaction, id: &str) -> Result<()> {
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_invocation SET revoked_at=coalesce(revoked_at,now()) WHERE id=$1::uuid",[id.into()])).await.map_err(db_error)?;
        Ok(())
    }
    pub async fn claim_job(&self, plugin: Option<&str>) -> Result<Option<JobLease>> {
        if plugin.is_some_and(|p| !slug(p)) {
            return Err(invalid("Invalid plugin"));
        }
        let discovery = self.controller().await?;
        let row=discovery.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT to_jsonb(j) AS job FROM webdock_auth.plugin_job_execution j WHERE status IN ('pending','running') AND (status='pending' OR lease_until<=clock_timestamp()) AND ($1::text IS NULL OR plugin_id=$1) AND (provenance #>> '{session_context,resourceAction,effect}' IS DISTINCT FROM 'delete' OR EXISTS(SELECT 1 FROM webdock_auth.plugin_invocation a WHERE a.id::text=j.provenance->>'id' AND a.session_context->>'resourceDeleteJob'=j.id)) ORDER BY available_at,id LIMIT 1",[plugin.into()])).await.map_err(db_error)?;
        discovery.commit().await.map_err(db_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let row: Value = row.try_get("", "job").map_err(db_error)?;
        if self.finish_unclaimed_job(&row).await? {
            return Ok(None);
        }
        let context = match self.job_authorization(&row).await {
            Ok(context) => context,
            Err(e) if e.code == ErrorCode::PermissionDenied => {
                self.cancel_invalid_job(&row, None).await?;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let locked=context.tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT to_jsonb(j) AS job, available_at<=clock_timestamp() AS due FROM webdock_auth.plugin_job_execution j WHERE id=$1 AND status IN ('pending','running') AND (status='pending' OR lease_until<=clock_timestamp()) FOR UPDATE SKIP LOCKED",[string(&row,"id")?.into()])).await.map_err(db_error)?;
        let Some(locked) = locked else {
            return Ok(None);
        };
        let fresh: Value = locked.try_get("", "job").map_err(db_error)?;
        if fresh["provenance"] != row["provenance"] {
            return denied();
        }
        if fresh["cancel_requested"] == true
            || integer(&fresh, "attempts")? >= integer(&fresh, "max_attempts")?
        {
            return Ok(None);
        }
        if !locked.try_get::<bool>("", "due").map_err(db_error)? {
            return Ok(None);
        }
        let token = uuid::Uuid::new_v4();
        let now = chrono::Utc::now().timestamp();
        let claims = InvocationClaims {
            v: 1,
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: string(&fresh, "plugin_id")?.into(),
            generation: integer(&fresh, "runtime_generation")? as u64,
            action: string(&fresh, "name")?.into(),
            project_id: string(&fresh, "project_id")?.into(),
            customer_id: string(&fresh, "customer_id")?.into(),
            iat: now,
            exp: now + 60,
            job: Some(JobIdentity {
                id: string(&fresh, "id")?.into(),
                attempt: (integer(&fresh, "attempts")? + 1) as u8,
            }),
        };
        if let Some(id) = fresh["invocation_id"].as_str() {
            Self::revoke_job_invocation(&context.tx, id).await?;
        }
        let source = &fresh["provenance"];
        context.tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_invocation(id,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,actor_id,session_id,job_execution_id,job_claim_token,operation_id,capabilities,created_at,expires_at,session_context,request_hash) VALUES($1::uuid,$2,$3,$4,$5,$6,'job',$7,'write',$8,$9,$10,$11::uuid,$10,$12,to_timestamp($13)-interval '60 seconds',to_timestamp($13),$14,$15)",[
            claims.id.clone().into(),claims.plugin_id.clone().into(),claims.customer_id.clone().into(),claims.project_id.clone().into(),(claims.generation as i64).into(),integer(&fresh,"grant_revision")?.into(),claims.action.clone().into(),string(source,"actor_id")?.into(),string(source,"session_id")?.into(),string(&fresh,"id")?.into(),token.to_string().into(),self.job_declaration(&context.metadata,&claims.action)?["capabilities"].clone().into(),claims.exp.into(),source["session_context"].clone().into(),digest(canonical_json(&fresh["payload"])?).into()])).await.map_err(db_error)?;
        context.tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_job_execution SET status='running',attempts=attempts+1,claim_token=$2::uuid,lease_until=clock_timestamp()+interval '90 seconds',invocation_id=$3::uuid,condition=NULL WHERE id=$1",[string(&fresh,"id")?.into(),token.to_string().into(),claims.id.clone().into()])).await.map_err(db_error)?;
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| unavailable())?);
        let signed = format!(
            "{body}.{}",
            URL_SAFE_NO_PAD.encode(self.key.sign(body.as_bytes()).to_bytes())
        );
        let lease = JobLease {
            id: string(&fresh, "id")?.into(),
            token,
            manifest: context.metadata.manifest.clone(),
            invocation: AdmittedInvocation {
                claims,
                token: signed,
                configuration: context.configuration.clone(),
                input: fresh["payload"].clone(),
                replay_result: None,
            },
        };
        context.commit(self).await?;
        Ok(Some(lease))
    }
    pub async fn complete_job(&self, lease: &JobLease, result: Result<Value>) -> Result<()> {
        let row = self.job_lookup(lease.id()).await?;
        if row["claim_token"] != lease.token.to_string() || row["status"] != "running" {
            return Ok(());
        }
        let context = match self.job_authorization(&row).await {
            Ok(context) => context,
            Err(e) if e.code == ErrorCode::PermissionDenied => {
                return self.cancel_invalid_job(&row, Some(lease.token)).await;
            }
            Err(e) => return Err(e),
        };
        let locked=context.tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.plugin_job_execution WHERE id=$1 AND claim_token=$2::uuid AND status='running' AND lease_until>clock_timestamp() AND invocation_id=$3::uuid FOR UPDATE",[lease.id.clone().into(),lease.token.to_string().into(),lease.invocation.claims.id.clone().into()])).await.map_err(db_error)?;
        if locked.is_none() {
            return Ok(());
        }
        let result = match result {
            Ok(v) => match canonical_json(&v) {
                Ok(encoded) if encoded.len() <= 32768 => self_service::record_result(
                    &context.tx,
                    &lease.invocation.claims.id,
                    &row["provenance"],
                    &v,
                    true,
                )
                .await
                .map(|()| {
                    if row["provenance"]["session_context"]["resourceAction"]["effect"] == "create"
                    {
                        json!({"resource":{"id":v["resource"]["id"],"name":v["resource"]["name"]}})
                    } else {
                        v
                    }
                }),
                _ => Err(invalid("Invalid job result")),
            },
            v => v,
        };
        let (status, condition, value) = match result {
            Ok(value) => ("succeeded", None, Some(value)),
            Err(e) if e.code == ErrorCode::PermissionDenied => {
                ("cancelled", Some("authority_revoked"), None)
            }
            Err(e)
                if [ErrorCode::Unavailable, ErrorCode::DeadlineExceeded].contains(&e.code)
                    && integer(&row, "attempts")? < integer(&row, "max_attempts")?
                    && row["provenance"]["session_context"]["resourceAction"]["effect"]
                        != "create" =>
            {
                (
                    "pending",
                    Some(if e.code == ErrorCode::Unavailable {
                        "unavailable"
                    } else {
                        "deadline_exceeded"
                    }),
                    None,
                )
            }
            Err(e) => (
                "failed",
                Some(
                    if integer(&row, "attempts")? >= integer(&row, "max_attempts")? {
                        "attempts_exhausted"
                    } else if e.code == ErrorCode::InvalidInput {
                        "invalid_result"
                    } else {
                        "attempt_failed"
                    },
                ),
                None,
            ),
        };
        let backoff = 1_i64 << (integer(&row, "attempts")? - 1).clamp(0, 4);
        let changed=context.tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_job_execution SET status=$3,condition=$4,result=$5,claim_token=NULL,lease_until=NULL,available_at=clock_timestamp()+$6*interval '1 second' WHERE id=$1 AND claim_token=$2::uuid AND status='running' AND lease_until>clock_timestamp() AND NOT cancel_requested",[lease.id.clone().into(),lease.token.to_string().into(),status.into(),condition.into(),value.into(),backoff.into()])).await.map_err(db_error)?.rows_affected();
        if changed != 1 {
            return Ok(());
        }
        Self::revoke_job_invocation(&context.tx, &lease.invocation.claims.id).await?;
        context.commit(self).await
    }
}

struct JobAuthorization {
    tx: DatabaseTransaction,
    session: DatabaseTransaction,
    auth: DatabaseTransaction,
    metadata: Metadata,
    configuration: Value,
    reference: AuthorityReference,
    subject: String,
    session_id: String,
}
impl JobAuthorization {
    async fn commit(self, host: &HostService) -> Result<()> {
        host.session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?
            .verify(
                &self.auth,
                &self.subject,
                &self.session_id,
                &self.reference,
                true,
            )
            .await?;
        self.tx.commit().await.map_err(db_error)?;
        self.session.commit().await.map_err(db_error)?;
        self.auth.commit().await.map_err(db_error)
    }
}
