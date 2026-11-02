//! Project exports reuse normalized repository transport under an exact Core plugin-job proof.
use super::repository::{PluginRepositoryProvider, RepositoryAdmission};
use super::*;
use sea_orm::DatabaseTransaction;
use serde_json::{Value, json};
use webdock_export_protocol::{SourceKind, SourceRef, fingerprint};
use webdock_plugin_host::{ExportJob, InvocationScope};
impl RepositoryAdmission {
    async fn export_reference(
        &self,
        scope: &InvocationScope,
        source: &Value,
        commit: Option<String>,
    ) -> Result<SourceRef> {
        if source["customer_id"] != scope.customer_id
            || source["project_id"] != scope.project_id
            || source["enabled"] != true
            || source["connection_state"] != "active"
        {
            return Err(error("source_unavailable"));
        }
        let plugin = source["repository_plugin_id"]
            .as_str()
            .ok_or_else(|| error("repository_provider_unavailable"))?;
        let input = json!({"installationID":source["installation_id"],"repositoryID":source["repository_id"],"reference":source["branch"]});
        let (policy, binding) = self
            .policy(
                plugin,
                &scope.project_id,
                webdock_repository_protocol::Action::Resolve,
                &input,
                true,
            )
            .await?;
        if policy.customer_id != scope.customer_id {
            return Err(error("repository_scope_changed"));
        }
        if let Some(sha) = &commit {
            self.policy(plugin,&scope.project_id,webdock_repository_protocol::Action::Archive,&json!({"installationID":source["installation_id"],"repositoryID":source["repository_id"],"commit":sha}),true).await?;
        }
        let hash = fingerprint(&json!([
            scope.customer_id,
            scope.project_id,
            source["id"],
            source["revision"],
            source["connection_generation"],
            source["repository_id"],
            policy.generation,
            policy.grant_revision,
            policy.configuration_revision,
            binding.fingerprint(&policy)?
        ]))
        .map_err(|_| error("invalid_source"))?;
        Ok(SourceRef {
            version: 1,
            kind: SourceKind::Project,
            id: source["id"]
                .as_str()
                .ok_or_else(|| error("invalid_source"))?
                .into(),
            binding: hash,
            commit,
        })
    }
    pub async fn export_binding(
        &self,
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        id: &str,
    ) -> Result<SourceRef> {
        let access=rows(tx,"SELECT jsonb_build_object('subject',webdock_rls.actor()->>'subject','session',webdock_rls.actor()->>'sessionID','allowed',webdock_rls.customer_permission($1,'org:deployments:create'),'mode',(SELECT h.mode FROM webdock_auth.hosting_project h WHERE h.project_id=$2 AND h.customer_id=$1),'operator',webdock_rls.operator() AND EXISTS(SELECT 1 FROM webdock_admin.users u WHERE u.auth_subject=webdock_rls.actor()->>'subject' AND u.role='operator')) AS value",vec![scope.customer_id.clone().into(),scope.project_id.clone().into()]).await?.pop().ok_or_else(||error("permission_denied"))?;
        if access["subject"] != scope.subject
            || access["session"] != scope.session_id
            || access["allowed"] != true
            || (access["mode"] != "selfservice" && access["operator"] != true)
        {
            return Err(error("permission_denied"));
        }
        BuildService::module(tx, true).await?;
        let source = BuildService::source(tx, id, &scope.project_id).await?;
        self.export_reference(scope, &source, None).await
    }
    pub async fn validate_export_source(
        &self,
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        expected: &SourceRef,
    ) -> Result<()> {
        expected.validate().map_err(|_| error("invalid_source"))?;
        let observed = self.export_binding(scope, tx, &expected.id).await?;
        if observed.binding != expected.binding || expected.kind != SourceKind::Project {
            return Err(error("repository_scope_changed"));
        }
        if let Some(commit) = &expected.commit {
            let source = BuildService::source(tx, &expected.id, &scope.project_id).await?;
            self.export_reference(scope, &source, Some(commit.clone()))
                .await?;
        }
        Ok(())
    }
    pub async fn archive_for_export(
        self: &std::sync::Arc<Self>,
        proof: ExportJob,
        scope: &InvocationScope,
        expected: &SourceRef,
    ) -> Result<(SourceRef, Vec<u8>)> {
        self.host
            .validate_export_job(
                &proof
                    .snapshot()
                    .map_err(|_| error("repository_authority_revoked"))?,
            )
            .await
            .map_err(|_| error("repository_authority_revoked"))?;
        let tx = self.tx(true).await?;
        let source = BuildService::source(&tx, &expected.id, &scope.project_id).await?;
        let mut observed = self
            .export_reference(scope, &source, expected.commit.clone())
            .await?;
        if observed.binding != expected.binding {
            return Err(error("repository_scope_changed"));
        }
        let repository = BuildService::repository(
            &source,
            RepositoryContext::PluginJob {
                proof: Box::new(proof),
            },
        )?;
        let branch = source["branch"]
            .as_str()
            .ok_or_else(|| error("invalid_source"))?
            .to_owned();
        tx.commit().await.map_err(db_error)?;
        let provider = PluginRepositoryProvider::new(self.clone());
        let commit = match &expected.commit {
            Some(sha) => sha.clone(),
            None => provider.resolve(&repository, &branch).await?.sha,
        };
        observed.commit = Some(commit.clone());
        let bytes = provider.archive(&repository, &commit).await?;
        if bytes.len() as u64 > webdock_export_protocol::MAX_BYTES {
            return Err(error("repository_archive_limit"));
        }
        observed.finished().map_err(|_| error("invalid_source"))?;
        Ok((observed, bytes))
    }
}

#[path = "export_logs.rs"]
mod logs;
