//! Private Kubernetes admission and durable dispatch intent. A lease expiring never frees a slot.
use super::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KubernetesStoragePolicy {
    pub node: String,
    #[serde(rename = "poolID")]
    pub pool_id: String,
    pub volume_bytes: u64,
}
impl KubernetesStoragePolicy {
    pub fn validate(&self, cluster_id: &str) -> Result<()> {
        spitzli_agent_protocol::build_storage::BuildStorageTarget {
            cluster_id: cluster_id.into(),
            node: self.node.clone(),
            pool_id: self.pool_id.clone(),
        }
        .validate()
        .map_err(|_| error("invalid_build_storage_target"))?;
        if !(27 * 1024 * 1024 * 1024..=32 * 1024 * 1024 * 1024).contains(&self.volume_bytes)
            || !self.volume_bytes.is_multiple_of(1024 * 1024)
        {
            return Err(error("invalid_build_storage_budget"));
        }
        Ok(())
    }
}

fn fingerprint(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return Err(error("invalid_kubernetes_fingerprint"));
    }
    Ok(())
}

fn job_name(build_id: &str, generation: i32) -> String {
    format!(
        "wd-build-{}-{generation}",
        &hex::encode(Sha256::digest(build_id.as_bytes()))[..40]
    )
}

impl BuildService {
    pub(super) async fn kubernetes_admission_lock(tx: &DatabaseTransaction) -> Result<()> {
        execute(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended('git-build-admission',0))",
            vec![],
        )
        .await?;
        Ok(())
    }

    pub async fn configure_kubernetes_admission(
        &self,
        actor: &Principal,
        enabled: bool,
        global_slots: u32,
        tenant_slots: u32,
    ) -> Result<()> {
        if !(1..=256).contains(&global_slots) || !(1..=32).contains(&tenant_slots) {
            return Err(error("invalid_build_capacity"));
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        self.operator(&tx, actor).await?;
        Self::kubernetes_admission_lock(&tx).await?;
        execute(&tx, "UPDATE webdock_auth.git_build_admission SET enabled=$1,global_slots=$2,tenant_slots=$3 WHERE singleton", vec![enabled.into(), (global_slots as i32).into(), (tenant_slots as i32).into()]).await?;
        self.audit(
            &tx,
            &actor.user_id,
            "git.kubernetes.admission",
            "global",
            if enabled { "enabled" } else { "disabled" },
        )
        .await?;
        self.operator(&tx, actor).await?;
        tx.commit().await.map_err(db_error)
    }

    pub async fn trust_kubernetes_project(
        &self,
        actor: &Principal,
        customer_id: &str,
        project_id: &str,
        enabled: bool,
    ) -> Result<()> {
        crate::contracts::id(customer_id)?;
        crate::contracts::id(project_id)?;
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        self.operator(&tx, actor).await?;
        Self::kubernetes_admission_lock(&tx).await?;
        if rows(&tx, "SELECT to_jsonb(p) AS value FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1 AND p.customer_id=$2 AND p.status='active' AND c.status='active' FOR SHARE OF c", vec![project_id.into(),customer_id.into()]).await?.is_empty() {
            return Err(error("project_unavailable"));
        }
        if execute(&tx, "INSERT INTO webdock_auth.git_build_trusted_project(project_id,customer_id,enabled) VALUES($1,$2,$3) ON CONFLICT(project_id) DO UPDATE SET enabled=excluded.enabled,updated_at=now() WHERE webdock_auth.git_build_trusted_project.customer_id=excluded.customer_id", vec![project_id.into(),customer_id.into(),enabled.into()]).await?!=1 {
            return Err(error("project_scope_conflict"));
        }
        self.audit(
            &tx,
            &actor.user_id,
            "git.kubernetes.trust",
            project_id,
            if enabled { "enabled" } else { "disabled" },
        )
        .await?;
        self.operator(&tx, actor).await?;
        tx.commit().await.map_err(db_error)
    }

    /// Enroll a separate dispatcher identity using existing artifact-only worker credentials.
    /// The cluster starts disabled and must be explicitly activated by the operator.
    pub async fn enroll_kubernetes_cluster(
        &self,
        actor: &Principal,
        worker_id: &str,
        namespace: &str,
        country: &str,
        evidence: &str,
        capacity: u32,
    ) -> Result<Value> {
        crate::contracts::id(worker_id)?;
        if namespace.is_empty()
            || namespace.len() > 63
            || !namespace
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            || namespace.starts_with('-')
            || namespace.ends_with('-')
            || !(1..=32).contains(&capacity)
            || !(20..=4000).contains(&evidence.len())
            || ![
                "AT", "BE", "BG", "HR", "CY", "CZ", "DE", "DK", "EE", "ES", "FI", "FR", "GR", "HU",
                "IE", "IT", "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK",
            ]
            .contains(&country)
        {
            return Err(error("invalid_kubernetes_cluster"));
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        Self::module(&tx, false).await?;
        self.operator(&tx, actor).await?;
        Self::kubernetes_admission_lock(&tx).await?;
        let worker = rows(&tx, "SELECT to_jsonb(w) AS value FROM webdock_auth.git_worker w WHERE id=$1 AND enabled AND isolation='artifact-only' AND country=$2 FOR SHARE", vec![worker_id.into(),country.into()]).await?.pop().ok_or_else(||error("invalid_kubernetes_worker"))?;
        if worker["capacity"].as_u64().unwrap_or(0) < u64::from(capacity) {
            return Err(error("invalid_build_capacity"));
        }
        let id = self.id().await?;
        execute(&tx, "INSERT INTO webdock_auth.git_build_cluster(id,worker_id,worker_generation,namespace,country,evidence,capacity) VALUES($1,$2,$3,$4,$5,$6,$7)", vec![id.clone().into(),worker_id.into(),(worker["generation"].as_i64().ok_or_else(||error("invalid_worker"))? as i32).into(),namespace.into(),country.into(),evidence.into(),(capacity as i32).into()]).await?;
        self.audit(
            &tx,
            &actor.user_id,
            "git.kubernetes.cluster.enroll",
            &id,
            "disabled",
        )
        .await?;
        self.operator(&tx, actor).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(
            json!({"id":id,"workerID":worker_id,"namespace":namespace,"enabled":false,"publicReady":false}),
        )
    }

    pub async fn enable_kubernetes_cluster(
        &self,
        actor: &Principal,
        cluster_id: &str,
        enabled: bool,
    ) -> Result<()> {
        crate::contracts::id(cluster_id)?;
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        self.operator(&tx, actor).await?;
        Self::kubernetes_admission_lock(&tx).await?;
        if execute(
            &tx,
            "UPDATE webdock_auth.git_build_cluster SET enabled=$2 WHERE id=$1",
            vec![cluster_id.into(), enabled.into()],
        )
        .await?
            != 1
        {
            return Err(error("kubernetes_cluster_unavailable"));
        }
        self.audit(
            &tx,
            &actor.user_id,
            "git.kubernetes.cluster.enable",
            cluster_id,
            if enabled { "enabled" } else { "disabled" },
        )
        .await?;
        self.operator(&tx, actor).await?;
        tx.commit().await.map_err(db_error)
    }

    /// Rotate dispatcher credentials without losing cleanup authority over uncertain old Jobs.
    pub async fn rebind_kubernetes_cluster(
        &self,
        actor: &Principal,
        cluster_id: &str,
        worker_id: &str,
    ) -> Result<()> {
        crate::contracts::id(cluster_id)?;
        crate::contracts::id(worker_id)?;
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        self.operator(&tx, actor).await?;
        Self::kubernetes_admission_lock(&tx).await?;
        if execute(&tx,"UPDATE webdock_auth.git_build_cluster c SET worker_id=w.id,worker_generation=w.generation,enabled=false FROM webdock_auth.git_worker w WHERE c.id=$1 AND w.id=$2 AND w.enabled AND w.isolation='artifact-only' AND w.country=c.country AND w.capacity>=c.capacity",vec![cluster_id.into(),worker_id.into()]).await?!=1 {
            return Err(error("invalid_kubernetes_worker"));
        }
        self.audit(
            &tx,
            &actor.user_id,
            "git.kubernetes.cluster.rebind",
            cluster_id,
            "disabled",
        )
        .await?;
        self.operator(&tx, actor).await?;
        tx.commit().await.map_err(db_error)
    }

    /// Call inside the already-authorized source configuration transaction, before its upsert.
    pub(crate) async fn kubernetes_source_allowed(
        tx: &DatabaseTransaction,
        customer: &str,
        project: &str,
    ) -> Result<()> {
        if rows(tx,"SELECT to_jsonb(t) AS value FROM webdock_auth.git_build_trusted_project t JOIN webdock_auth.git_build_admission a ON a.singleton WHERE t.project_id=$1 AND t.customer_id=$2 AND t.enabled AND a.enabled",vec![project.into(),customer.into()]).await?.is_empty() {
            return Err(error("kubernetes_project_not_trusted"));
        }
        Ok(())
    }

    async fn kubernetes_cluster(
        tx: &DatabaseTransaction,
        worker: &WorkerIdentity,
        cluster: &str,
        enabled: bool,
    ) -> Result<Value> {
        let identity = Self::verify(tx, worker).await?;
        if identity["isolation"] != "artifact-only" {
            return Err(error("invalid_kubernetes_worker"));
        }
        rows(tx,"SELECT to_jsonb(c) AS value FROM webdock_auth.git_build_cluster c WHERE c.id=$1 AND c.worker_id=$2 AND c.worker_generation=$3 AND (NOT $4 OR c.enabled)",vec![cluster.into(),worker.id.clone().into(),worker.generation.into(),enabled.into()]).await?.pop().ok_or_else(||error("kubernetes_cluster_unavailable"))
    }

    /// Atomically reserves capacity and commits intent before a dispatcher may create a Job.
    pub async fn claim_kubernetes_build(
        &self,
        worker: &WorkerIdentity,
        cluster_id: &str,
        runtime_fingerprint: &str,
    ) -> Result<Option<Value>> {
        self.claim_kubernetes_build_with_storage(worker, cluster_id, runtime_fingerprint, false)
            .await
    }

    pub async fn claim_kubernetes_build_with_storage(
        &self,
        worker: &WorkerIdentity,
        cluster_id: &str,
        runtime_fingerprint: &str,
        fixed_storage: bool,
    ) -> Result<Option<Value>> {
        crate::contracts::id(cluster_id)?;
        fingerprint(runtime_fingerprint)?;
        let tx = self.worker_tx().await?;
        Self::kubernetes_admission_lock(&tx).await?;
        let cluster = Self::kubernetes_cluster(&tx, worker, cluster_id, true).await?;
        if !cluster["storage_policy"].is_null() {
            if !fixed_storage || cluster["storage_runtime_fingerprint"] != runtime_fingerprint {
                return Err(error("kubernetes_storage_profile_required"));
            }
        } else if fixed_storage {
            return Err(error("kubernetes_storage_unconfigured"));
        }
        // Fixed ordering prevents competing API instances from oversubscribing any boundary.
        let admission = rows(
            &tx,
            "SELECT to_jsonb(a) AS value FROM webdock_auth.git_build_admission a WHERE singleton",
            vec![],
        )
        .await?
        .pop()
        .ok_or_else(|| error("kubernetes_disabled"))?;
        if admission["enabled"] != true {
            return Err(error("kubernetes_disabled"));
        }
        execute(
            &tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("git-build-cluster:{cluster_id}").into()],
        )
        .await?;
        execute(
            &tx,
            "SELECT webdock_rls.cancel_obsolete_git_builds()",
            vec![],
        )
        .await?;
        let counts = rows(&tx,"SELECT jsonb_build_object('global',count(*),'cluster',count(*) FILTER(WHERE cluster_id=$1)) AS value FROM webdock_auth.git_build_attempt WHERE retired_at IS NULL",vec![cluster_id.into()]).await?.pop().ok_or_else(||error("invalid_build_capacity"))?;
        if counts["global"].as_i64().unwrap_or(i64::MAX)
            >= admission["global_slots"].as_i64().unwrap_or(0)
            || counts["cluster"].as_i64().unwrap_or(i64::MAX)
                >= cluster["capacity"].as_i64().unwrap_or(0)
        {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        }
        // Least-recently served tenant, then project; FIFO inside each project.
        // History survives dispatcher restarts and retries cannot jump a tenant's turn.
        let build = rows(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id JOIN webdock_auth.git_build_trusted_project t ON t.project_id=b.project_id AND t.customer_id=b.customer_id JOIN webdock_admin.projects p ON p.id=b.project_id AND p.customer_id=b.customer_id JOIN webdock_admin.customers u ON u.id=b.customer_id WHERE b.status='queued' AND b.next_eligible_at<=now() AND b.execution_backend='kubernetes' AND s.execution_backend='kubernetes' AND s.build_provider='isolated' AND s.recipe IN ('dockerfile','dockerfile-frontend') AND s.enabled AND t.enabled AND c.state='active' AND b.source_revision=s.revision AND b.environment_revision=s.environment_revision AND b.connection_generation=c.generation AND p.status='active' AND u.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted') AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_build_attempt a WHERE a.build_id=b.id AND a.retired_at IS NULL) AND (SELECT count(*) FROM webdock_auth.git_build_attempt a WHERE a.customer_id=b.customer_id AND a.retired_at IS NULL)<$1 AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_build_attempt a WHERE a.project_id=b.project_id AND a.retired_at IS NULL) ORDER BY (SELECT max(a.created_at) FROM webdock_auth.git_build_attempt a WHERE a.customer_id=b.customer_id) ASC NULLS FIRST,(SELECT max(a.created_at) FROM webdock_auth.git_build_attempt a WHERE a.project_id=b.project_id) ASC NULLS FIRST,b.created_at,b.id FOR UPDATE OF b SKIP LOCKED LIMIT 1",vec![(admission["tenant_slots"].as_i64().unwrap_or(0) as i32).into()]).await?.pop();
        let Some(build) = build else {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        };
        execute(
            &tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("git-build-tenant:{}", field(&build, "customer_id")?).into()],
        )
        .await?;
        let source = Self::source(
            &tx,
            field(&build, "source_id")?,
            field(&build, "project_id")?,
        )
        .await?;
        Self::active(&source)?;
        if source["revision"] != build["source_revision"]
            || source["environment_revision"] != build["environment_revision"]
            || source["connection_generation"] != build["connection_generation"]
            || source["execution_backend"] != "kubernetes"
        {
            return Err(error("source_revision_conflict"));
        }
        Self::kubernetes_source_allowed(
            &tx,
            field(&build, "customer_id")?,
            field(&build, "project_id")?,
        )
        .await?;
        let generation = build["generation"]
            .as_i64()
            .and_then(|g| g.checked_add(1))
            .filter(|g| *g < i64::from(i32::MAX))
            .ok_or_else(|| error("generation_exhausted"))? as i32;
        let id = field(&build, "id")?;
        let attempt = self.id().await?;
        let name = job_name(id, generation);
        let environment = self
            .hosting
            .git_build_environment(&tx, field(&source, "id")?)
            .await
            .map_err(hosting)?;
        let request = crate::execution::GuestRequest {
            recipe: field(&source, "recipe")?.into(),
            root_directory: field(&source, "root_directory")?.into(),
            build_environment: environment,
            limits: crate::execution::Limits {
                cpu: 2,
                memory_mi_b: 4096,
                disk_bytes: 10 * 1024 * 1024 * 1024,
                duration_seconds: 1500,
            },
        };
        request.validate()?;
        execute(&tx,"UPDATE webdock_auth.git_build SET status='running',phase='provisioning',worker_id=$2,worker_generation=$3,generation=$4,lease_until=now()+interval '35 minutes' WHERE id=$1",vec![id.into(),worker.id.clone().into(),worker.generation.into(),generation.into()]).await?;
        execute(&tx,"INSERT INTO webdock_auth.git_build_attempt(id,build_id,customer_id,project_id,generation,worker_id,worker_generation,cluster_id,namespace,job_name,runtime_fingerprint,execution_request) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",vec![attempt.clone().into(),id.into(),field(&build,"customer_id")?.into(),field(&build,"project_id")?.into(),generation.into(),worker.id.clone().into(),worker.generation.into(),cluster_id.into(),field(&cluster,"namespace")?.into(),name.clone().into(),runtime_fingerprint.into(),json!({"recipe":request.recipe,"rootDirectory":request.root_directory,"limits":request.limits,"containerBuild":source["container_build"]}).into()]).await?;
        let storage_request = self
            .assign_storage_request(&tx, &cluster, &build, &attempt, generation)
            .await?;
        let budget=rows(&tx,"SELECT kubernetes_limits AS value FROM webdock_auth.git_build_attempt WHERE build_id=$1 AND generation=$2",vec![id.into(),generation.into()]).await?.pop().ok_or_else(||error("kubernetes_attempt_lost"))?;
        Self::check(&tx, id, "in_progress", None).await?;
        Self::verify(&tx, worker).await?;
        let deadline = rows(
            &tx,
            "SELECT to_jsonb(lease_until) AS value FROM webdock_auth.git_build WHERE id=$1",
            vec![id.into()],
        )
        .await?
        .pop()
        .ok_or_else(|| error("build_lease_lost"))?;
        let result = json!({"buildID":id,"customerID":build["customer_id"],"projectID":build["project_id"],"sourceID":build["source_id"],"sourceSHA":build["source_sha"],"repositoryID":source["repository_id"],"installationID":source["installation_id"],"leaseUntil":date(&deadline),"generation":generation,"attempt":generation,"attemptID":attempt,"clusterID":cluster_id,"namespace":cluster["namespace"],"jobName":name,"runtimeFingerprint":runtime_fingerprint,"request":request,"containerBuild":source["container_build"],"kubernetesLimits":budget,"storageRequest":storage_request});
        tx.commit().await.map_err(db_error)?;
        Ok(Some(result))
    }

    /// Requeue only a classified infrastructure failure after exact resource retirement.
    /// Returns true for both a newly scheduled retry and replay of that same generation.
    /// A create/push timeout alone is never evidence of failure or retirement.
    pub async fn retry_kubernetes_infrastructure(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        failure_code: &str,
    ) -> Result<bool> {
        crate::contracts::id(build_id)?;
        if !matches!(
            failure_code,
            "KUBERNETES_API_UNAVAILABLE" | "KUBERNETES_NODE_UNAVAILABLE"
        ) {
            return Err(error("kubernetes_failure_not_retryable"));
        }
        let tx = self.worker_tx().await?;
        Self::kubernetes_admission_lock(&tx).await?;
        Self::verify(&tx, worker).await?;
        let result = self
            .retry_kubernetes_infrastructure_in_tx(&tx, worker, build_id, generation, failure_code)
            .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }

    async fn retry_kubernetes_infrastructure_in_tx(
        &self,
        tx: &DatabaseTransaction,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        failure_code: &str,
    ) -> Result<bool> {
        let build = rows(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE b.id=$1 AND b.execution_backend='kubernetes' FOR UPDATE",vec![build_id.into()]).await?.pop().ok_or_else(||error("build_unavailable"))?;
        let attempt = rows(tx,"SELECT to_jsonb(a) AS value FROM webdock_auth.git_build_attempt a JOIN webdock_auth.git_build_cluster c ON c.id=a.cluster_id WHERE a.build_id=$1 AND a.generation=$2 AND a.worker_id=$3 AND a.worker_generation=$4 AND c.worker_id=$3 AND c.worker_generation=$4 AND c.enabled",vec![build_id.into(),generation.into(),worker.id.clone().into(),worker.generation.into()]).await?.pop();
        let Some(attempt) = attempt else {
            return Ok(false);
        };
        // Replays neither postpone the durable deadline nor spend another retry.
        if build["retry_generation"].as_i64() == Some(i64::from(generation)) {
            let accepted = build["last_infrastructure_failure"] == failure_code;
            return Ok(accepted);
        }
        if build["generation"].as_i64() != Some(i64::from(generation))
            || build["status"] != "failed"
            || build["retry_count"].as_i64().unwrap_or(2) >= 2
            || !matches!(
                build["failure_code"].as_str(),
                Some("KUBERNETES_JOB_RETIRED")
            ) && build["failure_code"] != failure_code
            || attempt["retired_at"].is_null()
            || !attempt["storage_request"].is_null() && attempt["storage_retired_at"].is_null()
        {
            return Ok(false);
        }
        let eligible = rows(tx,"SELECT to_jsonb(b.id) AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id JOIN webdock_auth.git_build_trusted_project t ON t.project_id=b.project_id AND t.customer_id=b.customer_id JOIN webdock_admin.projects p ON p.id=b.project_id AND p.customer_id=b.customer_id JOIN webdock_admin.customers u ON u.id=b.customer_id JOIN webdock_auth.git_build_admission a ON a.singleton WHERE b.id=$1 AND a.enabled AND t.enabled AND s.enabled AND s.execution_backend='kubernetes' AND s.build_provider='isolated' AND c.state='active' AND b.source_revision=s.revision AND b.environment_revision=s.environment_revision AND b.connection_generation=c.generation AND p.status='active' AND u.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted') AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_artifact f WHERE f.build_id=b.id)",vec![build_id.into()]).await?;
        if eligible.is_empty() {
            return Ok(false);
        }
        execute(tx,"UPDATE webdock_auth.git_build SET status='queued',phase='queued',retry_count=retry_count+1,retry_generation=generation,next_eligible_at=now()+CASE WHEN retry_count=0 THEN interval '30 seconds' ELSE interval '120 seconds' END,last_infrastructure_failure=$2,lease_until=NULL,finished_at=NULL,completion_hash=NULL,failure_code=NULL WHERE id=$1",vec![build_id.into(),failure_code.into()]).await?;
        Self::check(tx, build_id, "queued", None).await?;
        Self::verify(tx, worker).await?;
        Ok(true)
    }

    /// Additional fence used by the common build lease for Kubernetes work.
    /// Dispatch preparation uses `bound=false`; source/artifact callbacks require observed identity.
    pub(crate) async fn guard_kubernetes_lease(
        tx: &DatabaseTransaction,
        worker: &WorkerIdentity,
        build: &Value,
        bound: bool,
    ) -> Result<()> {
        if build["execution_backend"] != "kubernetes" {
            return Ok(());
        }
        let attempt=rows(tx,"SELECT to_jsonb(a) AS value FROM webdock_auth.git_build_attempt a JOIN webdock_auth.git_build_cluster c ON c.id=a.cluster_id JOIN webdock_auth.git_build_trusted_project t ON t.project_id=a.project_id AND t.customer_id=a.customer_id JOIN webdock_auth.git_build_admission p ON p.singleton WHERE a.build_id=$1 AND a.generation=$2 AND a.worker_id=$3 AND a.worker_generation=$4 AND c.worker_id=$3 AND c.worker_generation=$4 AND c.enabled AND t.enabled AND p.enabled AND a.retired_at IS NULL AND NOT a.cancel_requested AND EXISTS(SELECT 1 FROM webdock_admin.projects j JOIN webdock_admin.customers u ON u.id=j.customer_id WHERE j.id=a.project_id AND j.customer_id=a.customer_id AND j.status='active' AND u.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=j.id AND d.status<>'deleted')) AND (NOT $5 OR (a.spec_fingerprint IS NOT NULL AND a.job_uid IS NOT NULL)) FOR SHARE OF a",vec![field(build,"id")?.into(),(build["generation"].as_i64().ok_or_else(||error("invalid_build"))? as i32).into(),worker.id.clone().into(),worker.generation.into(),bound.into()]).await?;
        if attempt.is_empty() {
            return Err(error("kubernetes_attempt_lost"));
        }
        Ok(())
    }

    pub async fn record_kubernetes_job_intent(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        spec_fingerprint: &str,
    ) -> Result<()> {
        fingerprint(spec_fingerprint)?;
        let tx = self.worker_tx().await?;
        let build = Self::kubernetes_dispatch_lease(&tx, worker, build_id, generation).await?;
        Self::guard_kubernetes_lease(&tx, worker, &build, false).await?;
        if execute(&tx,"UPDATE webdock_auth.git_build_attempt SET spec_fingerprint=$5 WHERE build_id=$1 AND generation=$2 AND worker_id=$3 AND worker_generation=$4 AND retired_at IS NULL AND NOT cancel_requested AND (spec_fingerprint IS NULL OR spec_fingerprint=$5)",vec![build_id.into(),generation.into(),worker.id.clone().into(),worker.generation.into(),spec_fingerprint.into()]).await?!=1 {return Err(error("kubernetes_fingerprint_conflict"));}
        tx.commit().await.map_err(db_error)
    }

    pub async fn record_kubernetes_job(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        uid: &str,
    ) -> Result<()> {
        if uid.is_empty()
            || uid.len() > 128
            || !uid
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"_-".contains(&v))
        {
            return Err(error("invalid_kubernetes_uid"));
        }
        let tx = self.worker_tx().await?;
        let build = Self::kubernetes_dispatch_lease(&tx, worker, build_id, generation).await?;
        Self::guard_kubernetes_lease(&tx, worker, &build, false).await?;
        if execute(&tx,"UPDATE webdock_auth.git_build_attempt SET job_uid=$5 WHERE build_id=$1 AND generation=$2 AND worker_id=$3 AND worker_generation=$4 AND retired_at IS NULL AND NOT cancel_requested AND spec_fingerprint IS NOT NULL AND (job_uid IS NULL OR job_uid=$5)",vec![build_id.into(),generation.into(),worker.id.clone().into(),worker.generation.into(),uid.into()]).await?!=1 {return Err(error("kubernetes_uid_conflict"));}
        tx.commit().await.map_err(db_error)
    }

    // Separate from common lease: an observed UID cannot exist before creating the Job.
    async fn kubernetes_dispatch_lease(
        tx: &DatabaseTransaction,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
    ) -> Result<Value> {
        Self::verify(tx, worker).await?;
        rows(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE b.id=$1 AND b.worker_id=$2 AND b.worker_generation=$3 AND b.generation=$4 AND b.status='running' AND b.execution_backend='kubernetes' AND b.lease_until>now() AND s.enabled AND s.execution_backend='kubernetes' AND s.revision=b.source_revision AND s.environment_revision=b.environment_revision AND c.state='active' AND c.generation=b.connection_generation FOR UPDATE OF b",vec![build_id.into(),worker.id.clone().into(),worker.generation.into(),generation.into()]).await?.pop().ok_or_else(||error("build_lease_lost"))
    }

    /// Includes cancelled builds and expired leases so dispatchers can reconcile before admitting more.
    pub async fn list_kubernetes_attempts(
        &self,
        worker: &WorkerIdentity,
        cluster_id: &str,
    ) -> Result<Vec<Value>> {
        let tx = self.worker_tx().await?;
        Self::kubernetes_cluster(&tx, worker, cluster_id, false).await?;
        let attempts=rows(&tx,"SELECT to_jsonb(a)||jsonb_build_object('kubernetesLimits',a.kubernetes_limits,'sourceSHA',b.source_sha,'request',a.execution_request-'containerBuild','containerBuild',a.execution_request->'containerBuild','buildStatus',b.status,'leaseUntil',b.lease_until,'cancelRequested',a.cancel_requested OR a.worker_id<>c.worker_id OR a.worker_generation<>c.worker_generation OR b.status<>'running' OR b.generation<>a.generation OR b.lease_until IS NULL OR b.lease_until<=now() OR NOT c.enabled OR NOT p.enabled OR NOT coalesce(t.enabled,false) OR NOT s.enabled OR s.execution_backend<>'kubernetes' OR b.source_revision<>s.revision OR b.environment_revision<>s.environment_revision OR q.state<>'active' OR b.connection_generation<>q.generation OR NOT EXISTS(SELECT 1 FROM webdock_admin.projects j JOIN webdock_admin.customers u ON u.id=j.customer_id WHERE j.id=a.project_id AND j.customer_id=a.customer_id AND j.status='active' AND u.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=j.id AND d.status<>'deleted'))) AS value FROM webdock_auth.git_build_attempt a JOIN webdock_auth.git_build b ON b.id=a.build_id JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection q ON q.id=s.connection_id JOIN webdock_auth.git_build_cluster c ON c.id=a.cluster_id JOIN webdock_auth.git_build_admission p ON p.singleton LEFT JOIN webdock_auth.git_build_trusted_project t ON t.project_id=a.project_id AND t.customer_id=a.customer_id WHERE a.cluster_id=$1 AND a.retired_at IS NULL ORDER BY a.created_at,a.id",vec![cluster_id.into()]).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(attempts)
    }

    /// Private execution handoff only; reconciliation metadata and durable attempt snapshots contain no secrets.
    pub async fn kubernetes_build_environment(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        crate::contracts::id(build_id)?;
        if generation <= 0 {
            return Err(error("invalid_generation"));
        }
        let tx = self.worker_tx().await?;
        let current = Self::lease(&tx, worker, build_id, generation).await?;
        if current["execution_backend"] != "kubernetes" {
            return Err(error("invalid_execution_backend"));
        }
        let environment = self
            .hosting
            .git_build_environment(&tx, field(&current, "source_id")?)
            .await
            .map_err(hosting)?;
        Self::lease(&tx, worker, build_id, generation).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(environment)
    }

    /// Caller must first prove the exact Job and all its Pods absent using UID preconditions.
    /// `uid=None` is allowed only when no UID was ever recorded (uncertain create reconciliation).
    pub async fn retire_kubernetes_attempt(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        uid: Option<&str>,
    ) -> Result<()> {
        self.retire_kubernetes_attempt_with_retry(worker, build_id, generation, uid, None)
            .await
            .map(|_| ())
    }

    /// Retirement and the retry decision commit together, so observers never mistake an automatic retry for terminal failure.
    pub async fn retire_kubernetes_attempt_with_retry(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        uid: Option<&str>,
        failure_code: Option<&str>,
    ) -> Result<bool> {
        if failure_code.is_some_and(|code| {
            !matches!(
                code,
                "KUBERNETES_API_UNAVAILABLE" | "KUBERNETES_NODE_UNAVAILABLE"
            )
        }) {
            return Err(error("kubernetes_failure_not_retryable"));
        }
        let tx = self.worker_tx().await?;
        Self::kubernetes_admission_lock(&tx).await?;
        Self::verify(&tx, worker).await?;
        // Build transitions cancel attempts in this order too; avoid a reverse-lock deadlock.
        rows(
            &tx,
            "SELECT to_jsonb(b.id) AS value FROM webdock_auth.git_build b WHERE id=$1 FOR UPDATE",
            vec![build_id.into()],
        )
        .await?;
        let attempt=rows(&tx,"SELECT to_jsonb(a) AS value FROM webdock_auth.git_build_attempt a JOIN webdock_auth.git_build_cluster c ON c.id=a.cluster_id WHERE a.build_id=$1 AND a.generation=$2 AND c.worker_id=$3 AND c.worker_generation=$4 FOR UPDATE OF a",vec![build_id.into(),generation.into(),worker.id.clone().into(),worker.generation.into()]).await?.pop().ok_or_else(||error("kubernetes_attempt_lost"))?;
        if attempt["job_uid"].as_str() != uid {
            return Err(error("kubernetes_uid_conflict"));
        }
        if !attempt["storage_request"].is_null() && attempt["storage_retired_at"].is_null() {
            return Err(error("kubernetes_volume_not_retired"));
        }
        if attempt["retired_at"].is_null() {
            execute(&tx,"UPDATE webdock_auth.git_build_attempt SET retired_at=now(),cancel_requested=true WHERE id=$1",vec![field(&attempt,"id")?.into()]).await?;
            execute(&tx,"UPDATE webdock_auth.git_build SET status='failed',phase='failed',lease_until=NULL,finished_at=now(),failure_code=$3 WHERE id=$1 AND generation=$2 AND status='running'",vec![build_id.into(),generation.into(),failure_code.unwrap_or("KUBERNETES_JOB_RETIRED").into()]).await?;
        }
        let retry = if let Some(code) = failure_code {
            self.retry_kubernetes_infrastructure_in_tx(&tx, worker, build_id, generation, code)
                .await?
        } else {
            false
        };
        Self::verify(&tx, worker).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(retry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn job_identity_matches_executor_and_is_generation_bound() {
        let name = job_name("123456789012345678", 1);
        assert_eq!(
            name,
            format!(
                "wd-build-{}-1",
                &hex::encode(Sha256::digest(b"123456789012345678"))[..40]
            )
        );
        assert_ne!(name, job_name("123456789012345678", 2));
        assert_ne!(name, job_name("123456789012345679", 1));
        assert!(job_name("123456789012345678", i32::MAX - 1).len() <= 63);
    }
    #[test]
    fn fingerprints_are_bounded_canonical_hashes() {
        assert!(fingerprint(&"a".repeat(64)).is_ok());
        for bad in [
            "a".repeat(63),
            "A".repeat(64),
            "g".repeat(64),
            "../".repeat(22),
        ] {
            assert!(fingerprint(&bad).is_err());
        }
    }
}
