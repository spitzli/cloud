//! Signed GitHub events enqueue existing source-bound builds; provider credentials stay in plugins.
use super::*;
use serde::Deserialize;
use spitzli_repository_protocol::Action;

pub const MAX_GITHUB_WEBHOOK_BODY: usize = 1024 * 1024;

/// Construct only from the explicitly configured private secret reference and repository plugin.
/// Deliberately implements neither Debug nor Serialize.
pub struct GithubWebhookVerifier {
    secret: Vec<u8>,
    repository_plugin_id: String,
}

/// Opaque proof: unverified JSON can never be passed to the ingestion method.
pub struct VerifiedGithubEvent {
    delivery_id: String,
    repository_plugin_id: String,
    body_sha256: String,
    event: String,
    push: Option<Push>,
}

struct Push {
    installation_id: String,
    repository_id: String,
    branch: String,
    sha: String,
    workflow_path: Option<String>,
}

#[derive(Deserialize)]
struct ExternalId {
    id: u64,
}
#[derive(Deserialize)]
struct PushBody {
    installation: ExternalId,
    repository: ExternalId,
    #[serde(rename = "ref")]
    reference: String,
    after: String,
    deleted: bool,
}

#[derive(Deserialize)]
struct WorkflowRepository {
    id: u64,
    fork: bool,
}
#[derive(Deserialize)]
struct WorkflowDefinition {
    id: u64,
    path: String,
}
#[derive(Deserialize)]
struct WorkflowRun {
    id: u64,
    workflow_id: u64,
    event: String,
    status: String,
    conclusion: Option<String>,
    head_branch: Option<String>,
    head_sha: String,
    repository: WorkflowRepository,
    head_repository: Option<WorkflowRepository>,
}
#[derive(Deserialize)]
struct WorkflowBody {
    action: String,
    installation: ExternalId,
    repository: WorkflowRepository,
    workflow: Option<WorkflowDefinition>,
    workflow_run: WorkflowRun,
}
fn workflow_candidate(raw: &[u8]) -> Result<Option<Push>> {
    let body: WorkflowBody =
        serde_json::from_slice(raw).map_err(|_| error("invalid_github_workflow"))?;
    let run = body.workflow_run;
    let (Some(workflow), Some(head), Some(head_branch)) =
        (body.workflow, run.head_repository, run.head_branch)
    else {
        return Ok(None);
    };
    if body.action != "completed"
        || run.status != "completed"
        || run.conclusion.as_deref() != Some("success")
        || run.event != "push"
        || body.repository.fork
        || run.repository.fork
        || head.fork
        || body.repository.id != run.repository.id
        || body.repository.id != head.id
        || workflow.id != run.workflow_id
    {
        return Ok(None);
    }
    if [
        body.installation.id,
        body.repository.id,
        run.id,
        workflow.id,
    ]
    .iter()
    .any(|id| *id == 0 || *id > i64::MAX as u64)
        || workflow.path.len() > 300
    {
        return Err(error("invalid_github_workflow"));
    }
    branch(&head_branch)?;
    crate::contracts::sha(&run.head_sha)?;
    if run.head_sha.bytes().all(|b| b == b'0') {
        return Err(error("invalid_github_workflow"));
    }
    Ok(Some(Push {
        installation_id: body.installation.id.to_string(),
        repository_id: body.repository.id.to_string(),
        branch: head_branch,
        sha: run.head_sha,
        workflow_path: Some(workflow.path),
    }))
}

fn branch(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 200
        || value
            .as_bytes()
            .first()
            .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
        || value.contains("..")
        || value.contains("//")
        || value.ends_with('/')
        || value.ends_with(".lock")
        || value
            .split('/')
            .any(|part| part.starts_with('.') || part.ends_with('.') || part.ends_with(".lock"))
    {
        return Err(error("invalid_github_ref"));
    }
    Ok(())
}

