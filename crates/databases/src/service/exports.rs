use super::binding::validate_certificate;
use super::*;
use crate::DatabaseExportSubject;
use webdock_export_protocol::{SourceKind, SourceRef, wire::DatabaseMaterial};
impl DatabaseService {
    async fn export_snapshot_in(
        &self,
        tx: &DatabaseTransaction,
        actor: &DatabaseExportSubject<'_>,
        id: &str,
    ) -> Result<Value> {
        if !webdock_export_protocol::decimal(id) {
            return Err(invalid());
        }
        self.installed_scoped(tx, false).await?;
        let a=one(tx,"SELECT jsonb_build_object('subject',webdock_rls.actor()->>'subject','session',webdock_rls.actor()->>'sessionID','preview',webdock_rls.actor()->'preview','allowed',webdock_rls.customer_permission($1,'org:databases:manage')) AS value",vec![actor.customer.into()]).await?;
        if a["subject"] != actor.subject
            || a["session"] != actor.session
            || a["allowed"] != true
            || a["preview"] != Value::Null
        {
            return Err(denied());
        }
        let d=one(tx,"SELECT to_jsonb(d)||jsonb_build_object('backend_state',b.state,'backend_kind',b.kind,'cluster_id',b.cluster_id,'cnpg_uid',b.cnpg_uid,'host',b.host,'port',b.direct_port,'ca',b.ca_certificate,'endpoint_ready',b.endpoint_ready) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id JOIN webdock_admin.projects p ON p.id=d.project_id AND p.status='active' JOIN webdock_admin.customers c ON c.id=d.customer_id AND c.status='active' WHERE d.id=$1 AND d.customer_id=$2 AND d.project_id=$3 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion x WHERE x.project_id=p.id AND x.status<>'deleted') FOR SHARE OF d,b",vec![id.into(),actor.customer.into(),actor.project.into()]).await?;
        if d["kind"] != "dedicated"
            || d["backend_kind"] != "dedicated"
            || d["state"] != "ready"
            || d["backend_state"] != "ready"
            || d["endpoint_ready"] != true
            || !d["pending_backend_id"].is_null()
            || d["confirmed_credentials_encrypted"].is_null()
            || d["confirmed_credential_revision"] != d["credential_revision"]
            || d["revision"] != d["observed_revision"]
            || d["database_name"] != format!("db_{id}")
            || d["cnpg_uid"].as_str().is_none_or(str::is_empty)
        {
            return Err(conflict(
                "Dedicated source is not confirmed ready for export.",
            ));
        }
        validate_certificate(text(&d, "ca")?, false)?;
        Ok(d)
    }
    fn export_reference(actor: &DatabaseExportSubject<'_>, d: &Value) -> Result<SourceRef> {
        let binding = webdock_export_protocol::fingerprint(&json!([
            actor.customer,
            actor.project,
            d["id"],
            d["backend_id"],
            d["revision"],
            d["confirmed_credential_revision"],
            d["cluster_id"],
            d["cnpg_uid"],
            d["host"],
            d["port"],
            d["ca"]
        ]))
        .map_err(|_| unavailable())?;
        Ok(SourceRef {
            version: 1,
            kind: SourceKind::Postgres,
            id: text(d, "id")?.into(),
            binding,
            commit: None,
        })
    }
    pub async fn export_reference_in(
        &self,
        tx: &DatabaseTransaction,
        actor: &DatabaseExportSubject<'_>,
        id: &str,
    ) -> Result<SourceRef> {
        let d = self.export_snapshot_in(tx, actor, id).await?;
        Self::export_reference(actor, &d)
    }
    pub async fn export_material_in(
        &self,
        tx: &DatabaseTransaction,
        actor: &DatabaseExportSubject<'_>,
        reference: &SourceRef,
        expires_at: i64,
    ) -> Result<DatabaseMaterial> {
        let d = self.export_snapshot_in(tx, actor, &reference.id).await?;
        if Self::export_reference(actor, &d)? != *reference
            || expires_at <= chrono::Utc::now().timestamp()
            || expires_at > chrono::Utc::now().timestamp() + 45
        {
            return Err(denied());
        }
        let revision = number(&d, "confirmed_credential_revision")?;
        let credentials: DatabaseCredentials = self
            .cipher
            .open(
                text(&d, "confirmed_credentials_encrypted")?,
                &format!("database:{}:{revision}", reference.id),
            )
            .map_err(|_| unavailable())?;
        let password = webdock_export_protocol::readonly_password(
            &reference.id,
            &credentials.application_password,
        )
        .map_err(|_| conflict("Reconcile a native generated readonly export credential first."))?;
        Ok(DatabaseMaterial {
            host: text(&d, "host")?.into(),
            port: u32::try_from(number(&d, "port")?).map_err(|_| unavailable())?,
            database: text(&d, "database_name")?.into(),
            username: format!("wd_export_{}", reference.id),
            password,
            ca_certificate: text(&d, "ca")?.into(),
            expires_at,
            binding: reference.binding.clone(),
        })
    }
}
