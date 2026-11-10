use crate::{Result, artifact::StoredArtifact, contracts::BuildCommand, error};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait, Value as SqlValue,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_hosting::HostingService;
use spitzli_identity::{Principal, RlsSigner};
use spitzli_persistence::IdAllocator;
use std::sync::Arc;
use subtle::ConstantTimeEq;
#[derive(Clone)]
pub struct RepositoryRef<'a> {
    pub(crate) authority: RepositoryContext<'a>,
    pub customer_id: String,
    pub project_id: String,
    pub source_id: String,
    pub source_revision: i32,
    pub connection_generation: i32,
    pub plugin_id: Option<String>,
    pub installation_id: String,
    pub repository_id: String,
    pub metadata: Value,
}
#[derive(Clone)]
pub(crate) enum RepositoryContext<'a> {
    SourceTrigger {
        tx: &'a DatabaseTransaction,
    },
    PluginJob {
        proof: Box<spitzli_plugin_host::ExportJob>,
    },
    Session {
        principal: &'a Principal,
        tx: &'a DatabaseTransaction,
        authority: Option<&'a spitzli_browser_session::StoredAuthority>,
    },
    Build {
        worker: &'a WorkerIdentity,
        id: String,
        generation: i32,
    },
    Release {
        worker: &'a WorkerIdentity,
        id: String,
        generation: i32,
    },
}
pub struct VerifiedCommit {
    sha: String,
}
impl VerifiedCommit {
    pub fn new(sha: String) -> Result<Self> {
        crate::contracts::sha(&sha)?;
        Ok(Self { sha })
    }
}
#[async_trait]
pub trait RepositoryProvider: Send + Sync {
    /// Verify the live installation/repository binding and resolve an immutable commit.
    async fn resolve(
        &self,
        repository: &RepositoryRef<'_>,
        reference: &str,
    ) -> Result<VerifiedCommit>;
    /// Return a normalized, bounded source archive for exactly this immutable commit.
    async fn archive(&self, repository: &RepositoryRef<'_>, commit: &str) -> Result<Vec<u8>>;
}
struct Unavailable;
#[async_trait]
impl RepositoryProvider for Unavailable {
    async fn resolve(&self, _: &RepositoryRef<'_>, _: &str) -> Result<VerifiedCommit> {
        Err(error("repository_provider_unavailable"))
    }
    async fn archive(&self, _: &RepositoryRef<'_>, _: &str) -> Result<Vec<u8>> {
        Err(error("repository_provider_unavailable"))
    }
}
#[derive(Clone)]
pub struct WorkerIdentity {
    id: String,
    generation: i32,
    hash: String,
}
impl WorkerIdentity {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn generation(&self) -> i32 {
        self.generation
    }
}
pub struct BuildService {
    db: DatabaseConnection,
    worker: DatabaseConnection,
    signer: RlsSigner,
    secret: String,
    ids: Arc<IdAllocator>,
    hosting: Arc<HostingService>,
    provider: Arc<dyn RepositoryProvider>,
    repository_configured: bool,
    registry: String,
    authority: Option<Arc<session::SessionVerifier>>,
    github_webhooks: Option<(
        github_webhook::GithubWebhookVerifier,
        Arc<repository::RepositoryAdmission>,
    )>,
}
fn db_error(_: sea_orm::DbErr) -> crate::Error {
    error("build_database_unavailable")
}
async fn rows(db: &impl ConnectionTrait, sql: &str, values: Vec<SqlValue>) -> Result<Vec<Value>> {
    db.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map_err(db_error)?
    .into_iter()
    .map(|r| r.try_get("", "value").map_err(db_error))
    .collect()
}
async fn execute(db: &impl ConnectionTrait, sql: &str, values: Vec<SqlValue>) -> Result<u64> {
    Ok(db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(db_error)?
        .rows_affected())
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| error("invalid_stored_build"))
}
fn hosting(e: spitzli_hosting::HostingError) -> crate::Error {
    let _ = e;
    error("hosting_publication_unavailable")
}
impl BuildService {
    pub async fn new(
        db: DatabaseConnection,
        worker: DatabaseConnection,
        secret: String,
        ids: Arc<IdAllocator>,
        hosting: Arc<HostingService>,
        registry: String,
    ) -> Result<Self> {
        for (connection, role) in [
            (&db, "webdock_auth_runtime"),
            (&worker, "webdock_worker_runtime"),
        ] {
            spitzli_persistence::connection::verify_runtime_role(connection, role)
                .await
                .map_err(|_| error("invalid_database_role"))?;
        }
        if registry.is_empty()
            || registry.len() > 253
            || registry.contains('/')
            || registry.contains('@')
            || !registry
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
        {
            return Err(error("invalid_registry"));
        }
        let signer = RlsSigner::new(secret.clone(), "webdock_auth_runtime")
            .map_err(|_| error("invalid_signer"))?;
        Ok(Self {
            db,
            worker,
            signer,
            secret,
            ids,
            hosting,
            provider: Arc::new(Unavailable),
            repository_configured: false,
            registry,
            authority: None,
            github_webhooks: None,
        })
    }
    pub fn with_repository_provider(mut self, provider: Arc<dyn RepositoryProvider>) -> Self {
        self.provider = provider;
        self.repository_configured = true;
        self
    }
    pub fn with_github_webhooks(
        mut self,
        verifier: github_webhook::GithubWebhookVerifier,
        admission: Arc<repository::RepositoryAdmission>,
    ) -> Self {
        self.github_webhooks = Some((verifier, admission));
        self
    }
    pub async fn receive_github_webhook(
        &self,
        raw: &[u8],
        signature: &str,
        delivery: &str,
        event: &str,
        allow_new: bool,
    ) -> Result<Value> {
        let (verifier, admission) = self
            .github_webhooks
            .as_ref()
            .ok_or_else(|| error("github_webhook_unavailable"))?;
        let event = verifier.verify(raw, signature, delivery, event)?;
        self.ingest_github_webhook_with_admission(&event, admission, allow_new)
            .await
    }
    async fn id(&self) -> Result<String> {
        Ok(self
            .ids
            .allocate()
            .await
            .map_err(|_| error("id_unavailable"))?
            .to_string())
    }
    async fn scope(&self, tx: &DatabaseTransaction, purpose: &str) -> Result<()> {
        let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":purpose,"authProvider":"better-auth"}}).to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes())
            .map_err(|_| error("invalid_signer"))?;
        mac.update(raw.as_bytes());
        execute(tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await?;
        Ok(())
    }
    async fn worker_tx(&self) -> Result<DatabaseTransaction> {
        let tx = self.worker.begin().await.map_err(db_error)?;
        self.scope(&tx, "plugin-admission").await?;
        Self::module(&tx, true).await?;
        self.scope(&tx, "git-worker").await?;
        execute(&tx,"SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",vec![]).await?;
        Ok(tx)
    }
    pub(crate) async fn module(tx: &DatabaseTransaction, continuation: bool) -> Result<()> {
        execute(
            tx,
            "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))",
            vec![],
        )
        .await?;
        let state=rows(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.plugin_installation p WHERE id='hosting'",vec![]).await?.pop().ok_or_else(||error("hosting_unavailable"))?;
        if state["state"] == "disabled" || (!continuation && state["state"] != "enabled") {
            return Err(error("hosting_unavailable"));
        }
        Ok(())
    }
    async fn authorize(
        tx: &DatabaseTransaction,
        p: &Principal,
        project: &str,
        write: bool,
        manage: bool,
    ) -> Result<Value> {
        let r=rows(tx,"SELECT jsonb_build_object('actor',webdock_rls.actor(),'project',to_jsonb(p),'hosting',to_jsonb(h),'allowed',webdock_rls.customer_allowed(p.customer_id,false),'write',webdock_rls.customer_permission(p.customer_id,$2)) AS value FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id JOIN webdock_auth.hosting_project h ON h.project_id=p.id WHERE p.id=$1 AND p.status='active' AND c.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted') FOR SHARE OF c",vec![project.into(),if manage{"org:projects:manage"}else{"org:deployments:create"}.into()]).await?.pop().ok_or_else(||error("project_unavailable"))?;
        if r["actor"]["subject"] != p.user_id
            || r["actor"]["sessionID"] != p.session_id
            || r["allowed"] != true
            || write
                && (p.in_read_only_preview()
                    || r["write"] != true
                    || (r["actor"]["operator"] != true && r["hosting"]["mode"] != "selfservice"))
        {
            return Err(error("permission_denied"));
        }
        Ok(r)
    }
    pub(crate) async fn source(tx: &DatabaseTransaction, id: &str, project: &str) -> Result<Value> {
        rows(tx,"SELECT to_jsonb(s)||jsonb_build_object('connection_generation',c.generation,'connection_state',c.state,'installation_id',c.installation_id,'repository_plugin_id',c.repository_plugin_id,'metadata',r.metadata) AS value FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id JOIN webdock_auth.git_repository r ON r.connection_id=s.connection_id AND r.repository_id=s.repository_id WHERE s.id=$1 AND s.project_id=$2 FOR UPDATE OF s",vec![id.into(),project.into()]).await?.pop().ok_or_else(||error("source_unavailable"))
    }
    fn active(source: &Value) -> Result<()> {
        if source["enabled"] != true
            || source["connection_state"] != "active"
            || !source["recipe"]
                .as_str()
                .is_some_and(|r| ["dockerfile", crate::frontend::RECIPE].contains(&r))
            || source["build_provider"] != "isolated"
        {
            return Err(error("source_unavailable"));
        }
        Ok(())
    }
    pub(crate) fn repository<'a>(
        source: &Value,
        authority: RepositoryContext<'a>,
    ) -> Result<RepositoryRef<'a>> {
        Ok(RepositoryRef {
            authority,
            customer_id: field(source, "customer_id")?.into(),
            project_id: field(source, "project_id")?.into(),
            source_id: field(source, "id")?.into(),
            source_revision: source["revision"]
                .as_i64()
                .ok_or_else(|| error("invalid_source"))? as i32,
            connection_generation: source["connection_generation"]
                .as_i64()
                .ok_or_else(|| error("invalid_source"))? as i32,
            plugin_id: source["repository_plugin_id"].as_str().map(str::to_owned),
            installation_id: field(source, "installation_id")?.into(),
            repository_id: field(source, "repository_id")?.into(),
            metadata: source["metadata"].clone(),
        })
    }
    async fn audit(
        &self,
        tx: &DatabaseTransaction,
        subject: &str,
        action: &str,
        target: &str,
        outcome: &str,
    ) -> Result<()> {
        execute(tx,"INSERT INTO webdock_auth.hosting_audit(id,subject,action,target_id,outcome) VALUES($1,$2,$3,$4,$5)",vec![self.id().await?.into(),subject.into(),action.into(),target.into(),outcome.into()]).await?;
        Ok(())
    }
    async fn check(
        tx: &DatabaseTransaction,
        build: &str,
        status: &str,
        conclusion: Option<&str>,
    ) -> Result<()> {
        execute(tx,"INSERT INTO webdock_auth.git_check_outbox(build_id,installation_id,repository_id,source_sha,status,conclusion) SELECT b.id,c.installation_id,s.repository_id,b.source_sha,$2,$3 FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE b.id=$1 ON CONFLICT(build_id) DO UPDATE SET status=excluded.status,conclusion=excluded.conclusion,version=git_check_outbox.version+1,retry_at=now()",vec![build.into(),status.into(),conclusion.into()]).await?;
        Ok(())
    }
    pub async fn execute_with_ceiling(
        &self,
        principal: &Principal,
        input: Value,
        ceiling: crate::contracts::BuildCeiling,
    ) -> Result<Value> {
        ceiling.require(
            input["action"]
                .as_str()
                .ok_or_else(|| error("invalid_command"))?,
        )?;
        self.execute(principal, input).await
    }
    pub async fn execute_with_authority(
        &self,
        principal: &Principal,
        authority: &spitzli_browser_session::StoredAuthority,
        input: Value,
        ceiling: crate::BuildCeiling,
    ) -> Result<Value> {
        ceiling.require(
            input["action"]
                .as_str()
                .ok_or_else(|| error("invalid_command"))?,
        )?;
        self.execute_inner(principal, input, Some(authority)).await
    }
    pub async fn execute(&self, principal: &Principal, input: Value) -> Result<Value> {
        self.execute_inner(principal, input, None).await
    }
    async fn execute_inner(
        &self,
        principal: &Principal,
        input: Value,
        authority: Option<&spitzli_browser_session::StoredAuthority>,
    ) -> Result<Value> {
        if matches!(
            input["action"].as_str(),
            Some("git.connections.list" | "git.connections.disconnect" | "git.repositories.list")
        ) {
            return self.execute_connections(principal, input, authority).await;
        }
        let command = BuildCommand::parse(input)?;
        let project = command.project().to_owned();
        let manage = matches!(command, BuildCommand::SourceConfigure { .. });
        let write = command.write();
        let authority_guard = self.begin_authority(principal, authority, write).await?;
        if let BuildCommand::SourceWebhook {
            source_id,
            revision,
            enabled,
            ..
        } = &command
        {
            let value = self
                .configure_github_build_trigger(principal, &project, source_id, *revision, *enabled)
                .await?;
            session::verify(&authority_guard).await?;
            session::commit(authority_guard).await?;
            return Ok(value);
        }
        let tx = self
            .signer
            .begin(&self.db, principal)
            .await
            .map_err(|_| error("authentication_required"))?;
        Self::module(&tx, !write).await?;
        let access = Self::authorize(&tx, principal, &project, write, manage).await?;
        let customer = field(&access["project"], "customer_id")?.to_owned();
        let result = match command {
            BuildCommand::SourceWebhook { .. } => return Err(error("invalid_command")),
            BuildCommand::SourceGet { target_id, .. } => {
                let source = match Self::select_source(&tx, &project, target_id.as_deref()).await {
                    Ok(id) => {
                        let source = Self::source(&tx, &id, &project).await?;
                        self.source_view(&tx, &source).await?
                    }
                    Err(e) if e.code == "source_unavailable" => Value::Null,
                    Err(e) => return Err(e),
                };
                let targets=rows(&tx,"SELECT jsonb_build_object('id',id,'name',name) AS value FROM webdock_auth.hosting_app WHERE project_id=$1 AND status<>'deleted' ORDER BY name,id",vec![project.clone().into()]).await?;
                let write_allowed =
                    !principal.in_read_only_preview() && authority.is_none_or(|a| !a.read_only());
                let can_build = write_allowed
                    && Self::authorize(&tx, principal, &project, true, false)
                        .await
                        .is_ok();
                let can_configure = write_allowed
                    && Self::authorize(&tx, principal, &project, true, true)
                        .await
                        .is_ok();
                json!({"source":source,"targets":targets,"canBuild":can_build,"canConfigure":can_configure,"canApprove":can_build,"canBindPlatform":false,"blockers":if self.repository_configured{vec![]}else{vec!["Repository provider adapter is unavailable."]}})
            }
            BuildCommand::SourcesList { .. } => {
                let records=rows(&tx,"SELECT to_jsonb(s) AS value FROM webdock_auth.git_source s WHERE project_id=$1 ORDER BY created_at,id",vec![project.clone().into()]).await?;
                let mut docs = Vec::new();
                for record in records {
                    docs.push(self.source_view(&tx, &record).await?);
                }
                json!({"docs":docs})
            }
            BuildCommand::SourceConfigure {
                target_id,
                connection_id,
                repository_id,
                branch,
                root_directory,
                recipe,
                revision,
                enabled,
                auto_publish,
                health_path,
                build_provider: _,
                execution_backend,
                container_build,
                workflow_path,
                webhook_trigger,
                artifact_prefix,
                build_environment,
                ..
            } => {
                execute(
                    &tx,
                    "SELECT webdock_admin.lock_hosting_project($1)",
                    vec![project.clone().into()],
                )
                .await?;
                let repo=rows(&tx,"SELECT to_jsonb(r) AS value FROM webdock_auth.git_repository r JOIN webdock_auth.git_connection c ON c.id=r.connection_id WHERE r.connection_id=$1 AND r.repository_id=$2 AND r.customer_id=$3 AND c.state='active'",vec![connection_id.clone().into(),repository_id.clone().into(),customer.clone().into()]).await?;
                if repo.is_empty() {
                    return Err(error("repository_unavailable"));
                }
                let app=rows(&tx,"SELECT to_jsonb(a) AS value FROM webdock_auth.hosting_app a WHERE id=$1 AND project_id=$2 AND status<>'deleted' FOR SHARE",vec![target_id.clone().into(),project.clone().into()]).await?;
                if app.is_empty()
                    || !["k3s", "docker"]
                        .contains(&access["hosting"]["provider"].as_str().unwrap_or(""))
                {
                    return Err(error("target_unavailable"));
                }
                let prior=rows(&tx,"SELECT to_jsonb(s) AS value FROM webdock_auth.git_source s WHERE project_id=$1 AND target_id=$2 FOR UPDATE",vec![project.clone().into(),target_id.clone().into()]).await?.pop();
                if prior
                    .as_ref()
                    .map(|v| v["revision"].as_i64().unwrap_or(-1))
                    .unwrap_or(0)
                    != i64::from(revision)
                {
                    return Err(error("source_revision_conflict"));
                }
                if execution_backend == "kubernetes" {
                    Self::kubernetes_source_allowed(&tx, &customer, &project).await?;
                }
                let container_build = container_build
                    .map(|c| serde_json::to_value(c).unwrap())
                    .or_else(|| prior.as_ref().map(|v| v["container_build"].clone()))
                    .unwrap_or_else(|| {
                        serde_json::to_value(crate::contracts::ContainerSettings::default())
                            .unwrap()
                    });
                let webhook_trigger = webhook_trigger
                    .map(|mode| mode.as_str().to_owned())
                    .unwrap_or_else(|| {
                        prior
                            .as_ref()
                            .and_then(|s| s["webhook_trigger"].as_str())
                            .unwrap_or("push")
                            .to_owned()
                    });
                let workflow_path = workflow_path
                    .or_else(|| {
                        prior
                            .as_ref()
                            .and_then(|s| s["workflow_path"].as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(crate::contracts::workflow);
                if webhook_trigger == "workflow_run" && execution_backend != "kubernetes" {
                    return Err(error("invalid_webhook_trigger"));
                }
                let id = if let Some(row) = prior {
                    field(&row, "id")?.to_owned()
                } else {
                    self.id().await?
                };
                execute(&tx,"INSERT INTO webdock_auth.git_source(id,customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id,revision,enabled,auto_publish,policy_subject,policy_revision,execution_backend,container_build,webhook_trigger,workflow_path) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$10,$14,$15,$16,$17) ON CONFLICT(id) DO UPDATE SET connection_id=excluded.connection_id,repository_id=excluded.repository_id,branch=excluded.branch,root_directory=excluded.root_directory,recipe=excluded.recipe,revision=excluded.revision,enabled=excluded.enabled,auto_publish=excluded.auto_publish,policy_subject=excluded.policy_subject,policy_revision=excluded.policy_revision,execution_backend=excluded.execution_backend,container_build=excluded.container_build,webhook_trigger=excluded.webhook_trigger,workflow_path=excluded.workflow_path,latest_build_id=NULL",vec![id.clone().into(),customer.clone().into(),project.clone().into(),connection_id.into(),repository_id.into(),branch.into(),root_directory.into(),recipe.into(),target_id.into(),(revision+1).into(),enabled.into(),auto_publish.into(),principal.user_id.clone().into(),execution_backend.into(),container_build.into(),webhook_trigger.into(),workflow_path.into()]).await?;
                execute(&tx,"UPDATE webdock_auth.git_source SET health_path=$2,artifact_prefix=$3 WHERE id=$1",vec![id.clone().into(),health_path.into(),artifact_prefix.into()]).await?;
                if let Some(patch) = build_environment {
                    self.hosting
                        .patch_git_build_environment(&tx, &id, &patch)
                        .await
                        .map_err(hosting)?;
                    let environment = self
                        .hosting
                        .git_build_environment(&tx, &id)
                        .await
                        .map_err(hosting)?;
                    crate::execution::GuestRequest {
                        recipe: "dockerfile".into(),
                        root_directory: ".".into(),
                        build_environment: environment,
                        limits: crate::execution::Limits {
                            cpu: 2,
                            memory_mi_b: 4096,
                            disk_bytes: 10 * 1024 * 1024 * 1024,
                            duration_seconds: 1500,
                        },
                    }
                    .validate()?;
                }
                execute(
                    &tx,
                    "SELECT webdock_rls.cancel_obsolete_git_builds()",
                    vec![],
                )
                .await?;
                self.audit(
                    &tx,
                    &principal.user_id,
                    "git.source.configure",
                    &id,
                    "configured",
                )
                .await?;
                self.source_view(&tx, &Self::source(&tx, &id, &project).await?)
                    .await?
            }
            BuildCommand::BuildRequest {
                target_id,
                source_sha,
                idempotency_key,
                ..
            } => {
                let source_id = Self::select_source(&tx, &project, target_id.as_deref()).await?;
                let source = Self::source(&tx, &source_id, &project).await?;
                Self::active(&source)?;
                let prior=rows(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE project_id=$1 AND idempotency_key=$2",vec![project.clone().into(),idempotency_key.clone().into()]).await?.pop();
                if let Some(prior) = prior {
                    if prior["source_id"] != source_id {
                        return Err(error("idempotency_conflict"));
                    }
                    build_view(&prior)
                } else {
                    execute(
                        &tx,
                        "SELECT webdock_rls.cancel_obsolete_git_builds()",
                        vec![],
                    )
                    .await?;
                    let active=rows(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE project_id=$1 AND status IN ('queued','running','artifact-ready')",vec![project.clone().into()]).await?;
                    if active.len() >= 10 {
                        return Err(error("build_queue_limit"));
                    }
                    let reference = source_sha.as_deref().unwrap_or(field(&source, "branch")?);
                    let commit = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        self.provider.resolve(
                            &Self::repository(
                                &source,
                                RepositoryContext::Session {
                                    principal,
                                    tx: &tx,
                                    authority,
                                },
                            )?,
                            reference,
                        ),
                    )
                    .await
                    .map_err(|_| error("repository_timeout"))??;
                    if source_sha.as_ref().is_some_and(|s| *s != commit.sha) {
                        return Err(error("commit_mismatch"));
                    }
                    let (identity, target_revision) = self
                        .hosting
                        .git_environment_snapshot(&tx, &source_id)
                        .await
                        .map_err(hosting)?;
                    let id = self.id().await?;
                    execute(&tx,"INSERT INTO webdock_auth.git_build(id,customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,subject,idempotency_key,environment_identity,target_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",vec![id.clone().into(),customer.clone().into(),project.clone().into(),source_id.clone().into(),commit.sha.into(),(source["revision"].as_i64().unwrap() as i32).into(),(source["connection_generation"].as_i64().unwrap() as i32).into(),(source["environment_revision"].as_i64().unwrap() as i32).into(),principal.user_id.clone().into(),idempotency_key.into(),identity.clone().into(),(target_revision as i32).into()]).await?;
                    execute(&tx,"UPDATE webdock_auth.git_source SET latest_build_id=$2,environment_identity=$3 WHERE id=$1",vec![source_id.into(),id.clone().into(),identity.into()]).await?;
                    Self::check(&tx, &id, "queued", None).await?;
                    self.audit(&tx, &principal.user_id, "git.builds.request", &id, "queued")
                        .await?;
                    build_view(&Self::build(&tx, &id, &project).await?)
                }
            }
            BuildCommand::BuildGet { build_id, .. } => {
                build_view(&Self::build(&tx, &build_id, &project).await?)
            }
            BuildCommand::BuildCancel { build_id, .. } => {
                if execute(&tx,"UPDATE webdock_auth.git_build SET status='cancelled',phase='cancelled',generation=generation+1,lease_until=NULL,finished_at=now() WHERE id=$1 AND project_id=$2 AND status IN ('queued','running','artifact-ready')",vec![build_id.clone().into(),project.clone().into()]).await?!=1{return Err(error("build_not_cancellable"));}
                Self::check(&tx, &build_id, "completed", Some("cancelled")).await?;
                self.audit(
                    &tx,
                    &principal.user_id,
                    "git.builds.cancel",
                    &build_id,
                    "cancelled",
                )
                .await?;
                build_view(&Self::build(&tx, &build_id, &project).await?)
            }
            BuildCommand::BuildLogs {
                build_id,
                after,
                limit,
                ..
            } => Self::log_page(&tx, &project, &build_id, &after, limit).await?,
            BuildCommand::BuildsList {
                source_id,
                page,
                limit,
                ..
            } => Self::page(&tx, "git_build", &project, source_id, page, limit).await?,
            BuildCommand::ReleasesList {
                source_id,
                page,
                limit,
                ..
            } => Self::page(&tx, "git_release", &project, source_id, page, limit).await?,
            BuildCommand::ReleaseApprove {
                release_id,
                revision,
                idempotency_key,
                ..
            } => {
                self.approve(
                    &tx,
                    principal,
                    &project,
                    &release_id,
                    revision,
                    &idempotency_key,
                    false,
                    authority,
                )
                .await?
            }
            BuildCommand::ReleaseRollback {
                release_id,
                revision,
                idempotency_key,
                ..
            } => {
                self.approve(
                    &tx,
                    principal,
                    &project,
                    &release_id,
                    revision,
                    &idempotency_key,
                    true,
                    authority,
                )
                .await?
            }
        };
        Self::authorize(&tx, principal, &project, write, manage).await?;
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        Ok(result)
    }
    async fn page(
        tx: &DatabaseTransaction,
        table: &str,
        project: &str,
        source: Option<String>,
        page: u32,
        limit: u32,
    ) -> Result<Value> {
        let predicate = "project_id=$1 AND ($2::text IS NULL OR source_id=$2)";
        let count = rows(
            tx,
            &format!(
                "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.{table} WHERE {predicate}"
            ),
            vec![project.into(), source.clone().into()],
        )
        .await?
        .pop()
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
        let predicate = predicate
            .replace("project_id", "t.project_id")
            .replace("source_id", "t.source_id");
        let extra = if table == "git_release" {
            "(SELECT build_id FROM webdock_auth.git_artifact a WHERE a.id=t.artifact_id)"
        } else {
            "t.id"
        };
        let records=rows(tx,&format!("SELECT to_jsonb(t)||jsonb_build_object('target_id',s.target_id,'build_id',{extra}) AS value FROM webdock_auth.{table} t JOIN webdock_auth.git_source s ON s.id=t.source_id WHERE {predicate} ORDER BY t.created_at DESC,t.id DESC LIMIT $3 OFFSET $4"),vec![project.into(),source.into(),i64::from(limit).into(),(i64::from(page-1)*i64::from(limit)).into()]).await?;
        let docs = records
            .iter()
            .map(if table == "git_build" {
                build_view
            } else {
                release_view
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"docs":docs,"page":page,"limit":limit,"totalPages":count.div_ceil(u64::from(limit))}),
        )
    }
    pub async fn authenticate_worker(&self, credential: &str) -> Result<WorkerIdentity> {
        if !(32..=200).contains(&credential.len()) {
            return Err(error("worker_authentication_failed"));
        }
        let hash = hex::encode(Sha256::digest(credential.as_bytes()));
        let tx = self.worker_tx().await?;
        let row=rows(&tx,"SELECT to_jsonb(w) AS value FROM webdock_auth.git_worker w WHERE credential_hash=$1 AND enabled",vec![hash.clone().into()]).await?.pop().ok_or_else(||error("worker_authentication_failed"))?;
        let identity = WorkerIdentity {
            id: field(&row, "id")?.into(),
            generation: row["generation"]
                .as_i64()
                .ok_or_else(|| error("invalid_worker"))? as i32,
            hash,
        };
        tx.commit().await.map_err(db_error)?;
        Ok(identity)
    }
    async fn verify(tx: &DatabaseTransaction, worker: &WorkerIdentity) -> Result<Value> {
        let row=rows(tx,"SELECT to_jsonb(w) AS value FROM webdock_auth.git_worker w WHERE id=$1 AND enabled FOR SHARE",vec![worker.id.clone().into()]).await?.pop().ok_or_else(||error("worker_revoked"))?;
        if row["generation"] != worker.generation
            || !bool::from(
                field(&row, "credential_hash")?
                    .as_bytes()
                    .ct_eq(worker.hash.as_bytes()),
            )
        {
            return Err(error("worker_revoked"));
        }
        Ok(row)
    }
    pub async fn claim_build(&self, worker: &WorkerIdentity) -> Result<Option<Value>> {
        self.claim_build_compatible(worker, &["dockerfile".into()])
            .await
    }
    pub async fn claim_build_compatible(
        &self,
        worker: &WorkerIdentity,
        recipes: &[String],
    ) -> Result<Option<Value>> {
        if recipes.is_empty()
            || recipes.len() > 2
            || recipes
                .iter()
                .any(|r| !["dockerfile", crate::frontend::RECIPE].contains(&r.as_str()))
        {
            return Err(error("invalid_build_recipes"));
        }
        let tx = self.worker_tx().await?;
        let identity = Self::verify(&tx, worker).await?;
        if identity["isolation"] != "microvm" {
            return Err(error("build_isolation_unavailable"));
        }
        execute(
            &tx,
            "SELECT webdock_rls.cancel_obsolete_git_builds()",
            vec![],
        )
        .await?;
        // Serialize capacity decisions for this worker, while allowing other workers to claim independently.
        execute(
            &tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("git-worker:{}", worker.id).into()],
        )
        .await?;
        let count=rows(&tx,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.git_build WHERE worker_id=$1 AND status='running' AND lease_until>now()",vec![worker.id.clone().into()]).await?.pop().and_then(|v|v.as_i64()).unwrap_or(0);
        if count >= identity["capacity"].as_i64().unwrap_or(0) {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        }
        let build=rows(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE (b.status='queued' OR b.status='running' AND b.lease_until<now()) AND s.enabled AND $1::jsonb ? s.recipe AND s.build_provider='isolated' AND b.execution_backend='microvm' AND s.execution_backend='microvm' AND c.state='active' AND b.source_revision=s.revision AND b.connection_generation=c.generation ORDER BY b.created_at,b.id FOR UPDATE OF b SKIP LOCKED LIMIT 1",vec![json!(recipes).into()]).await?.pop();
        let Some(build) = build else {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        };
        let id = field(&build, "id")?;
        let generation = build["generation"]
            .as_i64()
            .ok_or_else(|| error("invalid_build"))?
            + 1;
        if generation >= i64::from(i32::MAX) {
            return Err(error("generation_exhausted"));
        }
        execute(&tx,"UPDATE webdock_auth.git_build SET status='running',worker_id=$2,worker_generation=$3,generation=$4,lease_until=now()+interval '27 minutes' WHERE id=$1",vec![id.into(),worker.id.clone().into(),worker.generation.into(),(generation as i32).into()]).await?;
        let source = Self::source(
            &tx,
            field(&build, "source_id")?,
            field(&build, "project_id")?,
        )
        .await?;
        Self::active(&source)?;
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
        let result = json!({"buildID":id,"customerID":build["customer_id"],"projectID":build["project_id"],"sourceID":build["source_id"],"sourceSHA":build["source_sha"],"repositoryID":source["repository_id"],"installationID":source["installation_id"],"leaseUntil":date(&deadline),"generation":generation,"request":request});
        tx.commit().await.map_err(db_error)?;
        Ok(Some(result))
    }
    async fn lease(
        tx: &DatabaseTransaction,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
    ) -> Result<Value> {
        Self::verify(tx, worker).await?;
        let build = rows(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE b.id=$1 AND b.worker_id=$2 AND b.worker_generation=$3 AND b.generation=$4 AND b.status='running' AND b.lease_until>now() AND s.enabled AND s.revision=b.source_revision AND s.environment_revision=b.environment_revision AND c.state='active' AND c.generation=b.connection_generation FOR UPDATE OF b",vec![build_id.into(),worker.id.clone().into(),worker.generation.into(),generation.into()]).await?.pop().ok_or_else(||error("build_lease_lost"))?;
        Self::guard_kubernetes_lease(tx, worker, &build, false).await?;
        Ok(build)
    }
    pub async fn guard_build(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
    ) -> Result<()> {
        let tx = self.worker_tx().await?;
        Self::lease(&tx, worker, build_id, generation).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }
    pub async fn source_archive(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
    ) -> Result<Vec<u8>> {
        let tx = self.worker_tx().await?;
        let b = Self::lease(&tx, worker, build_id, generation).await?;
        let s = Self::source(&tx, field(&b, "source_id")?, field(&b, "project_id")?).await?;
        let repository = Self::repository(
            &s,
            RepositoryContext::Build {
                worker,
                id: build_id.into(),
                generation,
            },
        )?;
        let sha = field(&b, "source_sha")?.to_owned();
        tx.commit().await.map_err(db_error)?;
        let data = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.provider.archive(&repository, &sha),
        )
        .await
        .map_err(|_| error("repository_timeout"))??;
        crate::artifact::Archive::read(&data, crate::artifact::MAX_SOURCE)?;
        self.guard_build(worker, build_id, generation).await?;
        Ok(data)
    }
    pub async fn complete_build(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        artifact: Option<&StoredArtifact>,
        logs: &str,
        failure: Option<&str>,
    ) -> Result<Value> {
        if logs.len() > 262144
            || failure.is_some_and(|c| {
                c.is_empty()
                    || c.len() > 80
                    || !c
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            })
            || artifact.is_some() == failure.is_some()
        {
            return Err(error("invalid_completion"));
        }
        let tx = self.worker_tx().await?;
        Self::verify(&tx, worker).await?;
        if let Some(artifact) = artifact {
            artifact.validate_remote()?;
        }
        let mut completion = json!({"artifact":artifact.map(|a|json!([a.key,a.digest,a.image_digest,a.size])),"logs":logs,"failure":failure});
        if let Some(remote) = artifact.and_then(StoredArtifact::remote_storage) {
            completion["remoteStorage"] = json!(remote);
        }
        let receipt = hex::encode(Sha256::digest(serde_json::to_vec(&completion).unwrap()));
        let prior=rows(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE id=$1 AND worker_id=$2 AND worker_generation=$3 AND generation=$4 FOR UPDATE",vec![build_id.into(),worker.id.clone().into(),worker.generation.into(),generation.into()]).await?.pop().ok_or_else(||error("build_lease_lost"))?;
        if matches!(
            prior["status"].as_str(),
            Some("succeeded" | "failed" | "artifact-ready")
        ) {
            if prior["completion_hash"] != receipt {
                return Err(error("completion_conflict"));
            }
            let release=rows(&tx,"SELECT to_jsonb(r) AS value FROM webdock_auth.git_release r JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id WHERE a.build_id=$1 ORDER BY r.created_at LIMIT 1",vec![build_id.into()]).await?.pop();
            tx.commit().await.map_err(db_error)?;
            return Ok(json!({"id":build_id,"status":prior["status"],"release":release}));
        }
        let b = Self::lease(&tx, worker, build_id, generation).await?;
        Self::guard_kubernetes_lease(&tx, worker, &b, true).await?;
        let s = Self::source(&tx, field(&b, "source_id")?, field(&b, "project_id")?).await?;
        let environment = self
            .hosting
            .git_build_environment(&tx, field(&s, "id")?)
            .await
            .map_err(hosting)?;
        let redactor = crate::execution::GuestRequest {
            recipe: "dockerfile".into(),
            root_directory: ".".into(),
            build_environment: environment,
            limits: crate::execution::Limits {
                cpu: 2,
                memory_mi_b: 4096,
                disk_bytes: 10 * 1024 * 1024 * 1024,
                duration_seconds: 1500,
            },
        };
        let logs = redactor.redact(logs);
        let mut release = None;
        if let Some(artifact) = artifact {
            let expected = format!("{}/{}/{}/", field(&b, "customer_id")?, build_id, generation);
            if !artifact.key.starts_with(&expected) {
                return Err(error("artifact_scope_mismatch"));
            }
            if (s["recipe"] == crate::frontend::RECIPE) != (artifact.format_version()? == 2) {
                return Err(error("artifact_format_mismatch"));
            }
            let aid = self.id().await?;
            let rid = self.id().await?;
            execute(&tx,"INSERT INTO webdock_auth.git_artifact(id,build_id,customer_id,project_id,kind,digest,storage_key,size_bytes,image_digest,remote_storage) VALUES($1,$2,$3,$4,'oci',$5,$6,$7,$8,$9)",vec![aid.clone().into(),build_id.into(),field(&b,"customer_id")?.into(),field(&b,"project_id")?.into(),artifact.digest.clone().into(),artifact.key.clone().into(),(artifact.size as i64).into(),artifact.image_digest.clone().into(),artifact.remote.as_ref().map(|r|serde_json::to_value(r).unwrap()).into()]).await?;
            execute(&tx,"INSERT INTO webdock_auth.git_release(id,customer_id,project_id,artifact_id,source_id,source_revision,connection_generation,environment_revision,subject,environment_identity,publication_target) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",vec![rid.clone().into(),field(&b,"customer_id")?.into(),field(&b,"project_id")?.into(),aid.into(),field(&b,"source_id")?.into(),(b["source_revision"].as_i64().unwrap() as i32).into(),(b["connection_generation"].as_i64().unwrap() as i32).into(),(b["environment_revision"].as_i64().unwrap() as i32).into(),field(&b,"subject")?.into(),b["environment_identity"].as_str().into(),json!({"recipe":"dockerfile","targetID":s["target_id"],"healthPath":s["health_path"]}).into()]).await?;
            if s["auto_publish"] == true && s["latest_build_id"] == build_id {
                let authority = rows(
                    &tx,
                    "SELECT coalesce(webdock_rls.git_source_policy_authority($1,$2),'null'::jsonb) AS value",
                    vec![
                        field(&s, "id")?.into(),
                        (s["revision"].as_i64().unwrap() as i32).into(),
                    ],
                )
                .await?
                .pop()
                .unwrap_or(Value::Null);
                let (identity, _) = self
                    .hosting
                    .git_environment_snapshot(&tx, field(&s, "id")?)
                    .await
                    .map_err(hosting)?;
                if !authority.is_null() && b["environment_identity"] == identity {
                    execute(&tx,"UPDATE webdock_auth.git_release SET status='superseded',revision=revision+1 WHERE source_id=$1 AND status='queued'",vec![field(&s,"id")?.into()]).await?;
                    execute(&tx,"UPDATE webdock_auth.git_release SET status='queued',approved_by=$2,approved_session_id='',approved_source='git-policy',approved_scopes='[\"hosting:read\",\"hosting:write\"]' WHERE id=$1",vec![rid.clone().into(),field(&authority,"subject")?.into()]).await?;
                    execute(
                        &tx,
                        "UPDATE webdock_auth.git_source SET desired_release_id=$2 WHERE id=$1",
                        vec![field(&s, "id")?.into(), rid.clone().into()],
                    )
                    .await?;
                }
            }
            release = rows(
                &tx,
                "SELECT to_jsonb(r) AS value FROM webdock_auth.git_release r WHERE id=$1",
                vec![rid.into()],
            )
            .await?
            .pop();
        }
        let status = if artifact.is_some() && b["execution_backend"] == "kubernetes" {
            "artifact-ready"
        } else if artifact.is_some() {
            "succeeded"
        } else {
            "failed"
        };
        execute(&tx,"UPDATE webdock_auth.git_build SET status=$2,phase=CASE WHEN $2='artifact-ready' THEN 'awaiting-approval' ELSE $2 END,logs=$3,failure_code=$4,finished_at=CASE WHEN $2='artifact-ready' THEN NULL ELSE now() END,lease_until=NULL,completion_hash=$5 WHERE id=$1",vec![build_id.into(),status.into(),logs.into(),failure.into(),receipt.into()]).await?;
        Self::check(
            &tx,
            build_id,
            if artifact.is_some() {
                "in_progress"
            } else {
                "completed"
            },
            failure.map(|_| "failure"),
        )
        .await?;
        Self::verify(&tx, worker).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"id":build_id,"status":status,"release":release}))
    }
}
impl BuildService {
    pub async fn claim_release(&self, worker: &WorkerIdentity) -> Result<Option<Value>> {
        self.claim_release_compatible(worker, &[], None, crate::artifact::SUPPORTED_FORMATS)
            .await
    }
    /// Compiled callers support this reader's formats; wire callers advertise their own set.
    pub async fn claim_release_with_storage(
        &self,
        worker: &WorkerIdentity,
        bindings: &[String],
        publication_projects: &std::collections::BTreeMap<String, String>,
    ) -> Result<Option<Value>> {
        self.claim_release_compatible(
            worker,
            bindings,
            Some(publication_projects),
            crate::artifact::SUPPORTED_FORMATS,
        )
        .await
    }
    pub async fn claim_release_compatible(
        &self,
        worker: &WorkerIdentity,
        bindings: &[String],
        publication_projects: Option<&std::collections::BTreeMap<String, String>>,
        formats: &[u8],
    ) -> Result<Option<Value>> {
        if formats.is_empty()
            || formats.len() > crate::artifact::SUPPORTED_FORMATS.len()
            || formats
                .iter()
                .any(|format| !crate::artifact::SUPPORTED_FORMATS.contains(format))
        {
            return Err(error("invalid_artifact_formats"));
        }
        if publication_projects.is_none() && !bindings.is_empty() {
            return Err(error("invalid_publication_projects"));
        }
        if let Some(projects) = publication_projects {
            if projects.len() > 128 {
                return Err(error("invalid_publication_projects"));
            }
            for (project, repository) in projects {
                crate::contracts::id(project)?;
                if repository.is_empty()
                    || repository.len() > 512
                    || repository.chars().any(char::is_control)
                {
                    return Err(error("invalid_publication_projects"));
                }
            }
        }
        self.claim_release_inner(worker, bindings, publication_projects, formats)
            .await
    }
    async fn claim_release_inner(
        &self,
        worker: &WorkerIdentity,
        bindings: &[String],
        publication_projects: Option<&std::collections::BTreeMap<String, String>>,
        formats: &[u8],
    ) -> Result<Option<Value>> {
        if bindings.len() > 16
            || bindings.iter().any(|b| {
                b.len() != 64
                    || !b
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
        {
            return Err(error("invalid_artifact_bindings"));
        }
        let tx = self.worker_tx().await?;
        Self::verify(&tx, worker).await?;
        execute(&tx,"UPDATE webdock_auth.git_release SET status='needs-reconciliation',revision=revision+1 WHERE status='deploying' AND lease_until<now()",vec![]).await?;
        // A canceled Kubernetes build can never have staged an operation: the same
        // build row is fenced atomically with staging. Retain bytes, retire only intent.
        execute(&tx,"UPDATE webdock_auth.git_release r SET status='superseded',revision=r.revision+1,lease_until=NULL FROM webdock_auth.git_artifact a JOIN webdock_auth.git_build b ON b.id=a.build_id WHERE r.artifact_id=a.id AND r.customer_id=b.customer_id AND r.project_id=b.project_id AND b.execution_backend='kubernetes' AND b.status IN ('cancelled','failed') AND r.operation_id IS NULL AND r.status IN ('queued','deploying','needs-reconciliation')",vec![]).await?;
        let release=rows(&tx,"SELECT to_jsonb(r)||jsonb_build_object('storage_key',a.storage_key,'digest',a.digest,'image_digest',a.image_digest,'size_bytes',a.size_bytes,'build_id',a.build_id,'remote_storage',a.remote_storage) AS value FROM webdock_auth.git_release r JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id JOIN webdock_auth.git_build b ON b.id=a.build_id JOIN webdock_auth.git_source s ON s.id=r.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE r.status='queued' AND a.retained AND a.kind='oci' AND a.image_digest IS NOT NULL AND (($6::bool AND a.storage_key ~ '/[a-f0-9]{64}[.]tar$') OR ($7::bool AND a.storage_key ~ '/[a-f0-9]{64}[.]e1[.]tar$') OR ($8::bool AND a.storage_key ~ '/[a-f0-9]{64}[.]e2[.]tar$')) AND ((a.remote_storage IS NULL AND b.worker_id=$1 AND b.worker_generation=$2) OR (a.remote_storage IS NOT NULL AND $3::jsonb ? (a.remote_storage->>'binding'))) AND ($4::jsonb IS NULL OR ($4::jsonb->>r.project_id)=($5::text||'/customers/'||r.customer_id||'/projects/'||r.project_id)) AND s.enabled AND s.desired_release_id=r.id AND s.revision=r.source_revision AND s.environment_revision=r.environment_revision AND c.generation=r.connection_generation AND c.state='active' AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_release busy WHERE busy.source_id=r.source_id AND busy.status IN ('deploying','needs-reconciliation')) ORDER BY r.created_at,r.id FOR UPDATE OF r,s SKIP LOCKED LIMIT 1",vec![worker.id.clone().into(),worker.generation.into(),json!(bindings).into(),publication_projects.map(|p|json!(p)).into(),self.registry.clone().into(),formats.contains(&0).into(),formats.contains(&1).into(),formats.contains(&2).into()]).await?.pop();
        let Some(mut r) = release else {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        };
        let id = field(&r, "id")?.to_owned();
        let authority = rows(
            &tx,
            "SELECT coalesce(webdock_rls.git_release_authority($1),'null'::jsonb) AS value",
            vec![id.clone().into()],
        )
        .await?
        .pop()
        .unwrap_or(Value::Null);
        let (snapshot, _) = self
            .hosting
            .git_environment_snapshot(&tx, field(&r, "source_id")?)
            .await
            .map_err(hosting)?;
        if authority.is_null() || r["environment_identity"] != snapshot {
            execute(&tx,"UPDATE webdock_auth.git_release SET status='superseded',revision=revision+1 WHERE id=$1",vec![id.into()]).await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        }
        let authority_guard = match self.release_authority(&r).await {
            Ok(guard) => guard,
            Err(e) if e.code == "session_authority_revoked" || e.code == "permission_denied" => {
                execute(&tx,"UPDATE webdock_auth.git_release SET status='superseded',revision=revision+1 WHERE id=$1",vec![id.into()]).await?;
                tx.commit().await.map_err(db_error)?;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let generation = r["generation"]
            .as_i64()
            .ok_or_else(|| error("invalid_release"))?
            + 1;
        if generation >= i64::from(i32::MAX) {
            return Err(error("generation_exhausted"));
        }
        execute(&tx,"UPDATE webdock_auth.git_release SET status='deploying',generation=$2,revision=revision+1,worker_id=$3,worker_generation=$4,lease_until=now()+interval '10 minutes' WHERE id=$1",vec![id.into(),(generation as i32).into(),worker.id.clone().into(),worker.generation.into()]).await?;
        Self::check(&tx, field(&r, "build_id")?, "in_progress", None).await?;
        r["lease_until"] = date(
            &rows(
                &tx,
                "SELECT to_jsonb(lease_until) AS value FROM webdock_auth.git_release WHERE id=$1",
                vec![field(&r, "id")?.into()],
            )
            .await?
            .pop()
            .ok_or_else(|| error("release_lease_lost"))?,
        );
        r["generation"] = json!(generation);
        r["status"] = json!("deploying");
        Self::verify(&tx, worker).await?;
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        Ok(Some(r))
    }
    async fn release_lease(
        tx: &DatabaseTransaction,
        worker: &WorkerIdentity,
        id: &str,
        generation: i32,
    ) -> Result<Value> {
        Self::verify(tx, worker).await?;
        let r=rows(tx,"SELECT to_jsonb(r)||jsonb_build_object('image_digest',a.image_digest,'build_id',a.build_id,'storage_key',a.storage_key,'digest',a.digest,'size_bytes',a.size_bytes,'remote_storage',a.remote_storage) AS value FROM webdock_auth.git_release r JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id WHERE r.id=$1 AND r.worker_id=$2 AND r.worker_generation=$3 AND r.generation=$4 AND r.status='deploying' AND r.lease_until>now() AND a.retained FOR UPDATE OF r",vec![id.into(),worker.id.clone().into(),worker.generation.into(),generation.into()]).await?.pop().ok_or_else(||error("release_lease_lost"))?;
        Self::guard_kubernetes_publication(tx, &r).await?;
        let authority = rows(
            tx,
            "SELECT coalesce(webdock_rls.git_release_authority($1),'null'::jsonb) AS value",
            vec![id.into()],
        )
        .await?
        .pop()
        .unwrap_or(Value::Null);
        if authority.is_null() {
            return Err(error("release_authority_revoked"));
        }
        Ok(r)
    }
    /// Marks the external write boundary durably before the publisher contacts a registry.
    pub async fn prepare_publication(
        &self,
        worker: &WorkerIdentity,
        id: &str,
        generation: i32,
    ) -> Result<Value> {
        self.revalidate_repository(worker, id, generation).await?;
        let tx = self.worker_tx().await?;
        let r = Self::release_lease(&tx, worker, id, generation).await?;
        let authority_guard = self.release_authority(&r).await?;
        let (snap, _) = self
            .hosting
            .git_environment_snapshot(&tx, field(&r, "source_id")?)
            .await
            .map_err(hosting)?;
        if r["environment_identity"] != snap {
            return Err(error("environment_changed"));
        }
        self.hosting
            .git_publication_ready(&tx, field(&r, "source_id")?)
            .await
            .map_err(hosting)?;
        let repository = format!(
            "{}/customers/{}/projects/{}",
            self.registry,
            field(&r, "customer_id")?,
            field(&r, "project_id")?
        );
        execute(&tx,"UPDATE webdock_auth.git_build SET phase='pushing' WHERE id=(SELECT build_id FROM webdock_auth.git_artifact WHERE id=$1) AND execution_backend='kubernetes' AND status='artifact-ready'",vec![field(&r,"artifact_id")?.into()]).await?;
        execute(&tx,"UPDATE webdock_auth.git_release SET publication_started_at=coalesce(publication_started_at,now()),publication_target=coalesce(publication_target,'{}')||jsonb_build_object('imageRepository',$2::text) WHERE id=$1",vec![id.into(),repository.clone().into()]).await?;
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        Ok(
            json!({"releaseID":id,"repository":repository,"imageDigest":r["image_digest"],"storageKey":r["storage_key"],"digest":r["digest"],"sizeBytes":r["size_bytes"],"remoteStorage":r["remote_storage"]}),
        )
    }
    pub async fn publish_image(
        &self,
        worker: &WorkerIdentity,
        id: &str,
        generation: i32,
        image: &str,
    ) -> Result<Value> {
        let replay = self.worker_tx().await?;
        Self::verify(&replay, worker).await?;
        // An accepted immutable operation is a receipt, not fresh promotion authority.
        // A lost callback response must remain replayable after rollout or lease expiry.
        if let Some(accepted)=rows(&replay,"SELECT to_jsonb(r)||jsonb_build_object('accepted_image',o.desired->'spec'->>'image') AS value FROM webdock_auth.git_release r JOIN webdock_auth.hosting_operation o ON o.id=r.operation_id AND o.git_release_id=r.id AND o.project_id=r.project_id WHERE r.id=$1 AND r.worker_id=$2 AND r.worker_generation=$3 AND r.generation=$4",vec![id.into(),worker.id.clone().into(),worker.generation.into(),generation.into()]).await?.pop() {
            if accepted["accepted_image"]!=image {return Err(error("publication_image_conflict"));}
            Self::guard_kubernetes_publication(&replay,&accepted).await?;
            let result=json!({"releaseID":id,"operationID":accepted["operation_id"],"status":accepted["status"],"replayed":true});
            replay.commit().await.map_err(db_error)?;return Ok(result);
        }
        let prior = Self::release_lease(&replay, worker, id, generation).await?;
        let prior_authority = self.release_authority(&prior).await?;
        if prior["operation_id"].as_str().is_some() {
            let result = self
                .hosting
                .publish_git_release_with_authority_in(
                    &replay,
                    prior_authority
                        .as_ref()
                        .map(session::SessionGuard::transaction),
                    id,
                    &worker.id,
                    worker.generation,
                    generation,
                    image,
                )
                .await
                .map_err(hosting)?;
            Self::mark_kubernetes_published(&replay, &prior).await?;
            session::verify(&prior_authority).await?;
            replay.commit().await.map_err(db_error)?;
            session::commit(prior_authority).await?;
            return Ok(result);
        }
        session::verify(&prior_authority).await?;
        replay.commit().await.map_err(db_error)?;
        session::commit(prior_authority).await?;
        self.revalidate_repository(worker, id, generation).await?;
        let tx = self.worker_tx().await?;
        let row = Self::release_lease(&tx, worker, id, generation).await?;
        let authority_guard = self.release_authority(&row).await?;
        let result = self
            .hosting
            .publish_git_release_with_authority_in(
                &tx,
                authority_guard
                    .as_ref()
                    .map(session::SessionGuard::transaction),
                id,
                &worker.id,
                worker.generation,
                generation,
                image,
            )
            .await
            .map_err(hosting)?;
        Self::mark_kubernetes_published(&tx, &row).await?;
        Self::verify(&tx, worker).await?;
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        Ok(result)
    }
    /// Publication failures after prepare are ambiguous and never automatically requeued.
    pub async fn publication_uncertain(
        &self,
        worker: &WorkerIdentity,
        id: &str,
        generation: i32,
    ) -> Result<()> {
        let tx = self.worker_tx().await?;
        Self::verify(&tx, worker).await?;
        if execute(&tx,"UPDATE webdock_auth.git_release SET status='needs-reconciliation',revision=revision+1,lease_until=NULL WHERE id=$1 AND worker_id=$2 AND worker_generation=$3 AND generation=$4 AND status='deploying'",vec![id.into(),worker.id.clone().into(),worker.generation.into(),generation.into()]).await?!=1{return Err(error("release_lease_lost"));}
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }
    pub async fn observe_release(
        &self,
        worker: &WorkerIdentity,
        id: &str,
        generation: i32,
    ) -> Result<Value> {
        let tx = self.worker_tx().await?;
        Self::verify(&tx, worker).await?;
        if let Some(done)=rows(&tx,"SELECT to_jsonb(r) AS value FROM webdock_auth.git_release r WHERE id=$1 AND worker_id=$2 AND worker_generation=$3 AND generation=$4 AND status IN ('ready','failed')",vec![id.into(),worker.id.clone().into(),worker.generation.into(),generation.into()]).await?.pop(){tx.commit().await.map_err(db_error)?;return Ok(json!({"id":id,"status":done["status"],"operationID":done["operation_id"],"healthVerified":done["status"]=="ready"}));}
        let r = Self::release_lease(&tx, worker, id, generation).await?;
        let authority_guard = self.release_authority(&r).await?;
        let op=rows(&tx,"SELECT to_jsonb(o)||jsonb_build_object('app_status',a.status,'observed_revision',a.observed_revision) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_app a ON a.id=o.app_id WHERE o.id=$1 AND o.git_release_id=$2",vec![r["operation_id"].as_str().into(),id.into()]).await?.pop();
        let status = match op.as_ref() {
            Some(o)
                if o["status"] == "succeeded"
                    && o["app_status"] == "ready"
                    && o["observed_revision"] == o["target_revision"] =>
            {
                "ready"
            }
            Some(o) if o["status"] == "failed" => "failed",
            Some(o) if o["status"] == "needs-reconciliation" => "needs-reconciliation",
            None => "needs-reconciliation",
            _ => "deploying",
        };
        if status != "deploying" {
            execute(&tx,"UPDATE webdock_auth.git_release SET status=$2,revision=revision+1,lease_until=NULL WHERE id=$1",vec![id.into(),status.into()]).await?;
            Self::check(
                &tx,
                field(&r, "build_id")?,
                if status == "needs-reconciliation" {
                    "in_progress"
                } else {
                    "completed"
                },
                match status {
                    "ready" => Some("success"),
                    "failed" => Some("failure"),
                    _ => None,
                },
            )
            .await?;
        }
        Self::verify(&tx, worker).await?;
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        Ok(
            json!({"id":id,"status":status,"operationID":r["operation_id"],"healthVerified":status=="ready"}),
        )
    }
}
impl BuildService {
    async fn select_source(
        tx: &DatabaseTransaction,
        project: &str,
        target: Option<&str>,
    ) -> Result<String> {
        let sources=rows(tx,"SELECT to_jsonb(s) AS value FROM webdock_auth.git_source s WHERE project_id=$1 AND ($2::text IS NULL OR target_id=$2) ORDER BY id",vec![project.into(),target.into()]).await?;
        if sources.len() != 1 {
            return Err(error(if sources.is_empty() {
                "source_unavailable"
            } else {
                "select_source_target"
            }));
        }
        Ok(field(&sources[0], "id")?.into())
    }
    async fn source_view(&self, tx: &DatabaseTransaction, s: &Value) -> Result<Value> {
        let environment = self
            .hosting
            .git_build_environment(tx, field(s, "id")?)
            .await
            .map_err(hosting)?;
        Ok(
            json!({"id":s["id"],"customerID":s["customer_id"],"projectID":s["project_id"],"connectionID":s["connection_id"],"repositoryID":s["repository_id"],"repositoryName":s["metadata"]["fullName"],"branch":s["branch"],"rootDirectory":s["root_directory"],"recipe":s["recipe"],"buildProvider":s["build_provider"],"executionBackend":s["execution_backend"],"containerBuild":s["container_build"],"environmentName":s["environment_name"],"workflowPath":s["workflow_path"],"webhookTrigger":s["webhook_trigger"],"artifactPrefix":s["artifact_prefix"],"healthPath":s["health_path"],"targetID":s["target_id"],"revision":s["revision"],"enabled":s["enabled"],"autoPublish":s["auto_publish"],"webhookEnabled":s["webhook_enabled"]==true && s["webhook_revision"]==s["revision"] && s["webhook_connection_generation"]==s["connection_generation"] && s["webhook_environment_revision"]==s["environment_revision"],"buildEnvironmentNames":environment.keys().collect::<Vec<_>>()}),
        )
    }
    async fn revalidate_repository(
        &self,
        worker: &WorkerIdentity,
        id: &str,
        generation: i32,
    ) -> Result<()> {
        let tx = self.worker_tx().await?;
        let r = Self::release_lease(&tx, worker, id, generation).await?;
        let authority_guard = self.release_authority(&r).await?;
        let source = Self::source(&tx, field(&r, "source_id")?, field(&r, "project_id")?).await?;
        let b = rows(
            &tx,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE id=$1",
            vec![field(&r, "build_id")?.into()],
        )
        .await?
        .pop()
        .ok_or_else(|| error("build_unavailable"))?;
        let expected = field(&b, "source_sha")?.to_owned();
        let reference = if r["approved_source"] == "git-policy" {
            field(&source, "branch")?
        } else {
            &expected
        }
        .to_owned();
        let repository = Self::repository(
            &source,
            RepositoryContext::Release {
                worker,
                id: id.into(),
                generation,
            },
        )?;
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        let commit = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.provider.resolve(&repository, &reference),
        )
        .await
        .map_err(|_| error("repository_timeout"))??;
        if commit.sha != expected {
            return Err(error("repository_head_changed"));
        }
        self.guard_release(worker, id, generation).await?;
        Ok(())
    }
}
impl BuildService {
    pub async fn guard_release(
        &self,
        worker: &WorkerIdentity,
        id: &str,
        generation: i32,
    ) -> Result<()> {
        let tx = self.worker_tx().await?;
        let r = Self::release_lease(&tx, worker, id, generation).await?;
        let authority_guard = self.release_authority(&r).await?;
        let (snapshot, _) = self
            .hosting
            .git_environment_snapshot(&tx, field(&r, "source_id")?)
            .await
            .map_err(hosting)?;
        if r["environment_identity"] != snapshot {
            return Err(error("environment_changed"));
        }
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        Ok(())
    }
}
fn date(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|d| {
            json!(
                d.to_utc()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            )
        })
        .unwrap_or(Value::Null)
}
fn build_view(r: &Value) -> Value {
    json!({"id":r["id"],"projectID":r["project_id"],"sourceID":r["source_id"],"targetID":r["target_id"],"sourceSHA":r["source_sha"],"status":r["status"],"revision":r["source_revision"],"generation":r["generation"],"createdAt":date(&r["created_at"]),"finishedAt":date(&r["finished_at"]),"logs":r["logs"],"failureCode":r["failure_code"],"actionsRunURL":null,"executionBackend":r["execution_backend"],"environmentName":r["environment_name"],"phase":r["phase"],"registryVerifiedAt":date(&r["registry_verified_at"]),"retryCount":r["retry_count"],"nextEligibleAt":date(&r["next_eligible_at"]),"lastInfrastructureFailure":r["last_infrastructure_failure"]})
}
fn release_view(r: &Value) -> Value {
    json!({"id":r["id"],"projectID":r["project_id"],"sourceID":r["source_id"],"targetID":r.get("target_id").unwrap_or(&r["publication_target"]["targetID"]),"artifactID":r["artifact_id"],"buildID":r["build_id"],"status":r["status"],"revision":r["revision"],"approvedBy":r["approved_by"],"operationID":r["operation_id"],"providerDeploymentID":r["provider_deployment_id"],"createdAt":date(&r["created_at"])})
}
impl BuildService {
    async fn build(tx: &DatabaseTransaction, id: &str, project: &str) -> Result<Value> {
        rows(tx,"SELECT to_jsonb(b)||jsonb_build_object('target_id',s.target_id) AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id WHERE b.id=$1 AND b.project_id=$2",vec![id.into(),project.into()]).await?.pop().ok_or_else(||error("build_unavailable"))
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    async fn approve(
        &self,
        tx: &DatabaseTransaction,
        principal: &Principal,
        project: &str,
        id: &str,
        revision: i32,
        key: &str,
        rollback: bool,
        authority: Option<&spitzli_browser_session::StoredAuthority>,
    ) -> Result<Value> {
        let release=rows(tx,"SELECT to_jsonb(r)||jsonb_build_object('build_id',a.build_id) AS value FROM webdock_auth.git_release r JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id AND a.retained WHERE r.id=$1 AND r.project_id=$2 FOR UPDATE OF r",vec![id.into(),project.into()]).await?.pop().ok_or_else(||error("release_unavailable"))?;
        Self::guard_kubernetes_publication(tx, &release).await?;
        let source = Self::source(tx, field(&release, "source_id")?, project).await?;
        Self::active(&source)?;
        if let Some(prior)=rows(tx,"SELECT to_jsonb(r) AS value FROM webdock_auth.git_release r WHERE project_id=$1 AND idempotency_key=$2",vec![project.into(),key.into()]).await?.pop(){if prior["source_id"]!=release["source_id"]{return Err(error("idempotency_conflict"));}return Ok(release_view(&prior));}
        if release["revision"] != revision
            || if rollback {
                release["status"] != "ready"
            } else {
                !matches!(
                    release["status"].as_str(),
                    Some("awaiting-approval" | "queued")
                ) || source["latest_build_id"] != release["build_id"]
            }
            || release["source_revision"] != source["revision"]
            || release["connection_generation"] != source["connection_generation"]
            || release["environment_revision"] != source["environment_revision"]
        {
            return Err(error("release_revision_conflict"));
        }
        let (snapshot, _) = self
            .hosting
            .git_environment_snapshot(tx, field(&source, "id")?)
            .await
            .map_err(hosting)?;
        if release["environment_identity"] != snapshot {
            return Err(error("environment_changed"));
        }
        let target = if rollback {
            let new_id = self.id().await?;
            execute(tx,"INSERT INTO webdock_auth.git_release(id,customer_id,project_id,artifact_id,source_id,source_revision,connection_generation,environment_revision,subject,environment_identity,publication_target) SELECT $2,customer_id,project_id,artifact_id,source_id,source_revision,connection_generation,environment_revision,$3,environment_identity,publication_target-'imageRepository' FROM webdock_auth.git_release WHERE id=$1",vec![id.into(),new_id.clone().into(),principal.user_id.clone().into()]).await?;
            new_id
        } else {
            id.to_owned()
        };
        execute(tx,"UPDATE webdock_auth.git_release SET status='superseded',revision=revision+1 WHERE source_id=$1 AND id<>$2 AND status='queued'",vec![field(&source,"id")?.into(),target.clone().into()]).await?;
        execute(tx,"UPDATE webdock_auth.git_release SET status='queued',revision=revision+$5,approved_by=$2,approved_session_id=$3,approved_source='studio',approved_scopes='[\"hosting:read\",\"hosting:write\"]',idempotency_key=$4,publication_target=jsonb_set(publication_target,'{approvedAuthority}',$6::jsonb,true) WHERE id=$1",vec![target.clone().into(),principal.user_id.clone().into(),principal.session_id.clone().into(),key.into(),i32::from(!rollback).into(),serde_json::to_value(authority).map_err(|_|error("invalid_authority"))?.into()]).await?;
        execute(
            tx,
            "UPDATE webdock_auth.git_source SET desired_release_id=$2 WHERE id=$1",
            vec![field(&source, "id")?.into(), target.clone().into()],
        )
        .await?;
        self.audit(
            tx,
            &principal.user_id,
            if rollback {
                "git.releases.rollback"
            } else {
                "git.releases.approve"
            },
            &target,
            "queued",
        )
        .await?;
        let row = rows(
            tx,
            "SELECT to_jsonb(r) AS value FROM webdock_auth.git_release r WHERE id=$1",
            vec![target.into()],
        )
        .await?
        .pop()
        .ok_or_else(|| error("release_unavailable"))?;
        Ok(release_view(&row))
    }
}
impl BuildService {
    async fn operator(tx: &DatabaseTransaction, actor: &Principal) -> Result<()> {
        let row = rows(
            tx,
            "SELECT coalesce(webdock_rls.actor(),'null'::jsonb) AS value",
            vec![],
        )
        .await?
        .pop()
        .unwrap_or(Value::Null);
        if row["operator"] != true
            || row["subject"] != actor.user_id
            || row["sessionID"] != actor.session_id
            || !row["preview"].is_null()
        {
            return Err(error("operator_required"));
        }
        Ok(())
    }
    pub async fn enroll_worker(
        &self,
        actor: &Principal,
        country: &str,
        isolation: &str,
        evidence: &str,
        capacity: u32,
    ) -> Result<Value> {
        if country.len() != 2
            || !country.bytes().all(|b| b.is_ascii_uppercase())
            || !(1..=32).contains(&capacity)
            || !(20..=4000).contains(&evidence.len())
            || !matches!(isolation, "microvm" | "artifact-only")
            || (country == "ZZ"
                && !evidence
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| w.eq_ignore_ascii_case("unverified")))
        {
            return Err(error("invalid_worker_evidence"));
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        Self::module(&tx, false).await?;
        Self::operator(&tx, actor).await?;
        use base64::Engine;
        let mut random = [0u8; 32];
        getrandom::getrandom(&mut random).map_err(|_| error("entropy_unavailable"))?;
        let credential = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
        let id = self.id().await?;
        execute(&tx,"INSERT INTO webdock_auth.git_worker(id,credential_hash,enabled,country,isolation,evidence,capacity) VALUES($1,$2,true,$3,$4,$5,$6)",vec![id.clone().into(),hex::encode(Sha256::digest(credential.as_bytes())).into(),country.into(),isolation.into(),evidence.into(),(capacity as i32).into()]).await?;
        self.audit(&tx, &actor.user_id, "git.worker.enroll", &id, "enabled")
            .await?;
        Self::operator(&tx, actor).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"id":id,"generation":1,"credential":credential}))
    }
    pub async fn revoke_worker(&self, actor: &Principal, id: &str, generation: i32) -> Result<()> {
        crate::contracts::id(id)?;
        if generation <= 0 || generation == i32::MAX {
            return Err(error("invalid_generation"));
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        Self::operator(&tx, actor).await?;
        if execute(&tx,"UPDATE webdock_auth.git_worker SET enabled=false,generation=generation+1 WHERE id=$1 AND generation=$2",vec![id.into(),generation.into()]).await?!=1{return Err(error("worker_generation_conflict"));}
        self.audit(&tx, &actor.user_id, "git.worker.revoke", id, "revoked")
            .await?;
        Self::operator(&tx, actor).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }
    /// Complete the real worker-local publish flow. Any failure after the prepared external-write
    /// boundary leaves durable reconciliation state, including authority changes after upload.
    pub async fn publish_local_artifact(
        &self,
        worker: &WorkerIdentity,
        id: &str,
        generation: i32,
        store: &crate::artifact::ArtifactStore,
        publisher: &crate::publisher::RegistryPublisher,
    ) -> Result<Value> {
        let prepared = self.prepare_publication(worker, id, generation).await?;
        let tx = self.worker_tx().await?;
        let r = Self::release_lease(&tx, worker, id, generation).await?;
        let authority_guard = self.release_authority(&r).await?;
        let customer = field(&r, "customer_id")?.to_owned();
        let project = field(&r, "project_id")?.to_owned();
        session::verify(&authority_guard).await?;
        tx.commit().await.map_err(db_error)?;
        session::commit(authority_guard).await?;
        struct Guard<'a> {
            service: &'a BuildService,
            worker: &'a WorkerIdentity,
            id: &'a str,
            generation: i32,
        }
        #[async_trait]
        impl crate::publisher::PublicationGuard for Guard<'_> {
            async fn check(&self) -> Result<()> {
                self.service
                    .guard_release(self.worker, self.id, self.generation)
                    .await
            }
        }
        let result = async {
            let artifact = store.load(
                field(&prepared, "storageKey")?.into(),
                field(&prepared, "digest")?.into(),
                prepared["sizeBytes"]
                    .as_u64()
                    .ok_or_else(|| error("invalid_artifact"))?,
                field(&prepared, "imageDigest")?.into(),
            )?;
            let image = publisher
                .publish(
                    &customer,
                    &project,
                    id,
                    store,
                    &artifact,
                    &Guard {
                        service: self,
                        worker,
                        id,
                        generation,
                    },
                )
                .await?;
            self.publish_image(worker, id, generation, &image).await
        }
        .await;
        if result.is_err() {
            let _ = self.publication_uncertain(worker, id, generation).await;
        }
        result
    }
}

#[path = "connections.rs"]
mod connections;

#[path = "repository.rs"]
pub(crate) mod repository;

#[path = "session.rs"]
mod session;

#[path = "export.rs"]
mod export;

#[path = "service/kubernetes.rs"]
pub mod kubernetes;

#[path = "service/logs.rs"]
mod logs;

#[path = "service/github_webhook.rs"]
pub mod github_webhook;

#[path = "service/infrastructure.rs"]
mod infrastructure;

#[path = "service/volumes.rs"]
mod volumes;