impl GithubWebhookVerifier {
    pub fn from_private_file(path: &std::path::Path, repository_plugin_id: String) -> Result<Self> {
        use std::{
            io::Read,
            os::unix::fs::{MetadataExt, OpenOptionsExt},
        };
        if !path.is_absolute() {
            return Err(error("invalid_github_webhook_configuration"));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| error("github_webhook_configuration_unavailable"))?;
        let metadata = file
            .metadata()
            .map_err(|_| error("github_webhook_configuration_unavailable"))?;
        if !metadata.is_file() || metadata.nlink()!=1 || metadata.len()>4096
            // SAFETY: geteuid has no preconditions or pointer arguments.
            || metadata.uid()!=unsafe {libc::geteuid()}
            || !matches!(metadata.mode()&0o7777,0o400|0o600)
        {
            return Err(error("invalid_github_webhook_configuration"));
        }
        let mut secret = Vec::new();
        file.take(4097)
            .read_to_end(&mut secret)
            .map_err(|_| error("github_webhook_configuration_unavailable"))?;
        Self::new(secret, repository_plugin_id)
    }

    pub fn new(secret: Vec<u8>, repository_plugin_id: String) -> Result<Self> {
        if !(32..=4096).contains(&secret.len())
            || repository_plugin_id.is_empty()
            || repository_plugin_id.len() > 128
            || !repository_plugin_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(error("invalid_github_webhook_configuration"));
        }
        Ok(Self {
            secret,
            repository_plugin_id,
        })
    }

    /// Verify the original HTTP bytes, never reserialized JSON. All ignored events are signed too.
    pub fn verify(
        &self,
        raw: &[u8],
        signature: &str,
        delivery_id: &str,
        event: &str,
    ) -> Result<VerifiedGithubEvent> {
        if raw.len() > MAX_GITHUB_WEBHOOK_BODY {
            return Err(error("github_webhook_body_limit"));
        }
        let signature = signature
            .strip_prefix("sha256=")
            .filter(|v| {
                v.len() == 64
                    && v.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or_else(|| error("github_webhook_signature_denied"))?;
        let bytes = hex::decode(signature).map_err(|_| error("github_webhook_signature_denied"))?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret)
            .map_err(|_| error("invalid_github_webhook_configuration"))?;
        mac.update(raw);
        mac.verify_slice(&bytes)
            .map_err(|_| error("github_webhook_signature_denied"))?;
        let delivery = uuid::Uuid::parse_str(delivery_id)
            .ok()
            .filter(|v| !v.is_nil() && v.to_string() == delivery_id)
            .ok_or_else(|| error("invalid_github_delivery"))?;
        if event.is_empty()
            || event.len() > 64
            || !event.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            return Err(error("invalid_github_event"));
        }
        let push = if event == "push" {
            let body: PushBody =
                serde_json::from_slice(raw).map_err(|_| error("invalid_github_push"))?;
            if body.installation.id == 0
                || body.repository.id == 0
                || body.installation.id > i64::MAX as u64
                || body.repository.id > i64::MAX as u64
            {
                return Err(error("invalid_github_push"));
            }
            crate::contracts::sha(&body.after)?;
            let (reference, heads) =
                if let Some(reference) = body.reference.strip_prefix("refs/heads/") {
                    (reference, true)
                } else if let Some(reference) = body.reference.strip_prefix("refs/tags/") {
                    (reference, false)
                } else {
                    return Err(error("invalid_github_ref"));
                };
            branch(reference)?;
            if body.deleted || !heads {
                None
            } else {
                if body.after.bytes().all(|b| b == b'0') {
                    return Err(error("invalid_github_push"));
                }
                Some(Push {
                    installation_id: body.installation.id.to_string(),
                    repository_id: body.repository.id.to_string(),
                    branch: reference.into(),
                    sha: body.after,
                    workflow_path: None,
                })
            }
        } else if event == "workflow_run" {
            workflow_candidate(raw)?
        } else {
            None
        };
        Ok(VerifiedGithubEvent {
            delivery_id: delivery.to_string(),
            repository_plugin_id: self.repository_plugin_id.clone(),
            body_sha256: hex::encode(Sha256::digest(raw)),
            event: event.into(),
            push,
        })
    }
}

