//! Composition of the fixed archive source lane with existing native product readers.
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_export_protocol::{SourceKind, SourceRef, wire::DatabaseMaterial};
use webdock_plugin_host::{ExportJob, ExportSources, InvocationScope, PluginError};
type Result<T> = std::result::Result<T, PluginError>;
fn denied() -> PluginError {
    PluginError::denied()
}
async fn rows(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Vec<Value>> {
    tx.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map_err(|_| denied())?
    .into_iter()
    .map(|r| r.try_get("", "value").map_err(|_| denied()))
    .collect()
}
pub struct Sources {
    pub databases: Arc<webdock_databases::DatabaseService>,
    pub repository: Option<Arc<webdock_build::RepositoryAdmission>>,
}
impl Sources {
    fn actor<'a>(scope: &'a InvocationScope) -> webdock_databases::DatabaseExportSubject<'a> {
        webdock_databases::DatabaseExportSubject {
            customer: &scope.customer_id,
            project: &scope.project_id,
            subject: &scope.subject,
            session: &scope.session_id,
        }
    }
    async fn report_ref(scope: &InvocationScope, tx: &DatabaseTransaction) -> Result<SourceRef> {
        let r=rows(tx,"SELECT jsonb_build_object('id',p.id,'customer',p.customer_id) AS value FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1 AND p.customer_id=$2 AND p.status='active' AND c.status='active' AND webdock_rls.customer_allowed(c.id,false) AND webdock_rls.actor()->>'subject'=$3 AND webdock_rls.actor()->>'sessionID'=$4 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')",vec![scope.project_id.clone().into(),scope.customer_id.clone().into(),scope.subject.clone().into(),scope.session_id.clone().into()]).await?;
        if r.len() != 1 {
            return Err(denied());
        }
        Ok(SourceRef {
            version: 1,
            kind: SourceKind::Report,
            id: scope.project_id.clone(),
            binding: webdock_export_protocol::fingerprint(&r[0]).map_err(|_| denied())?,
            commit: None,
        })
    }
}
#[async_trait::async_trait]
impl ExportSources for Sources {
    async fn choices(&self, scope: &InvocationScope, tx: &DatabaseTransaction) -> Result<Value> {
        Self::report_ref(scope, tx).await?;
        let pg=rows(tx,"SELECT jsonb_build_object('id',d.id,'kind','postgres','state','ready') AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.project_id=$1 AND d.customer_id=$2 AND d.kind='dedicated' AND b.kind='dedicated' AND d.state='ready' AND b.state='ready' AND b.endpoint_ready AND d.confirmed_credential_revision=d.credential_revision AND d.observed_revision=d.revision AND d.pending_backend_id IS NULL AND webdock_rls.customer_permission(d.customer_id,'org:databases:manage') ORDER BY d.id LIMIT 100",vec![scope.project_id.clone().into(),scope.customer_id.clone().into()]).await?;
        let git=rows(tx,"SELECT jsonb_build_object('id',s.id,'kind','project','state','ready') AS value FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE s.project_id=$1 AND s.customer_id=$2 AND s.enabled AND c.state='active' AND c.repository_plugin_id IS NOT NULL AND webdock_rls.customer_permission(s.customer_id,'org:deployments:create') ORDER BY s.id LIMIT 100",vec![scope.project_id.clone().into(),scope.customer_id.clone().into()]).await?;
        let mut choices = vec![json!({"id":scope.project_id,"kind":"report","state":"ready"})];
        choices.extend(pg);
        choices.extend(
            webdock_build::BuildService::export_log_choices(scope, tx)
                .await
                .map_err(|_| denied())?,
        );
        if self.repository.is_some() {
            choices.extend(git);
        }
        Ok(
            json!({"sources":choices,"maxBytes":webdock_export_protocol::MAX_BYTES,"maxRetentionSeconds":86400,"pgSourceRequiresReadonlyRole":true}),
        )
    }
    async fn bind(
        &self,
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        kind: SourceKind,
        id: &str,
    ) -> Result<SourceRef> {
        if !webdock_export_protocol::decimal(id) {
            return Err(denied());
        }
        match kind {
            SourceKind::Postgres => self
                .databases
                .export_reference_in(tx, &Self::actor(scope), id)
                .await
                .map_err(|_| denied()),
            SourceKind::Project => self
                .repository
                .as_ref()
                .ok_or_else(denied)?
                .export_binding(scope, tx, id)
                .await
                .map_err(|_| denied()),
            SourceKind::BuildLog => {
                webdock_build::BuildService::export_log_reference(scope, tx, id)
                    .await
                    .map_err(|_| denied())
            }
            SourceKind::Report => {
                if id != scope.project_id {
                    return Err(denied());
                }
                Self::report_ref(scope, tx).await
            }
        }
    }
    async fn validate(
        &self,
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        expected: &SourceRef,
    ) -> Result<()> {
        expected.validate().map_err(|_| denied())?;
        if expected.kind == SourceKind::Project {
            return self
                .repository
                .as_ref()
                .ok_or_else(denied)?
                .validate_export_source(scope, tx, expected)
                .await
                .map_err(|_| denied());
        }
        if self.bind(scope, tx, expected.kind, &expected.id).await? != *expected {
            return Err(denied());
        }
        Ok(())
    }
    async fn database(
        &self,
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        source: &SourceRef,
        expires: i64,
    ) -> Result<DatabaseMaterial> {
        self.databases
            .export_material_in(tx, &Self::actor(scope), source, expires)
            .await
            .map_err(|_| denied())
    }
    async fn report(
        &self,
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        source: &SourceRef,
    ) -> Result<Vec<u8>> {
        self.validate(scope, tx, source).await?;
        let resources=rows(tx,"SELECT jsonb_build_object('plugin',plugin_id,'offering',offering_id,'count',count(*)) AS value FROM webdock_auth.plugin_resource WHERE project_id=$1 AND customer_id=$2 GROUP BY plugin_id,offering_id ORDER BY plugin_id,offering_id LIMIT 101",vec![scope.project_id.clone().into(),scope.customer_id.clone().into()]).await?;
        if resources.len() > 100 {
            return Err(PluginError::new(
                webdock_plugin_protocol::ErrorCode::InvalidInput,
                "Project report exceeds supported group limit",
            ));
        }
        let builds=rows(tx,"SELECT jsonb_build_object('state',CASE WHEN status IN ('queued','running','succeeded','failed','cancelled') THEN status ELSE 'other' END,'count',count(*)) AS value FROM webdock_auth.git_build WHERE project_id=$1 AND customer_id=$2 GROUP BY CASE WHEN status IN ('queued','running','succeeded','failed','cancelled') THEN status ELSE 'other' END ORDER BY 1",vec![scope.project_id.clone().into(),scope.customer_id.clone().into()]).await?;
        let mut csv = format!(
            "section,kind,count\nproject,{},1\nobserved_at,{},0\n",
            scope.project_id,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| denied())?
                .as_secs()
        );
        for r in resources {
            let plugin = r["plugin"].as_str().ok_or_else(denied)?;
            let offering = r["offering"].as_str().ok_or_else(denied)?;
            if [plugin, offering].iter().any(|s| {
                s.is_empty()
                    || s.len() > 64
                    || !s.as_bytes()[0].is_ascii_lowercase()
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
            }) {
                return Err(denied());
            }
            csv.push_str(&format!(
                "resources,{plugin}.{offering},{}\n",
                r["count"].as_u64().ok_or_else(denied)?
            ));
        }
        for b in builds {
            csv.push_str(&format!(
                "builds,{},{}\n",
                b["state"].as_str().ok_or_else(denied)?,
                b["count"].as_u64().ok_or_else(denied)?
            ));
        }
        Ok(csv.into_bytes())
    }
    async fn build_log(
        &self,
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        source: &SourceRef,
    ) -> Result<Vec<u8>> {
        webdock_build::BuildService::export_log_bytes(scope, tx, source)
            .await
            .map_err(|_| denied())
    }
    async fn archive(
        &self,
        job: ExportJob,
        scope: &InvocationScope,
        source: &SourceRef,
    ) -> Result<(SourceRef, Vec<u8>)> {
        self.repository
            .as_ref()
            .ok_or_else(denied)?
            .archive_for_export(job, scope, source)
            .await
            .map_err(|_| denied())
    }
}