fn logical_key(source: &Value, sha: &str, workflow: bool) -> Result<String> {
    let input = json!([
        source["id"],
        source["revision"],
        source["connection_generation"],
        source["environment_revision"],
        sha
    ]);
    Ok(format!(
        "github-{}-{}",
        if workflow { "workflow" } else { "push" },
        hex::encode(Sha256::digest(
            serde_json::to_vec(&input).map_err(|_| error("invalid_source"))?
        ))
    ))
}

impl BuildService {
    /// Existing source-management authority gives explicit consent; publication remains separate.
    pub async fn configure_github_build_trigger(
        &self,
        actor: &Principal,
        project: &str,
        source_id: &str,
        revision: i32,
        enabled: bool,
    ) -> Result<Value> {
        self.require_execution(true)?;
        crate::contracts::id(project)?;
        crate::contracts::id(source_id)?;
        if revision <= 0 {
            return Err(error("invalid_revision"));
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        Self::module(&tx, false).await?;
        Self::authorize(&tx, actor, project, true, true).await?;
        let source = Self::source(&tx, source_id, project).await?;
        if source["revision"] != revision {
            return Err(error("source_revision_conflict"));
        }
        if enabled {
            Self::active(&source)?;
            if source["repository_plugin_id"].as_str().is_none()
                || source["build_provider"] != "isolated"
            {
                return Err(error("repository_provider_unavailable"));
            }
        }
        execute(&tx,"UPDATE webdock_auth.git_source SET webhook_enabled=$2,webhook_subject=$3,webhook_revision=revision,webhook_connection_generation=$4,webhook_environment_revision=environment_revision WHERE id=$1",vec![source_id.into(),enabled.into(),actor.user_id.clone().into(),(source["connection_generation"].as_i64().ok_or_else(||error("invalid_source"))? as i32).into()]).await?;
        self.audit(
            &tx,
            &actor.user_id,
            "git.source.webhook",
            source_id,
            if enabled { "enabled" } else { "disabled" },
        )
        .await?;
        Self::authorize(&tx, actor, project, true, true).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"sourceID":source_id,"revision":revision,"webhookEnabled":enabled}))
    }

    pub async fn ingest_github_webhook(
        &self,
        event: &VerifiedGithubEvent,
        admission: &repository::RepositoryAdmission,
    ) -> Result<Value> {
        self.ingest_github_webhook_with_admission(event, admission, true)
            .await
    }
    /// `allow_new` is derived by the trusted Core feature gate, never from event JSON.
    /// Exact receipts remain replayable during draining; new deliveries cannot enqueue work.
    pub async fn ingest_github_webhook_with_admission(
        &self,
        event: &VerifiedGithubEvent,
        admission: &repository::RepositoryAdmission,
        allow_new: bool,
    ) -> Result<Value> {
        let tx = self.worker_tx().await?;
        execute(
            &tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("github-delivery:{}", event.delivery_id).into()],
        )
        .await?;
        if let Some(prior)=rows(&tx,"SELECT to_jsonb(d) AS value FROM webdock_auth.git_webhook_delivery d WHERE provider='github' AND delivery_id=$1::uuid",vec![event.delivery_id.clone().into()]).await?.pop() {
            if prior["body_sha256"]!=event.body_sha256 || prior["event"]!=event.event || prior["repository_plugin_id"]!=event.repository_plugin_id {return Err(error("github_webhook_delivery_conflict"));}
            tx.commit().await.map_err(db_error)?;
            return Ok(json!({"buildIDs":prior["build_ids"],"replayed":true}));
        }
        if !allow_new {
            return Err(error("build_admission_draining"));
        }
        let mut builds = Vec::<String>::new();
        if let Some(push) = &event.push {
            // Worker continuation authority permits draining; a fresh push is new admission.
            self.scope(&tx, "plugin-admission").await?;
            Self::module(&tx, false).await?;
            self.scope(&tx, "git-worker").await?;
            let workflow = push.workflow_path.is_some();
            let trigger = if workflow { "workflow_run" } else { "push" };
            let kind = if workflow {
                "github-workflow"
            } else {
                "github-push"
            };
            let candidates=rows(&tx,"SELECT to_jsonb(s) AS value FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id AND c.customer_id=s.customer_id JOIN webdock_auth.git_repository r ON r.connection_id=c.id AND r.customer_id=s.customer_id AND r.repository_id=s.repository_id WHERE s.enabled AND s.webhook_enabled AND s.webhook_revision=s.revision AND s.branch=$1 AND s.repository_id=$2 AND c.installation_id=$3 AND c.repository_plugin_id=$4 AND c.state='active' AND s.webhook_trigger=$5 AND ($6::text IS NULL OR s.workflow_path=$6 AND s.execution_backend='kubernetes') ORDER BY s.id LIMIT 101",vec![push.branch.clone().into(),push.repository_id.clone().into(),push.installation_id.clone().into(),event.repository_plugin_id.clone().into(),trigger.into(),push.workflow_path.clone().into()]).await?;
            if candidates.len() > 100 {
                return Err(error("github_webhook_source_limit"));
            }
            for candidate in candidates {
                let source = Self::source(
                    &tx,
                    field(&candidate, "id")?,
                    field(&candidate, "project_id")?,
                )
                .await?;
                // Source and connection are rechecked after acquiring the source lock.
                if source["installation_id"] != push.installation_id
                    || source["repository_id"] != push.repository_id
                    || source["repository_plugin_id"] != event.repository_plugin_id
                    || source["branch"] != push.branch
                    || source["connection_state"] != "active"
                    || source["webhook_trigger"] != trigger
                    || workflow
                        && (source["workflow_path"].as_str() != push.workflow_path.as_deref()
                            || source["execution_backend"] != "kubernetes")
                {
                    continue;
                }
                if rows(&tx,"SELECT to_jsonb(c.id) AS value FROM webdock_auth.git_connection c WHERE id=$1 AND generation=$2 AND state='active' AND repository_plugin_id=$3 FOR SHARE",vec![field(&source,"connection_id")?.into(),(source["connection_generation"].as_i64().ok_or_else(||error("invalid_source"))? as i32).into(),event.repository_plugin_id.clone().into()]).await?.is_empty() {continue;}
                let actor=rows(&tx,"SELECT coalesce(webdock_rls.git_source_build_trigger_authority($1,$2),'null'::jsonb) AS value",vec![field(&source,"id")?.into(),(source["revision"].as_i64().ok_or_else(||error("invalid_source"))? as i32).into()]).await?.pop().unwrap_or(Value::Null);
                if actor.is_null() {
                    continue;
                }
                if workflow {
                    Self::kubernetes_source_allowed(
                        &tx,
                        field(&source, "customer_id")?,
                        field(&source, "project_id")?,
                    )
                    .await?;
                }
                let reference = if workflow {
                    push.branch.as_str()
                } else {
                    push.sha.as_str()
                };
                let input = json!({"installationID":push.installation_id,"repositoryID":push.repository_id,"reference":reference});
                let (policy, _) = admission
                    .policy(
                        &event.repository_plugin_id,
                        field(&source, "project_id")?,
                        Action::Resolve,
                        &input,
                        false,
                    )
                    .await?;
                if policy.customer_id != field(&source, "customer_id")? {
                    return Err(error("repository_scope_changed"));
                }
                if workflow {
                    let repository =
                        Self::repository(&source, RepositoryContext::SourceTrigger { tx: &tx })?;
                    let head = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        self.provider.resolve(&repository, &push.branch),
                    )
                    .await
                    .map_err(|_| error("repository_timeout"))??;
                    if head.sha != push.sha {
                        continue;
                    }
                    // A current head is insufficient after actor/module/provider consent was revoked during I/O.
                    self.scope(&tx, "plugin-admission").await?;
                    Self::module(&tx, false).await?;
                    self.scope(&tx, "git-worker").await?;
                    let current=rows(&tx,"SELECT coalesce(webdock_rls.git_source_build_trigger_authority($1,$2),'null'::jsonb) AS value",vec![field(&source,"id")?.into(),(source["revision"].as_i64().ok_or_else(||error("invalid_source"))? as i32).into()]).await?.pop().unwrap_or(Value::Null);
                    if current.is_null() || current["subject"] != actor["subject"] {
                        continue;
                    }
                    Self::kubernetes_source_allowed(
                        &tx,
                        field(&source, "customer_id")?,
                        field(&source, "project_id")?,
                    )
                    .await?;
                    admission
                        .policy(
                            &event.repository_plugin_id,
                            field(&source, "project_id")?,
                            Action::Resolve,
                            &input,
                            false,
                        )
                        .await?;
                }
                let key = logical_key(&source, &push.sha, workflow)?;
                let prior=rows(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE project_id=$1 AND idempotency_key=$2",vec![field(&source,"project_id")?.into(),key.clone().into()]).await?.pop();
                let id = if let Some(prior) = prior {
                    if prior["source_id"] != source["id"]
                        || prior["source_revision"] != source["revision"]
                        || prior["connection_generation"] != source["connection_generation"]
                        || prior["environment_revision"] != source["environment_revision"]
                        || prior["source_sha"] != push.sha
                        || workflow && prior["trigger_kind"] != kind
                    {
                        return Err(error("github_webhook_build_conflict"));
                    }
                    field(&prior, "id")?.to_owned()
                } else {
                    execute(
                        &tx,
                        "SELECT webdock_rls.cancel_obsolete_git_builds()",
                        vec![],
                    )
                    .await?;
                    let count=rows(&tx,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.git_build WHERE project_id=$1 AND status IN ('queued','running','artifact-ready')",vec![field(&source,"project_id")?.into()]).await?.pop().and_then(|v|v.as_i64()).unwrap_or(10);
                    if count >= 10 {
                        return Err(error("build_queue_limit"));
                    }
                    let (identity, target_revision) = self
                        .hosting
                        .git_environment_snapshot(&tx, field(&source, "id")?)
                        .await
                        .map_err(hosting)?;
                    let id = self.id().await?;
                    execute(&tx,"INSERT INTO webdock_auth.git_build(id,customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,subject,idempotency_key,environment_identity,target_revision,trigger_kind) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",vec![id.clone().into(),field(&source,"customer_id")?.into(),field(&source,"project_id")?.into(),field(&source,"id")?.into(),push.sha.clone().into(),(source["revision"].as_i64().unwrap() as i32).into(),(source["connection_generation"].as_i64().unwrap() as i32).into(),(source["environment_revision"].as_i64().unwrap() as i32).into(),field(&actor,"subject")?.into(),key.into(),identity.clone().into(),(target_revision as i32).into(),kind.into()]).await?;
                    let superseded=rows(&tx,"UPDATE webdock_auth.git_build SET status='cancelled',phase='cancelled',generation=generation+1,lease_until=NULL,finished_at=now(),failure_code='SUPERSEDED' WHERE source_id=$1 AND id<>$2 AND trigger_kind=$3 AND status='queued' AND generation<2147483646 RETURNING jsonb_build_object('id',id) AS value",vec![field(&source,"id")?.into(),id.clone().into(),kind.into()]).await?;
                    for old in superseded {
                        Self::check(&tx, field(&old, "id")?, "completed", Some("cancelled"))
                            .await?;
                    }
                    execute(&tx,"UPDATE webdock_auth.git_source SET latest_build_id=$2,environment_identity=$3 WHERE id=$1",vec![field(&source,"id")?.into(),id.clone().into(),identity.into()]).await?;
                    Self::check(&tx, &id, "queued", None).await?;
                    self.audit(
                        &tx,
                        field(&actor, "subject")?,
                        if workflow {
                            "git.builds.workflow"
                        } else {
                            "git.builds.push"
                        },
                        &id,
                        "queued",
                    )
                    .await?;
                    id
                };
                builds.push(id);
            }
        }
        execute(&tx,"INSERT INTO webdock_auth.git_webhook_delivery(provider,delivery_id,repository_plugin_id,body_sha256,event,build_ids) VALUES('github',$1::uuid,$2,$3,$4,$5)",vec![event.delivery_id.clone().into(),event.repository_plugin_id.clone().into(),event.body_sha256.clone().into(),event.event.clone().into(),json!(builds).into()]).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"buildIDs":builds,"replayed":false}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn signed(raw: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&[7; 32]).unwrap();
        mac.update(raw);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }
    fn workflow_body() -> Value {
        json!({"action":"completed","installation":{"id":1234},"repository":{"id":55,"fork":false},"workflow":{"id":77,"path":".github/workflows/quality.yml"},"workflow_run":{"id":88,"workflow_id":77,"head_branch":"release","head_sha":"a".repeat(40),"event":"push","status":"completed","conclusion":"success","repository":{"id":55,"fork":false},"head_repository":{"id":55,"fork":false}}})
    }
    #[test]
    fn workflow_success_requires_push_same_nonfork_repository_and_exact_completed_state() {
        let verifier = GithubWebhookVerifier::new(vec![7; 32], "github".into()).unwrap();
        let verify = |value: Value| {
            let raw = serde_json::to_vec(&value).unwrap();
            verifier
                .verify(
                    &raw,
                    &signed(&raw),
                    "01111111-2222-4333-8444-555555555555",
                    "workflow_run",
                )
                .unwrap()
        };
        assert!(
            verify(workflow_body()).push.is_some(),
            "successful same-repo workflow must produce an admitted candidate"
        );
        for (pointer, value) in [
            ("/action", json!("requested")),
            ("/workflow_run/conclusion", json!("failure")),
            ("/workflow_run/status", json!("in_progress")),
            ("/workflow_run/event", json!("pull_request")),
            ("/workflow_run/event", json!("pull_request_target")),
            ("/workflow_run/event", json!("workflow_dispatch")),
            ("/repository/fork", json!(true)),
            ("/workflow_run/head_repository/fork", json!(true)),
            ("/workflow_run/head_repository/id", json!(99)),
            ("/workflow_run/repository/id", json!(99)),
            ("/workflow_run/workflow_id", json!(99)),
        ] {
            let mut body = workflow_body();
            *body.pointer_mut(pointer).unwrap() = value;
            assert!(
                verify(body).push.is_none(),
                "unsafe workflow candidate at {pointer}"
            );
        }
    }

    #[test]
    fn private_secret_reference_rejects_symlinks_links_and_shared_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("webhook.key");
        std::fs::write(&path, [7; 32]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(GithubWebhookVerifier::from_private_file(&path, "github".into()).is_ok());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(GithubWebhookVerifier::from_private_file(&path, "github".into()).is_ok());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(GithubWebhookVerifier::from_private_file(&path, "github".into()).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(GithubWebhookVerifier::from_private_file(&alias, "github".into()).is_err());
        std::fs::remove_file(&alias).unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(GithubWebhookVerifier::from_private_file(&path, "github".into()).is_err());
        std::fs::remove_file(&alias).unwrap();
        let mut secret = vec![7; 32];
        secret.push(b'\n');
        std::fs::write(&path, &secret).unwrap();
        let verifier = GithubWebhookVerifier::from_private_file(&path, "github".into()).unwrap();
        assert!(
            verifier
                .verify(
                    b"{}",
                    &signed(b"{}"),
                    "01111111-2222-4333-8444-555555555555",
                    "ping"
                )
                .is_err(),
            "private secret bytes are never trimmed"
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(&secret).unwrap();
        mac.update(b"{}");
        assert!(
            verifier
                .verify(
                    b"{}",
                    &format!("sha256={}", hex::encode(mac.finalize().into_bytes())),
                    "01111111-2222-4333-8444-555555555555",
                    "ping"
                )
                .is_ok()
        );
        std::fs::write(&path, vec![7; 4097]).unwrap();
        assert!(GithubWebhookVerifier::from_private_file(&path, "github".into()).is_err());
        assert!(
            GithubWebhookVerifier::from_private_file(
                std::path::Path::new("relative.key"),
                "github".into()
            )
            .is_err()
        );
    }
    #[test]
    fn raw_signature_and_strict_push_identity_gate_the_opaque_proof() {
        let verifier = GithubWebhookVerifier::new(vec![7; 32], "github".into()).unwrap();
        let raw=json!({"installation":{"id":1234},"repository":{"id":55},"ref":"refs/heads/main","after":"a".repeat(40),"deleted":false}).to_string();
        let event = verifier
            .verify(
                raw.as_bytes(),
                &signed(raw.as_bytes()),
                "01111111-2222-4333-8444-555555555555",
                "push",
            )
            .unwrap();
        assert_eq!(event.push.unwrap().sha, "a".repeat(40));
        assert!(
            verifier
                .verify(
                    format!("{raw} ").as_bytes(),
                    &signed(raw.as_bytes()),
                    "01111111-2222-4333-8444-555555555555",
                    "push"
                )
                .is_err()
        );
        assert!(
            verifier
                .verify(raw.as_bytes(), &signed(raw.as_bytes()), "invalid", "push")
                .is_err()
        );
        assert!(
            verifier
                .verify(
                    raw.as_bytes(),
                    &signed(raw.as_bytes()),
                    "01111111-2222-4333-8444-555555555555",
                    "Push"
                )
                .is_err()
        );
        assert!(
            verifier
                .verify(&vec![0; MAX_GITHUB_WEBHOOK_BODY + 1], "", "", "push")
                .is_err()
        );
    }
    #[test]
    fn ignored_events_still_require_the_signature_and_build_keys_bind_all_generations() {
        let verifier = GithubWebhookVerifier::new(vec![7; 32], "github".into()).unwrap();
        assert!(
            verifier
                .verify(
                    b"{}",
                    "sha256=00",
                    "01111111-2222-4333-8444-555555555555",
                    "ping"
                )
                .is_err()
        );
        assert!(
            verifier
                .verify(
                    b"{}",
                    &signed(b"{}"),
                    "01111111-2222-4333-8444-555555555555",
                    "ping"
                )
                .unwrap()
                .push
                .is_none()
        );
        let deleted=json!({"installation":{"id":1234},"repository":{"id":55},"ref":"refs/heads/main","after":"0".repeat(40),"deleted":true}).to_string();
        assert!(
            verifier
                .verify(
                    deleted.as_bytes(),
                    &signed(deleted.as_bytes()),
                    "01111111-2222-4333-8444-555555555555",
                    "push"
                )
                .unwrap()
                .push
                .is_none()
        );
        for invalid in [
            "main/./bad",
            "main/foo.lock/bad",
            "main/bad.",
            "main//bad",
            "../main",
        ] {
            assert!(branch(invalid).is_err());
        }
        let mut source =
            json!({"id":"100","revision":1,"connection_generation":1,"environment_revision":1});
        let original = logical_key(&source, &"a".repeat(40), false).unwrap();
        for field in ["revision", "connection_generation", "environment_revision"] {
            source[field] = json!(2);
            assert_ne!(
                logical_key(&source, &"a".repeat(40), false).unwrap(),
                original
            );
            source[field] = json!(1);
        }
        assert_ne!(
            logical_key(&source, &"b".repeat(40), false).unwrap(),
            original
        );
    }
}
